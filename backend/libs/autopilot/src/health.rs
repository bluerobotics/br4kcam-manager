use std::{
    collections::HashSet,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant}, // std::time::Instant: sync health_state under Mutex
};

use ::mavlink::ardupilotmega::{MavComponent, MavMessage, MavSeverity, STATUSTEXT_DATA};
use br4kcam_api::{AutopilotHealth, Diagnostics, LuaScriptStatus};
use indexmap::IndexMap;
use once_cell::sync::OnceCell;
use tokio::sync::{Notify, broadcast};
use tracing::*;
use uuid::Uuid;

use crate::{
    actuators_watch::{self, ServoStreamHealth},
    mavlink::{self, Message},
    parameters::{self, ParamType},
};

const FRAME_SILENCE: Duration = Duration::from_secs(5);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const SERVO_SILENCE: Duration = Duration::from_secs(10);
/// One exhausted command RPC (already multi-second) is enough evidence.
const RPC_FAILURES_TO_UNRESPONSIVE: u8 = 1;
const DEGRADED_GRACE: Duration = Duration::from_secs(1);
/// ArduPilot re-emits a broken Lua script's STATUSTEXT about once per second; keep
/// this above that interval so a persistent fault stays visible, but below the time
/// an operator would treat a one-shot boot glitch as still failing.
#[cfg(not(test))]
const LUA_SCRIPT_FAILURE_TTL: Duration = Duration::from_secs(15);
#[cfg(test)]
const LUA_SCRIPT_FAILURE_TTL: Duration = Duration::from_millis(50);
const BR4KCAM_LUA_SCRIPT: &str = "br4kcam.lua";

static HEALTH: OnceCell<Mutex<Health>> = OnceCell::new();
static HEALTH_TX: OnceCell<broadcast::Sender<()>> = OnceCell::new();
static BACKEND_VERSION: OnceCell<String> = OnceCell::new();
static STARTED: AtomicBool = AtomicBool::new(false);
static CLASSIFIER_NOTIFY: OnceCell<Notify> = OnceCell::new();

struct Health {
    state: AutopilotHealth,
    detail: Option<String>,
    endpoint_ok: Option<bool>,
    endpoint_detail: Option<String>,
    last_frame_at: Option<Instant>,
    last_heartbeat_at: Option<Instant>,
    consecutive_rpc_failures: u8,
    last_rpc_error: Option<String>,
    servo_stalled: bool,
    syncing: bool,
    rebooting: bool,
    lua_scripting_disabled: bool,
    lua_scripting_disabled_logged: bool,
    lua_script: LuaScriptStatus,
    lua_script_failure: Option<(Instant, String)>,
    param_drifts: IndexMap<(Uuid, String), ParameterDrift>,
    script_reloads: u32,
    frames_lagged: u64,
    ever_online: bool,
    pending: Option<(AutopilotHealth, Instant)>,
}

pub(crate) struct ClassifyHealthInput<'a> {
    pub endpoint_ok: Option<bool>,
    pub endpoint_detail: Option<&'a str>,
    pub mavlink_up: bool,
    pub frame_age: Option<Duration>,
    pub heartbeat_age: Option<Duration>,
    pub servo_age: Option<Duration>,
    pub rpc_failures: u8,
    pub servo_stalled: bool,
    pub last_rpc_error: Option<&'a str>,
    pub syncing: bool,
    pub servo_detail: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParameterDrift {
    pub name: String,
    pub expected: f32,
    pub actual: f32,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            state: AutopilotHealth::Unknown,
            detail: None,
            endpoint_ok: None,
            endpoint_detail: None,
            last_frame_at: None,
            last_heartbeat_at: None,
            consecutive_rpc_failures: 0,
            last_rpc_error: None,
            servo_stalled: false,
            syncing: false,
            rebooting: false,
            lua_scripting_disabled: false,
            lua_scripting_disabled_logged: false,
            lua_script: LuaScriptStatus::Unknown,
            lua_script_failure: None,
            param_drifts: IndexMap::new(),
            script_reloads: 0,
            frames_lagged: 0,
            ever_online: false,
            pending: None,
        }
    }
}

/// Current autopilot health and detail text for non-`Online` states.
pub fn health() -> (AutopilotHealth, Option<String>) {
    ensure_started();
    let guard = health_state().lock().expect("health lock");
    (guard.state, guard.detail.clone())
}

/// True when BlueOS may have lost our MAVLink endpoint and we should re-create it.
pub fn needs_mavlink_endpoint_ensure() -> bool {
    ensure_started();
    let guard = health_state().lock().expect("health lock");
    match guard.state {
        AutopilotHealth::EndpointSetupFailed => true,
        AutopilotHealth::MavlinkDown => guard.ever_online || guard.last_frame_at.is_some(),
        _ => false,
    }
}

/// True when `SCR_ENABLE` is known and not 1.
pub fn lua_scripting_disabled() -> bool {
    ensure_started();
    health_state()
        .lock()
        .expect("health lock")
        .lua_scripting_disabled
}

/// Whether the installed Lua script matches the current configuration, and what the
/// autopilot said if it is erroring.
pub fn lua_script_status() -> (LuaScriptStatus, Option<String>) {
    ensure_started();
    let mut guard = health_state().lock().expect("health lock");

    let failure = guard.lua_script_failure.as_ref().and_then(|(at, detail)| {
        if at.elapsed() < LUA_SCRIPT_FAILURE_TTL {
            Some(detail.clone())
        } else {
            None
        }
    });
    if guard.lua_script_failure.is_some() && failure.is_none() {
        guard.lua_script_failure = None;
    }

    // A reported failure only adds information while the expected script is the one
    // installed: Missing and Outdated already tell the user what to do about it.
    match (guard.lua_script, failure) {
        (LuaScriptStatus::Ok, Some(failure)) => (LuaScriptStatus::Failing, Some(failure)),
        (status, _) => (status, None),
    }
}

/// Autopilot parameters that no longer match persisted actuator settings.
pub fn parameter_drifts() -> Vec<ParameterDrift> {
    ensure_started();
    let guard = health_state().lock().expect("health lock");
    guard.param_drifts.values().cloned().collect()
}

/// Count a Lua reload triggered by the script no longer answering.
///
/// A reload that works is a working system, so this stays a support counter rather
/// than a problem: flagging it would fire during normal actuator use.
pub(crate) fn note_script_reload() {
    let mut guard = health_state().lock().expect("health lock");
    guard.script_reloads = guard.script_reloads.saturating_add(1);
}

/// Forget a reported scripting failure, after evidence that the script runs again.
// No ensure_started(): called from the servo sample path, and the unit test in
// manager/script.rs must not spawn background tasks.
pub(crate) fn clear_lua_script_failure() {
    let mut guard = health_state().lock().expect("health lock");
    if guard.lua_script_failure.take().is_some() {
        drop(guard);
        notify_health();
    }
}

/// Forget reported parameter drift after a fresh apply or when values match again.
pub(crate) fn clear_stale_parameter_drifts(expected: &HashSet<(Uuid, String)>) {
    let mut guard = health_state().lock().expect("health lock");
    let before = guard.param_drifts.len();
    guard.param_drifts.retain(|key, _| expected.contains(key));
    if guard.param_drifts.len() != before {
        drop(guard);
        notify_health();
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ObserveSource {
    /// Full parameter download at connect or after reboot: establish eligibility only.
    BulkSync,
    /// `PARAM_VALUE` broadcast after bulk sync: report drift when eligible.
    LiveChange,
    /// Post-apply re-evaluation: clear stale drifts, never report new ones.
    AfterApply,
}

pub(crate) fn observe_owned_parameter_value(
    camera_uuid: &Uuid,
    name: &str,
    actual: &ParamType,
    expected: &ParamType,
    source: ObserveSource,
) {
    let drift_key = (*camera_uuid, name.to_owned());
    if parameters::param_values_match(expected, actual, parameters::PARAM_DRIFT_TOLERANCE) {
        crate::manager::owned_parameters::mark_eligible(camera_uuid, name);
        let mut guard = health_state().lock().expect("health lock");
        if guard.param_drifts.shift_remove(&drift_key).is_some() {
            drop(guard);
            notify_health();
        }
        return;
    }

    let report = matches!(source, ObserveSource::LiveChange)
        && crate::manager::owned_parameters::is_eligible(camera_uuid, name);
    if !report {
        return;
    }

    let Some(expected_f) = parameters::param_display_value(expected) else {
        return;
    };
    let Some(actual_f) = parameters::param_display_value(actual) else {
        return;
    };

    let drift = ParameterDrift {
        name: name.to_owned(),
        expected: expected_f,
        actual: actual_f,
    };

    let mut guard = health_state().lock().expect("health lock");
    if guard.param_drifts.get(&drift_key) == Some(&drift) {
        return;
    }

    warn!(
        "Autopilot parameter drift for camera {camera_uuid}: {name} expected {expected_f} but is {actual_f}"
    );
    guard.param_drifts.insert(drift_key, drift);
    drop(guard);
    notify_health();
}

/// Re-read the installed Lua script and publish the result.
///
/// Call it right after the script file is written or removed: without it the change
/// is only picked up by the next classifier tick, so the UI keeps showing the stale
/// status until after the action it just ran has finished.
// No ensure_started(): callers are already on the classifier/manager path, and the unit test in manager/script.rs must not spawn background tasks.
pub(crate) async fn refresh_lua_script_status() {
    let status = crate::manager::Manager::script_status().await;
    if store_lua_script_status(status) {
        notify_health();
    }
}

pub fn diagnostics() -> Diagnostics {
    ensure_started();
    let guard = health_state().lock().expect("health lock");
    let servo = actuators_watch::stream_health();

    let mut diagnostics = Diagnostics {
        mavlink_reconnects: mavlink::reconnect_count(),
        mavlink_frames_lagged: guard.frames_lagged,
        script_reloads: guard.script_reloads,
        backend_version: backend_version_string(),
        settings_error: settings::last_save_error(),
        ..Default::default()
    };

    diagnostics.last_frame_age_ms = guard
        .last_frame_at
        .map(|at| at.elapsed().as_millis() as u64);
    diagnostics.last_heartbeat_age_ms = guard
        .last_heartbeat_at
        .map(|at| at.elapsed().as_millis() as u64);
    diagnostics.last_servo_age_ms = servo
        .last_sample_at
        .map(|at| at.elapsed().as_millis() as u64);

    drop(guard);

    if let Ok(component) = mavlink::component()
        && let Ok(encoding) = component.inner.encoding.try_read()
    {
        diagnostics.param_encoding = Some(format!("{encoding:?}"));
    }

    diagnostics
}

pub fn set_backend_version(version: String) {
    // OnceLock: a second set is a deliberate no-op.
    let _ = BACKEND_VERSION.set(version);
}

fn backend_version_string() -> String {
    BACKEND_VERSION
        .get()
        .cloned()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

pub fn subscribe_health() -> broadcast::Receiver<()> {
    ensure_started();
    health_sender().subscribe()
}

#[instrument(level = "debug", skip_all)]
pub fn report_endpoint_setup(ok: bool, detail: Option<String>) {
    ensure_started();
    let mut guard = health_state().lock().expect("health lock");
    guard.endpoint_ok = Some(ok);
    guard.endpoint_detail = detail;
    drop(guard);
    run_classifier();
}

pub fn rpc_ok() {
    ensure_started();
    let mut guard = health_state().lock().expect("health lock");
    guard.consecutive_rpc_failures = 0;
    drop(guard);
    run_classifier();
}

/// Record a MAVLink command RPC that exhausted retries or failed after ACK.
#[instrument(level = "debug", skip_all)]
pub fn rpc_failed(reason: &str) {
    ensure_started();
    let mut guard = health_state().lock().expect("health lock");
    guard.consecutive_rpc_failures = guard.consecutive_rpc_failures.saturating_add(1);
    guard.last_rpc_error = Some(reason.to_string());
    drop(guard);
    run_classifier();
}

#[instrument(level = "debug", skip_all)]
pub fn set_syncing(syncing: bool) {
    ensure_started();
    let mut guard = health_state().lock().expect("health lock");
    guard.syncing = syncing;
    drop(guard);
    run_classifier();
}

/// Suppress transient degradation while the autopilot reboots.
#[instrument(level = "debug", skip_all)]
pub fn set_rebooting(rebooting: bool) {
    ensure_started();
    let mut guard = health_state().lock().expect("health lock");
    guard.rebooting = rebooting;
    if rebooting {
        guard.pending = None;
    } else {
        drop(guard);
        run_classifier();
    }
}

fn health_state() -> &'static Mutex<Health> {
    HEALTH.get_or_init(|| Mutex::new(Health::default()))
}

fn health_sender() -> &'static broadcast::Sender<()> {
    HEALTH_TX.get_or_init(|| {
        let (sender, _) = broadcast::channel(16);
        sender
    })
}

fn notify_health() {
    if let Err(error) = health_sender().send(()) {
        debug!("No autopilot health subscribers: {error}");
    }
}

/// Caches `status`, returning whether it differed from the cached one.
fn store_lua_script_status(status: LuaScriptStatus) -> bool {
    let mut guard = health_state().lock().expect("health lock");
    if guard.lua_script == status {
        return false;
    }

    if !matches!(status, LuaScriptStatus::Ok | LuaScriptStatus::Unknown) {
        warn!("Autopilot Lua script is {status:?}");
    }
    guard.lua_script = status;
    true
}

fn ensure_started() {
    if STARTED.load(Ordering::Acquire) {
        return;
    }
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    tokio::spawn(frame_drain_task());
    tokio::spawn(classifier_task());
}

#[instrument(level = "debug", skip_all)]
async fn frame_drain_task() {
    loop {
        let Ok(component) = mavlink::component() else {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        };

        let target_system = component.system_id();
        let target_component = MavComponent::MAV_COMP_ID_AUTOPILOT1 as u8;
        let mut receiver = component.get_receiver().await;

        loop {
            match receiver.recv().await {
                Ok(Message::Received((header, message))) => {
                    stamp_frame();
                    if header.system_id != target_system || header.component_id != target_component
                    {
                        continue;
                    }
                    match message {
                        // Any HEARTBEAT from the autopilot component proves it is running.
                        // Do not require ACTIVE/STANDBY: ArduSub/ArduPilot often report
                        // CRITICAL/CALIBRATING/etc while still fully operational, and that
                        // filter left health stuck on AutopilotOffline despite live SERVO.
                        MavMessage::HEARTBEAT(_) => stamp_heartbeat(),
                        MavMessage::STATUSTEXT(status) => note_statustext(&status),
                        _ => {}
                    }
                }
                Ok(Message::ToBeSent(_)) => {}
                Err(broadcast::error::RecvError::Lagged(samples)) => {
                    let mut guard = health_state().lock().expect("health lock");
                    guard.last_frame_at = Some(Instant::now());
                    guard.frames_lagged += samples;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    break;
                }
            }
        }
    }
}

fn stamp_frame() {
    let mut guard = health_state().lock().expect("health lock");
    guard.last_frame_at = Some(Instant::now());
}

fn stamp_heartbeat() {
    let mut guard = health_state().lock().expect("health lock");
    guard.last_heartbeat_at = Some(Instant::now());
}

fn note_statustext(status: &STATUSTEXT_DATA) {
    // ponytail: text longer than 50 characters arrives as several chunks and only the
    // first is kept. It carries the Lua file and line, which is what the user acts on.
    // Upgrade path is reassembling by `id` until a null character.
    if status.chunk_seq != 0 {
        return;
    }

    let Ok(text) = status.text.to_str() else {
        return;
    };

    if !is_scripting_failure(status.severity, text) {
        return;
    }

    let mut guard = health_state().lock().expect("health lock");
    if guard
        .lua_script_failure
        .as_ref()
        .is_some_and(|(_, failure)| failure == text)
    {
        return;
    }

    warn!("Autopilot reported a scripting failure: {text}");
    guard.lua_script_failure = Some((Instant::now(), text.to_owned()));
    drop(guard);

    notify_health();
}

/// Whether a `STATUSTEXT` is the autopilot reporting that scripting is broken.
///
/// `Scripting:` lifecycle noise (`restarted`, `stopped`) arrives at CRITICAL on every
/// ordinary restart, including the ones we ask for, so those are allow-listed and
/// everything else at ERROR or above under the prefix is treated as a failure.
///
/// `Lua:` errors name the script file; only messages mentioning our deployed script
/// count — ArduPilot prefixes every script's errors the same way.
fn is_scripting_failure(severity: MavSeverity, text: &str) -> bool {
    const BENIGN_SCRIPTING_LIFECYCLE: [&str; 2] = ["Scripting: restarted", "Scripting: stopped"];

    if text.starts_with("Lua: ") {
        return scripting_warning_or_above(severity) && text.contains(BR4KCAM_LUA_SCRIPT);
    }

    if text.starts_with("Scripting: ") {
        return scripting_error_or_above(severity) && !BENIGN_SCRIPTING_LIFECYCLE.contains(&text);
    }

    false
}

fn scripting_error_or_above(severity: MavSeverity) -> bool {
    matches!(
        severity,
        MavSeverity::MAV_SEVERITY_EMERGENCY
            | MavSeverity::MAV_SEVERITY_ALERT
            | MavSeverity::MAV_SEVERITY_CRITICAL
            | MavSeverity::MAV_SEVERITY_ERROR
    )
}

fn scripting_warning_or_above(severity: MavSeverity) -> bool {
    scripting_error_or_above(severity) || severity == MavSeverity::MAV_SEVERITY_WARNING
}

fn classifier_notify() -> &'static Notify {
    CLASSIFIER_NOTIFY.get_or_init(Notify::new)
}

async fn classifier_task() {
    let notify = classifier_notify();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // The script file is also written by hand, by MAVFTP and by a reflashed SD card, and
    // none of those tell us. Re-reading it is the only way to notice, but it renders the
    // expected script to compare against, so it is deliberately far slower than the tick.
    let mut script_interval = tokio::time::interval(Duration::from_secs(10));
    script_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = script_interval.tick() => refresh_lua_script_status().await,
            () = notify.notified() => {}
        }
        run_classifier_async().await;
    }
}

fn run_classifier() {
    if tokio::runtime::Handle::try_current().is_ok() {
        classifier_notify().notify_one();
    }
}

async fn run_classifier_async() {
    let lua_disabled = if let Ok(component) = mavlink::component() {
        let params = component.inner.parameters.read().await;
        scr_enable_disabled(&params)
    } else {
        false
    };

    let mut guard = health_state().lock().expect("health lock");

    let stream = actuators_watch::stream_health();
    let now = Instant::now();
    guard.servo_stalled = servo_stalled(&stream, now);

    if lua_disabled && !guard.lua_scripting_disabled_logged {
        warn!("Lua scripting disabled (SCR_ENABLE is not 1)");
        guard.lua_scripting_disabled_logged = true;
    } else if !lua_disabled {
        guard.lua_scripting_disabled_logged = false;
    }
    guard.lua_scripting_disabled = lua_disabled;

    apply_classifier(&mut guard, &stream);
}

fn scr_enable_disabled(params: &indexmap::IndexMap<String, crate::parameters::Parameter>) -> bool {
    let Some(param) = params.get("SCR_ENABLE") else {
        return false;
    };

    match param.value {
        ParamType::REAL32(value) => value != 1.0,
        ParamType::REAL64(value) => value != 1.0,
        ParamType::UINT8(value) => value != 1,
        ParamType::INT8(value) => value != 1,
        ParamType::UINT16(value) => value != 1,
        ParamType::INT16(value) => value != 1,
        ParamType::UINT32(value) => value != 1,
        ParamType::INT32(value) => value != 1,
        ParamType::UINT64(value) => value != 1,
        ParamType::INT64(value) => value != 1,
    }
}

fn apply_classifier(guard: &mut Health, stream: &ServoStreamHealth) {
    if guard.rebooting {
        return;
    }

    let mavlink_up = mavlink::component().is_ok();
    let servo_detail = guard.servo_stalled.then(|| stream_detail(stream));
    let candidate = classify_health(ClassifyHealthInput {
        endpoint_ok: guard.endpoint_ok,
        endpoint_detail: guard.endpoint_detail.as_deref(),
        mavlink_up,
        frame_age: guard.last_frame_at.map(|at| at.elapsed()),
        heartbeat_age: guard.last_heartbeat_at.map(|at| at.elapsed()),
        servo_age: stream.last_sample_at.map(|at| at.elapsed()),
        rpc_failures: guard.consecutive_rpc_failures,
        servo_stalled: guard.servo_stalled,
        last_rpc_error: guard.last_rpc_error.as_deref(),
        syncing: guard.syncing,
        servo_detail: servo_detail.as_deref(),
    });

    if candidate.0 == guard.state {
        guard.pending = None;
        return;
    }

    if candidate.0 == AutopilotHealth::Online {
        publish_transition(guard, candidate);
        return;
    }

    if !needs_debounce(candidate.0) {
        publish_transition(guard, candidate);
        return;
    }

    let now = Instant::now();
    match guard.pending {
        Some((pending_state, first_seen)) if pending_state == candidate.0 => {
            if now.duration_since(first_seen) >= DEGRADED_GRACE {
                publish_transition(guard, candidate);
            }
        }
        _ => {
            guard.pending = Some((candidate.0, now));
        }
    }
}

fn needs_debounce(state: AutopilotHealth) -> bool {
    matches!(
        state,
        AutopilotHealth::EndpointSetupFailed
            | AutopilotHealth::MavlinkDown
            | AutopilotHealth::AutopilotOffline
            | AutopilotHealth::Unresponsive
    )
}

fn publish_transition(guard: &mut Health, candidate: (AutopilotHealth, Option<String>)) {
    let from = guard.state;
    let to = candidate.0;
    if from != to {
        if to == AutopilotHealth::Online || from == AutopilotHealth::Online {
            info!(?from, ?to, "Autopilot health recovered");
        } else if matches!(to, AutopilotHealth::Syncing) {
            info!(?from, ?to, "Autopilot health syncing");
        } else {
            let evidence = candidate.1.as_deref().unwrap_or("unknown");
            warn!(?from, ?to, evidence, "Autopilot health degraded");
        }
    }
    if candidate.0 == AutopilotHealth::Online {
        guard.ever_online = true;
    }
    guard.state = candidate.0;
    guard.detail = candidate.1;
    guard.pending = None;
    notify_health();
}

pub(crate) fn classify_health(input: ClassifyHealthInput<'_>) -> (AutopilotHealth, Option<String>) {
    if input.endpoint_ok.is_none() {
        return (AutopilotHealth::Unknown, None);
    }

    if input.endpoint_ok == Some(false) {
        return (
            AutopilotHealth::EndpointSetupFailed,
            input.endpoint_detail.map(str::to_string),
        );
    }

    if input.syncing {
        return (AutopilotHealth::Syncing, None);
    }

    if !input.mavlink_up || frame_stale(input.frame_age) {
        let detail = input.frame_age.map_or_else(
            || "MAVLink component unavailable".to_string(),
            |age| format!("no MAVLink traffic for {:.1}s", age.as_secs_f32()),
        );
        return (AutopilotHealth::MavlinkDown, Some(detail));
    }

    if input.frame_age.is_none() {
        return (AutopilotHealth::Unknown, None);
    }

    // Autopilot is "running" if it heartbeats OR still emits SERVO_OUTPUT_RAW.
    // Heartbeat-only was too strict: some links/status values never matched.
    if input.frame_age.is_some()
        && heartbeat_stale(input.heartbeat_age)
        && !autopilot_servo_fresh(input.servo_age)
    {
        let detail = input.heartbeat_age.map_or_else(
            || "no autopilot heartbeat received".to_string(),
            |age| format!("no autopilot heartbeat for {:.1}s", age.as_secs_f32()),
        );
        return (AutopilotHealth::AutopilotOffline, Some(detail));
    }

    if input.rpc_failures >= RPC_FAILURES_TO_UNRESPONSIVE || input.servo_stalled {
        let detail = if input.servo_stalled {
            input.servo_detail.map(str::to_string)
        } else {
            input.last_rpc_error.map(str::to_string)
        };
        return (AutopilotHealth::Unresponsive, detail);
    }

    (AutopilotHealth::Online, None)
}

fn frame_stale(frame_age: Option<Duration>) -> bool {
    matches!(frame_age, Some(age) if age > FRAME_SILENCE)
}

fn heartbeat_stale(heartbeat_age: Option<Duration>) -> bool {
    match heartbeat_age {
        None => true,
        Some(age) => age > HEARTBEAT_TIMEOUT,
    }
}

fn autopilot_servo_fresh(servo_age: Option<Duration>) -> bool {
    matches!(servo_age, Some(age) if age <= HEARTBEAT_TIMEOUT)
}

pub(crate) fn servo_stalled(stream: &ServoStreamHealth, now: Instant) -> bool {
    if stream.interest == 0 {
        return false;
    }

    let Some(request_at) = stream.last_request_at else {
        return false;
    };

    if now.duration_since(request_at) < SERVO_SILENCE {
        return false;
    }

    match stream.last_sample_at {
        None => true,
        Some(sample_at) => now.duration_since(sample_at) >= SERVO_SILENCE,
    }
}

fn stream_detail(stream: &ServoStreamHealth) -> String {
    let request_age = stream
        .last_request_at
        .map(|at| at.elapsed().as_secs_f32())
        .unwrap_or(0.0);
    let sample_age = stream.last_sample_at.map(|at| at.elapsed().as_secs_f32());
    match sample_age {
        Some(age) => format!(
            "SERVO_OUTPUT_RAW stale for {age:.0}s with interest and a request {request_age:.0}s ago"
        ),
        None => format!(
            "SERVO_OUTPUT_RAW not received with interest and a request {request_age:.0}s ago"
        ),
    }
}

#[cfg(test)]
static HEALTH_TEST_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serializes tests that mutate process-lifetime health/eligibility, or that hold
/// `CONFIG_APPLY`. `observe_param_value_from_sync` treats a held apply lock as
/// in-apply and skips live PARAM_VALUE.
#[cfg(test)]
pub(crate) async fn lock_health_tests() -> tokio::sync::MutexGuard<'static, ()> {
    HEALTH_TEST_GUARD.lock().await
}
#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;
    use crate::{
        mavlink::parameters::observe_param_value_from_sync,
        parameters::{ActuatorsParameters, ChannelFunction, Parameter},
    };

    /// Resets in-process autopilot health for unit tests. The process-lifetime
    /// [`OnceCell`] is not cleared — tests share globals and must reset observable
    /// state or use disjoint inputs (see S-T6). Call only while holding
    /// [`lock_health_tests`].
    fn health_reset() {
        let mut guard = health_state().lock().expect("health lock");
        *guard = Health::default();
        crate::manager::owned_parameters::clear_eligibility_for_test();
        crate::manager::owned_parameters::clear_expectations_for_test();
    }

    #[test]
    fn scripting_statustext_classifies_failures_and_ignores_lifecycle_noise() {
        assert!(is_scripting_failure(
            MavSeverity::MAV_SEVERITY_CRITICAL,
            "Lua: /scripts/br4kcam.lua:42: attempt to call a nil value"
        ));
        assert!(is_scripting_failure(
            MavSeverity::MAV_SEVERITY_ERROR,
            "Scripting: restart not supported on this board"
        ));
        assert!(!is_scripting_failure(
            MavSeverity::MAV_SEVERITY_ERROR,
            "Lua: /scripts/other.lua:1: attempt to call a nil value"
        ));
        assert!(!is_scripting_failure(
            MavSeverity::MAV_SEVERITY_ERROR,
            "EKF3 IMU0 is using GPS"
        ));
        assert!(!is_scripting_failure(
            MavSeverity::MAV_SEVERITY_DEBUG,
            "Lua: Running br4kcam.lua"
        ));
        assert!(!is_scripting_failure(
            MavSeverity::MAV_SEVERITY_CRITICAL,
            "Scripting: restarted"
        ));
        assert!(!is_scripting_failure(
            MavSeverity::MAV_SEVERITY_CRITICAL,
            "Scripting: stopped"
        ));
    }

    #[tokio::test]
    async fn parameter_drift_eligibility_and_observe_sources() {
        let _health_tests = lock_health_tests().await;
        health_reset();
        let expected = ParamType::UINT16(1500);

        let bulk_mismatch = Uuid::from_u128(10);
        observe_owned_parameter_value(
            &bulk_mismatch,
            "SERVO10_MIN",
            &ParamType::UINT16(1600),
            &expected,
            ObserveSource::BulkSync,
        );
        assert!(parameter_drifts().is_empty());
        assert!(!crate::manager::owned_parameters::is_eligible(
            &bulk_mismatch,
            "SERVO10_MIN"
        ));

        let bulk_match = Uuid::from_u128(11);
        let matched = ParamType::UINT16(1500);
        observe_owned_parameter_value(
            &bulk_match,
            "SERVO10_MIN",
            &matched,
            &matched,
            ObserveSource::BulkSync,
        );
        assert!(crate::manager::owned_parameters::is_eligible(
            &bulk_match,
            "SERVO10_MIN"
        ));

        let live = Uuid::from_u128(12);
        observe_owned_parameter_value(
            &live,
            "SERVO10_MIN",
            &ParamType::UINT16(1600),
            &expected,
            ObserveSource::LiveChange,
        );
        assert!(parameter_drifts().is_empty());
        crate::manager::owned_parameters::mark_eligible(&live, "SERVO10_MIN");
        observe_owned_parameter_value(
            &live,
            "SERVO10_MIN",
            &ParamType::UINT16(1600),
            &expected,
            ObserveSource::LiveChange,
        );
        assert_eq!(parameter_drifts().len(), 1);
        assert_eq!(parameter_drifts()[0].actual, 1600.0);

        let apply = Uuid::from_u128(13);
        let reapplied = ParamType::UINT16(1200);
        crate::manager::owned_parameters::mark_eligible(&apply, "SERVO10_MIN");
        observe_owned_parameter_value(
            &apply,
            "SERVO10_MIN",
            &ParamType::UINT16(1100),
            &reapplied,
            ObserveSource::LiveChange,
        );
        assert_eq!(parameter_drifts().len(), 2);

        observe_owned_parameter_value(
            &apply,
            "SERVO10_MIN",
            &reapplied,
            &reapplied,
            ObserveSource::AfterApply,
        );
        assert_eq!(parameter_drifts().len(), 1);
        assert_eq!(parameter_drifts()[0].actual, 1600.0);

        // A cached baseline that already matches seeds eligibility, so a later
        // PARAM_VALUE from the live stream reports drift while bulk sync does not.
        health_reset();
        let camera_uuid = Uuid::from_u128(0xd11f_0000_0000_0001);
        let actuators = ActuatorsParameters::default();
        let param_name = format!("SERVO{}_FUNCTION", actuators.focus_channel as u8);
        let expected_function = ParamType::INT16(ChannelFunction::Script1 as i16);
        let drifted_function = ParamType::INT16(ChannelFunction::Disabled as i16);

        crate::manager::owned_parameters::install_expectations_for_test(camera_uuid, &actuators);
        let mut cache = IndexMap::new();
        cache.insert(
            param_name.clone(),
            Parameter {
                name: param_name.clone(),
                value: expected_function,
            },
        );
        crate::manager::owned_parameters::establish_baseline_from_cache(&cache);
        assert!(crate::manager::owned_parameters::is_eligible(
            &camera_uuid,
            &param_name
        ));
        assert!(parameter_drifts().is_empty());

        observe_param_value_from_sync(
            &Parameter {
                name: param_name.clone(),
                value: drifted_function,
            },
            u16::MAX,
        );
        let drifts = parameter_drifts();
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].name, param_name);
        assert_eq!(drifts[0].expected, ChannelFunction::Script1 as i16 as f32);
        assert_eq!(drifts[0].actual, ChannelFunction::Disabled as i16 as f32);

        observe_param_value_from_sync(
            &Parameter {
                name: param_name.clone(),
                value: expected_function,
            },
            u16::MAX,
        );
        assert!(parameter_drifts().is_empty());

        observe_param_value_from_sync(
            &Parameter {
                name: param_name,
                value: drifted_function,
            },
            0,
        );
        assert!(parameter_drifts().is_empty());
    }

    #[tokio::test]
    async fn servo_pwm_range_params_are_not_owned() {
        let _health_tests = lock_health_tests().await;
        health_reset();
        let camera_uuid = Uuid::from_u128(0xd11f_0000_0000_0002);
        let actuators = ActuatorsParameters::default();
        let channel = actuators.focus_channel as u8;
        crate::manager::owned_parameters::install_expectations_for_test(camera_uuid, &actuators);

        assert_eq!(
            crate::manager::owned_parameters::expectations_for_param(&format!(
                "SERVO{channel}_FUNCTION"
            ))
            .len(),
            1
        );

        for suffix in ["MIN", "TRIM", "MAX"] {
            let name = format!("SERVO{channel}_{suffix}");
            assert!(
                crate::manager::owned_parameters::expectations_for_param(&name).is_empty(),
                "{name} must not be owned"
            );
            observe_param_value_from_sync(
                &Parameter {
                    name,
                    value: ParamType::UINT16(1),
                },
                u16::MAX,
            );
        }
        assert!(parameter_drifts().is_empty());
    }
}
