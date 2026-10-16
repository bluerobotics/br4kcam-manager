//! Per-camera reachability derived from slow-watcher HTTP probes and TCP fallback.

use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use br4kcam_api::{CameraConnectivity, ExpectedCamera, McmHealth, SystemHealth};
use once_cell::sync::OnceCell;
use tokio::net::TcpStream;
use tokio::sync::{Notify, broadcast};
// tokio::time::Instant: debounce timestamps in async tasks (pause-aware under #[tokio::test]).
use tokio::time::Instant;
use tracing::*;
use uuid::Uuid;

use crate::web::{camera_state, camera_ui, ws_connections::ConnectionId};

// Per-state debounce: Unreachable↔Unresponsive oscillation needs a fresh 6s hold each way.
const UNHEALTHY_GRACE: Duration = Duration::from_secs(6);
const TCP_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

static CAMERAS: OnceCell<Mutex<HashMap<Uuid, Entry>>> = OnceCell::new();
static HEALTH_TX: OnceCell<broadcast::Sender<()>> = OnceCell::new();
static HEALTH_DIRTY: Notify = Notify::const_new();
static LAST_PUBLISHED_COMPARABLE: Mutex<Option<SystemHealth>> = Mutex::new(None);
static FAN_IN_STARTED: AtomicBool = AtomicBool::new(false);

struct Entry {
    connectivity: CameraConnectivity,
    pending: Option<(CameraConnectivity, Instant)>,
}

struct StitchHealthParams<'a> {
    mcm_state: McmHealth,
    mcm_detail: Option<String>,
    mcm_consecutive_failures: u32,
    cameras_discovered: usize,
    configured: &'a [Uuid],
    autopilot: br4kcam_api::AutopilotHealth,
    autopilot_detail: Option<String>,
    lua_scripting_disabled: bool,
    lua_script: br4kcam_api::LuaScriptStatus,
    lua_script_detail: Option<String>,
    parameter_drifts: Option<Vec<br4kcam_api::ParameterDrift>>,
    diagnostics: br4kcam_api::Diagnostics,
    state_events_lagged: u64,
}

pub(crate) struct ClassifyEvidence {
    pub in_mcm_list: bool,
    pub expected: bool,
    pub camera_answered: bool,
    pub tcp_ok: bool,
    pub probed: bool,
}

impl Entry {
    fn new(connectivity: CameraConnectivity) -> Self {
        Self {
            connectivity,
            pending: None,
        }
    }
}

/// When a configured camera leaves MCM discovery but we can still probe its last
/// IP, a failed TCP path is [`Unreachable`] (cable/power) — not [`Missing`].
/// [`Missing`] is reserved for "configured, absent, and no probe possible yet".
///
/// [`Unresponsive`] requires the camera to be in the MCM list (HTTP attempted and
/// failed while TCP still works). Absent-from-list + TCP-ok is normal while MCM
/// rediscovers ONVIF devices after a restart — keep [`Unknown`] so the UI does
/// not blame cables during that window.
pub(crate) fn classify_table(evidence: ClassifyEvidence) -> CameraConnectivity {
    if evidence.camera_answered {
        return CameraConnectivity::Online;
    }
    if !evidence.probed {
        if evidence.expected && !evidence.in_mcm_list {
            return CameraConnectivity::Missing;
        }
        return CameraConnectivity::Unknown;
    }
    if evidence.tcp_ok {
        if evidence.in_mcm_list {
            return CameraConnectivity::Unresponsive;
        }
        // Host still answers TCP but MCM has not listed it yet (ONVIF rediscovery).
        return CameraConnectivity::Unknown;
    }
    // Probed, no HTTP answer, TCP failed.
    CameraConnectivity::Unreachable
}

/// Remember the last hostname MCM reported for `camera_uuid`.
pub(crate) fn observe_camera(camera_uuid: Uuid, hostname: Ipv4Addr) {
    mcm_client::remember_hostname(camera_uuid, hostname);
    let mut guard = cameras().lock().expect("connectivity lock");
    guard
        .entry(camera_uuid)
        .or_insert_with(|| Entry::new(CameraConnectivity::Unknown));
}

/// Apply per-state debounce and push wire updates through [`camera_ui`].
pub(crate) fn publish(camera_uuid: Uuid, next: CameraConnectivity, reason: &str) {
    // While MCM is down, list membership and HTTP probes are inconclusive — do not
    // latch Unreachable/Unresponsive that would flash as "camera not responding"
    // the moment MCM returns.
    let next = if mcm_client::health().state == McmHealth::Down
        && !matches!(
            next,
            CameraConnectivity::Online | CameraConnectivity::Unknown
        ) {
        CameraConnectivity::Unknown
    } else {
        next
    };

    if camera_ui::get(camera_uuid).rebooting && next != CameraConnectivity::Online {
        let mut guard = cameras().lock().expect("connectivity lock");
        if let Some(entry) = guard.get_mut(&camera_uuid) {
            entry.pending = None;
        }
        return;
    }

    let (from, hostname, should_emit) = {
        let mut guard = cameras().lock().expect("connectivity lock");
        let entry = guard
            .entry(camera_uuid)
            .or_insert_with(|| Entry::new(CameraConnectivity::Unknown));
        let from = entry.connectivity;
        let hostname = mcm_client::cached_hostname(&camera_uuid)
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "?".to_string());

        if from == next {
            entry.pending = None;
            (from, hostname, false)
        } else if next == CameraConnectivity::Online || next == CameraConnectivity::Missing {
            entry.connectivity = next;
            entry.pending = None;
            (from, hostname, true)
        } else if let Some((candidate, first_seen)) = entry.pending
            && candidate == next
            && first_seen.elapsed() >= UNHEALTHY_GRACE
        {
            entry.connectivity = next;
            entry.pending = None;
            (from, hostname, true)
        } else if entry.pending.map(|(candidate, _)| candidate) != Some(next) {
            entry.pending = Some((next, Instant::now()));
            (from, hostname, false)
        } else {
            (from, hostname, false)
        }
    };

    if should_emit {
        if next == CameraConnectivity::Online {
            info!(
                %camera_uuid,
                hostname,
                ?from,
                ?next,
                reason,
                "Camera connectivity recovered"
            );
        } else {
            warn!(
                %camera_uuid,
                hostname,
                ?from,
                ?next,
                reason,
                "Camera connectivity degraded"
            );
        }
        camera_ui::set_connectivity(camera_uuid, next);
        notify_health();
    }
}

/// After MCM recovers, drop probe faults that may have latched while the list was empty.
pub(crate) fn reset_after_mcm_recovery() {
    let changed: Vec<Uuid> = {
        let mut guard = cameras().lock().expect("connectivity lock");
        let mut changed = Vec::new();
        for (camera_uuid, entry) in guard.iter_mut() {
            entry.pending = None;
            if matches!(
                entry.connectivity,
                CameraConnectivity::Unresponsive
                    | CameraConnectivity::Unreachable
                    | CameraConnectivity::Missing
            ) {
                entry.connectivity = CameraConnectivity::Unknown;
                changed.push(*camera_uuid);
            }
        }
        changed
    };

    for camera_uuid in &changed {
        camera_ui::set_connectivity(*camera_uuid, CameraConnectivity::Unknown);
    }
    if !changed.is_empty() {
        notify_health();
    }
}

#[cfg(test)]
fn get(camera_uuid: Uuid) -> CameraConnectivity {
    cameras()
        .lock()
        .expect("connectivity lock")
        .get(&camera_uuid)
        .map(|entry| entry.connectivity)
        .unwrap_or(CameraConnectivity::Unknown)
}

/// Drop connectivity state after the user forgets a configured camera and push health.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
pub(crate) fn forget_camera(camera_uuid: Uuid) {
    cameras()
        .lock()
        .expect("connectivity lock")
        .remove(&camera_uuid);
    camera_ui::set_connectivity(camera_uuid, CameraConnectivity::Unknown);
    notify_health();
}

/// True when `camera_uuid` is configured in persisted actuator settings.
pub(crate) async fn is_expected(camera_uuid: Uuid) -> bool {
    autopilot::configured_cameras().await.contains(&camera_uuid)
}

/// True when a WebSocket may subscribe to `camera_uuid` even if MCM does not list it.
pub(crate) async fn subscribe_allowed(camera_uuid: Uuid, connection_id: ConnectionId) -> bool {
    mcm_client::get_camera(&camera_uuid).await.is_some()
        || is_expected(camera_uuid).await
        || camera_state::has_interest(camera_uuid)
        || camera_state::connection_subscribed(connection_id, camera_uuid)
        || last_hostname(camera_uuid).await.is_some()
}

pub(crate) async fn last_hostname(camera_uuid: Uuid) -> Option<Ipv4Addr> {
    mcm_client::camera_address(&camera_uuid).await
}

fn stitch_system_health(
    params: StitchHealthParams<'_>,
    is_discovered: impl Fn(Uuid) -> bool,
    hostname_of: impl Fn(Uuid) -> Option<String>,
) -> SystemHealth {
    let mut diagnostics = params.diagnostics;
    diagnostics.mcm_consecutive_failures = params.mcm_consecutive_failures;
    diagnostics.state_events_lagged = params.state_events_lagged;

    let expected_missing = params
        .configured
        .iter()
        .filter(|uuid| !is_discovered(**uuid))
        .map(|&uuid| ExpectedCamera {
            uuid,
            last_hostname: hostname_of(uuid),
        })
        .collect();

    SystemHealth {
        mcm: params.mcm_state,
        mcm_detail: (params.mcm_state != McmHealth::Online)
            .then_some(params.mcm_detail)
            .flatten(),
        cameras_discovered: params.cameras_discovered,
        expected_missing,
        autopilot: params.autopilot,
        autopilot_detail: (params.autopilot != br4kcam_api::AutopilotHealth::Online)
            .then_some(params.autopilot_detail)
            .flatten(),
        lua_scripting_disabled: params.lua_scripting_disabled,
        lua_script: params.lua_script,
        lua_script_detail: params.lua_script_detail,
        parameter_drifts: params.parameter_drifts,
        diagnostics,
    }
}

/// Backend-wide health snapshot for WebSocket push and `GET /v1/health`.
#[instrument(level = "debug", skip_all)]
pub(crate) async fn system_health() -> SystemHealth {
    let mcm = mcm_client::health();
    let discovered = mcm_client::cameras().await;
    let configured = autopilot::configured_cameras().await;
    let (autopilot, autopilot_detail) = autopilot::health();
    let (lua_script, lua_script_detail) = autopilot::lua_script_status();
    let parameter_drifts = {
        let drifts: Vec<_> = autopilot::parameter_drifts()
            .into_iter()
            .map(|drift| br4kcam_api::ParameterDrift {
                name: drift.name,
                expected: drift.expected,
                actual: drift.actual,
                adoptable: drift.adoptable,
            })
            .collect();
        (!drifts.is_empty()).then_some(drifts)
    };

    let mut hostnames = HashMap::new();
    for uuid in &configured {
        if let Some(hostname) = mcm_client::camera_address(uuid).await {
            hostnames.insert(*uuid, hostname.to_string());
        }
    }

    stitch_system_health(
        StitchHealthParams {
            mcm_state: mcm.state,
            mcm_detail: mcm.detail,
            mcm_consecutive_failures: mcm.consecutive_failures,
            cameras_discovered: discovered.len(),
            configured: &configured,
            autopilot,
            autopilot_detail,
            lua_scripting_disabled: autopilot::lua_scripting_disabled(),
            lua_script,
            lua_script_detail,
            parameter_drifts,
            diagnostics: autopilot::diagnostics(),
            state_events_lagged: camera_state::state_events_lagged(),
        },
        |uuid| discovered.contains_key(&uuid),
        |uuid| hostnames.get(&uuid).cloned(),
    )
}

/// Health with the fields that move on every sample blanked, so equality means
/// "nothing a client would render has changed".
pub(crate) fn health_comparable(mut health: SystemHealth) -> SystemHealth {
    health.diagnostics.last_frame_age_ms = None;
    health.diagnostics.last_heartbeat_age_ms = None;
    health.diagnostics.last_servo_age_ms = None;
    health.diagnostics.mcm_consecutive_failures = 0;
    health
}

/// Broadcast a health notification only when the non-volatile fields moved since
/// the last publish. Every subscriber then pulls the full current snapshot, so
/// this dedupe is global and cannot starve a second connection.
#[instrument(level = "debug", skip_all)]
pub(crate) async fn publish_health_if_changed() {
    let comparable = health_comparable(system_health().await);
    {
        let mut last = LAST_PUBLISHED_COMPARABLE
            .lock()
            .expect("health compare lock");
        if last.as_ref() == Some(&comparable) {
            return;
        }
        *last = Some(comparable);
    }
    if let Err(error) = health_sender().send(()) {
        debug!("No system health subscribers: {error}");
    }
}

/// Subscribe to backend health change notifications (MCM, autopilot, camera list, connectivity).
pub(crate) fn subscribe() -> broadcast::Receiver<()> {
    ensure_fan_in();
    health_sender().subscribe()
}

/// Drop connectivity entries for cameras no longer listed, subscribed, or expected.
pub(crate) fn retain(keep: &HashSet<Uuid>) {
    cameras()
        .lock()
        .expect("connectivity lock")
        .retain(|camera_uuid, _| keep.contains(camera_uuid));
}

#[instrument(level = "debug", skip_all, fields(%hostname, %port))]
pub(crate) async fn tcp_probe(hostname: Ipv4Addr, port: u16) -> bool {
    let address = SocketAddr::from((hostname, port));
    tokio::time::timeout(TCP_PROBE_TIMEOUT, TcpStream::connect(address))
        .await
        .is_ok_and(|result| result.is_ok())
}

fn cameras() -> &'static Mutex<HashMap<Uuid, Entry>> {
    CAMERAS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn health_sender() -> &'static broadcast::Sender<()> {
    HEALTH_TX.get_or_init(|| {
        let (sender, _) = broadcast::channel(16);
        sender
    })
}

/// Ask the fan-in task to re-evaluate health; it publishes only on a real change.
fn notify_health() {
    HEALTH_DIRTY.notify_one();
}

/// True when any camera has a debounced connectivity transition awaiting a second sample.
pub(crate) fn has_pending_debounce() -> bool {
    cameras()
        .lock()
        .expect("connectivity lock")
        .values()
        .any(|entry| entry.pending.is_some())
}

async fn sync_mcm_camera_errors() {
    let discovered = mcm_client::cameras().await;
    let configured = autopilot::configured_cameras().await;
    let uuids: HashSet<Uuid> = discovered
        .keys()
        .chain(configured.iter())
        .copied()
        .collect();
    for uuid in uuids {
        let stream_error = mcm_client::stream_failure(&uuid).await;
        camera_ui::set_stream_error(uuid, stream_error);
        let onvif_auth_error = mcm_client::authentication_failure(&uuid).await;
        camera_ui::set_onvif_auth_error(uuid, onvif_auth_error);
    }
}

fn ensure_fan_in() {
    if FAN_IN_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(async {
        let mut mcm_rx = mcm_client::subscribe_health();
        let mut autopilot_rx = autopilot::subscribe_health();
        let mut cameras_rx = mcm_client::subscribe_cameras();

        loop {
            tokio::select! {
                _ = HEALTH_DIRTY.notified() => {}
                _ = mcm_rx.recv() => {
                    if mcm_client::health().state == McmHealth::Online {
                        reset_after_mcm_recovery();
                    }
                }
                _ = autopilot_rx.recv() => {}
                _ = cameras_rx.recv() => {
                    sync_mcm_camera_errors().await;
                }
            }
            publish_health_if_changed().await;
        }
    });
}

#[cfg(test)]
fn connectivity_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use br4kcam_api::{AutopilotHealth, Diagnostics};

    use super::*;

    // Process-lifetime OnceCell globals are not reset between tests; use unique
    // UUIDs, remove entries in cleanup, and take connectivity_test_lock() when
    // mutating shared state (see S-T6).

    #[test]
    fn stitch_system_health_expected_missing_and_detail_suppression() {
        let configured = vec![
            Uuid::from_u128(0xa501_0000_0000_0001),
            Uuid::from_u128(0xa501_0000_0000_0002),
        ];

        let health = stitch_system_health(
            StitchHealthParams {
                mcm_state: McmHealth::Online,
                mcm_detail: Some("suppressed".into()),
                mcm_consecutive_failures: 0,
                cameras_discovered: 1,
                configured: &configured,
                autopilot: AutopilotHealth::Online,
                autopilot_detail: Some("suppressed".into()),
                lua_scripting_disabled: false,
                lua_script: br4kcam_api::LuaScriptStatus::Ok,
                lua_script_detail: None,
                parameter_drifts: Some(vec![br4kcam_api::ParameterDrift {
                    name: "SERVO1_FUNCTION".into(),
                    expected: 33.0,
                    actual: 0.0,
                    adoptable: false,
                }]),
                diagnostics: Diagnostics::default(),
                state_events_lagged: 0,
            },
            |uuid| uuid == Uuid::from_u128(0xa501_0000_0000_0001),
            |uuid| (uuid == Uuid::from_u128(0xa501_0000_0000_0002)).then_some("10.0.0.2".into()),
        );

        assert_eq!(health.cameras_discovered, 1);
        assert!(health.mcm_detail.is_none());
        assert!(health.autopilot_detail.is_none());
        assert_eq!(health.expected_missing.len(), 1);
        assert_eq!(
            health.expected_missing[0].uuid,
            Uuid::from_u128(0xa501_0000_0000_0002)
        );
        assert_eq!(
            health.expected_missing[0].last_hostname.as_deref(),
            Some("10.0.0.2")
        );
        assert_eq!(health.parameter_drifts.as_ref().unwrap().len(), 1);
        assert_eq!(
            health.parameter_drifts.as_ref().unwrap()[0].name,
            "SERVO1_FUNCTION"
        );
        assert_eq!(health.parameter_drifts.as_ref().unwrap()[0].expected, 33.0);
        assert_eq!(health.parameter_drifts.as_ref().unwrap()[0].actual, 0.0);

        let degraded = stitch_system_health(
            StitchHealthParams {
                mcm_state: McmHealth::Down,
                mcm_detail: Some("mcm down".into()),
                mcm_consecutive_failures: 3,
                cameras_discovered: 0,
                configured: &[],
                autopilot: AutopilotHealth::MavlinkDown,
                autopilot_detail: Some("no frames".into()),
                lua_scripting_disabled: false,
                lua_script: br4kcam_api::LuaScriptStatus::Ok,
                lua_script_detail: None,
                parameter_drifts: None,
                diagnostics: Diagnostics::default(),
                state_events_lagged: 0,
            },
            |_| false,
            |_| None,
        );
        assert_eq!(degraded.mcm_detail.as_deref(), Some("mcm down"));
        assert_eq!(degraded.autopilot_detail.as_deref(), Some("no frames"));
    }

    #[test]
    fn health_comparable_ignores_volatile_diagnostics() {
        fn sample_health(mcm: McmHealth, failures: u32) -> SystemHealth {
            SystemHealth {
                mcm,
                diagnostics: Diagnostics {
                    mcm_consecutive_failures: failures,
                    last_frame_age_ms: Some(100),
                    last_heartbeat_age_ms: Some(200),
                    last_servo_age_ms: Some(300),
                    ..Default::default()
                },
                ..Default::default()
            }
        }

        let a = sample_health(McmHealth::Down, 5);
        let b = sample_health(McmHealth::Down, 99);
        let mut c = sample_health(McmHealth::Down, 5);
        c.diagnostics.last_frame_age_ms = Some(999);
        c.diagnostics.last_heartbeat_age_ms = Some(888);
        c.diagnostics.last_servo_age_ms = Some(777);

        assert_eq!(health_comparable(a.clone()), health_comparable(b));
        assert_eq!(health_comparable(a), health_comparable(c));
        assert_ne!(
            health_comparable(sample_health(McmHealth::Online, 0)),
            health_comparable(sample_health(McmHealth::Down, 0))
        );
    }

    #[test]
    fn retain_preserves_subscribed_but_undiscovered() {
        let _lock = connectivity_test_lock();
        let camera_uuid = Uuid::from_u128(0xa504_0000_0000_0001);
        publish(camera_uuid, CameraConnectivity::Missing, "test missing");
        assert_eq!(get(camera_uuid), CameraConnectivity::Missing);

        let mut keep = HashSet::new();
        keep.insert(camera_uuid);
        retain(&keep);
        assert_eq!(get(camera_uuid), CameraConnectivity::Missing);

        retain(&HashSet::new());
        assert_eq!(get(camera_uuid), CameraConnectivity::Unknown);
        cameras().lock().unwrap().remove(&camera_uuid);
    }

    #[test]
    fn classifier_table() {
        fn missing(tcp_ok: bool, probed: bool) -> ClassifyEvidence {
            ClassifyEvidence {
                in_mcm_list: false,
                expected: true,
                camera_answered: false,
                tcp_ok,
                probed,
            }
        }

        // Configured + absent + no probe yet → Missing (Forget path).
        assert_eq!(
            classify_table(missing(false, false)),
            CameraConnectivity::Missing
        );
        // Configured + absent + TCP failed → Unreachable (unplugged cable).
        assert_eq!(
            classify_table(missing(false, true)),
            CameraConnectivity::Unreachable
        );
        assert_eq!(
            classify_table(ClassifyEvidence {
                in_mcm_list: true,
                camera_answered: true,
                probed: true,
                expected: false,
                tcp_ok: false,
            }),
            CameraConnectivity::Online
        );
        assert_eq!(
            classify_table(ClassifyEvidence {
                in_mcm_list: true,
                tcp_ok: true,
                probed: true,
                expected: false,
                camera_answered: false,
            }),
            CameraConnectivity::Unresponsive
        );
        // Absent from MCM + TCP ok: normal ONVIF rediscovery — do not alarm.
        assert_eq!(
            classify_table(missing(true, true)),
            CameraConnectivity::Unknown
        );
        assert_eq!(
            classify_table(ClassifyEvidence {
                tcp_ok: true,
                probed: true,
                in_mcm_list: false,
                expected: false,
                camera_answered: false,
            }),
            CameraConnectivity::Unknown
        );
        assert_eq!(
            classify_table(ClassifyEvidence {
                in_mcm_list: true,
                probed: true,
                expected: false,
                camera_answered: false,
                tcp_ok: false,
            }),
            CameraConnectivity::Unreachable
        );
        assert_eq!(
            classify_table(ClassifyEvidence {
                in_mcm_list: true,
                expected: false,
                camera_answered: false,
                tcp_ok: false,
                probed: false,
            }),
            CameraConnectivity::Unknown
        );
    }

    #[allow(clippy::await_holding_lock)] // test lock must span paused clock advances to serialize globals
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn debounce_is_per_state() {
        let _lock = connectivity_test_lock();
        let camera_uuid = Uuid::from_u128(0xdeb0_0000_0000_0001);
        publish(
            camera_uuid,
            CameraConnectivity::Unreachable,
            "test unreachable",
        );
        assert_eq!(get(camera_uuid), CameraConnectivity::Unknown);

        tokio::time::advance(UNHEALTHY_GRACE).await;
        publish(
            camera_uuid,
            CameraConnectivity::Unreachable,
            "test unreachable",
        );
        assert_eq!(get(camera_uuid), CameraConnectivity::Unreachable);

        publish(
            camera_uuid,
            CameraConnectivity::Unresponsive,
            "test unresponsive",
        );
        assert_eq!(get(camera_uuid), CameraConnectivity::Unreachable);

        tokio::time::advance(UNHEALTHY_GRACE).await;
        publish(
            camera_uuid,
            CameraConnectivity::Unresponsive,
            "test unresponsive",
        );
        assert_eq!(get(camera_uuid), CameraConnectivity::Unresponsive);

        publish(camera_uuid, CameraConnectivity::Online, "test online");
        assert_eq!(get(camera_uuid), CameraConnectivity::Online);

        cameras().lock().unwrap().remove(&camera_uuid);
    }
}
