//! Lock order: `camera_ui::UI` is always taken before [`REGISTRY`]. Never hold the
//! `REGISTRY` guard across a call into [`camera_ui`], or the two orders can deadlock.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use br4kcam_api::{CameraStateEvent, CameraUiState};
use br4kcam_commands::{
    Action as CameraAction, CameraControl,
    protocol::video::video_parameters::{VideoChannelValue, VideoParameterSettings},
};
use futures::stream::{self, StreamExt};
use once_cell::sync::OnceCell;
use tokio::sync::broadcast;
use tracing::*;
use uuid::Uuid;

use autopilot::api::{Action as AutopilotAction, ActuatorsControl};

use crate::web::camera_ui;
use crate::web::connectivity;
use crate::web::ws_connections::ConnectionId;

const SLOW_WATCH_INTERVAL: Duration = Duration::from_secs(2);
// Half of connectivity's 6s unhealthy grace: second tick sample commits debounce within one period.
const CAMERA_LIST_WATCHER_TICK: Duration = Duration::from_secs(3);
const CAMERA_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap concurrent slow HTTP fetches so a flood of subscriptions cannot fan out unbounded.
const SLOW_FETCH_CONCURRENCY: usize = 4;
/// Cap distinct cameras one WebSocket may subscribe to.
const MAX_CAMERAS_PER_CONNECTION: usize = 8;

static ACTUATORS_BRIDGE_STARTED: AtomicBool = AtomicBool::new(false);
static SLOW_WATCHER_STARTED: AtomicBool = AtomicBool::new(false);
static CAMERA_LIST_WATCHER_STARTED: AtomicBool = AtomicBool::new(false);
static ACTUATORS_LAG_RESYNCING: AtomicBool = AtomicBool::new(false);
static ACTUATORS_LAG_NEEDS_RESYNC: AtomicBool = AtomicBool::new(false);
static REGISTRY: OnceCell<Mutex<Registry>> = OnceCell::new();
static STATE_TX: OnceCell<broadcast::Sender<CameraStateEvent>> = OnceCell::new();
static ACTUATORS_DEFAULT_CONFIG: tokio::sync::OnceCell<serde_json::Value> =
    tokio::sync::OnceCell::const_new();
static STATE_EVENTS_LAGGED: AtomicU64 = AtomicU64::new(0);

struct Registry {
    connections: HashMap<ConnectionId, HashSet<Uuid>>,
    camera_interest: HashMap<Uuid, usize>,
    last_states: HashMap<Uuid, CameraSnapshot>,
}

/// Clears the actuators lag-resync flag when the task ends or panics.
///
/// Call [`ActuatorsLagResyncGuard::disarm`] before a clean exit that already
/// cleared `ACTUATORS_LAG_RESYNCING`, so Drop does not clobber a successor task.
struct ActuatorsLagResyncGuard {
    armed: bool,
}

impl ActuatorsLagResyncGuard {
    fn new() -> Self {
        Self { armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct CameraSnapshot {
    actuators_config: Option<serde_json::Value>,
    actuators_state: Option<serde_json::Value>,
    actuators_configured: Option<bool>,
    video_parameters: Option<serde_json::Value>,
    base_parameters: Option<serde_json::Value>,
    advanced_parameters: Option<serde_json::Value>,
}

impl Drop for ActuatorsLagResyncGuard {
    fn drop(&mut self) {
        if self.armed {
            ACTUATORS_LAG_RESYNCING.store(false, Ordering::SeqCst);
        }
    }
}

/// Subscribe to camera state broadcast events.
#[instrument(level = "debug")]
pub(crate) fn subscribe_state() -> broadcast::Receiver<CameraStateEvent> {
    state_sender().subscribe()
}

/// `camera/state` broadcast events dropped because a client lagged behind the broadcast.
pub(crate) fn state_events_lagged() -> u64 {
    STATE_EVENTS_LAGGED.load(Ordering::Relaxed)
}

/// Count `camera/state` events dropped by a lagging subscriber.
pub(crate) fn record_state_events_lagged(samples: u64) {
    STATE_EVENTS_LAGGED.fetch_add(samples, Ordering::Relaxed);
}

/// Emit a UI-only state update for `camera_uuid`.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
pub(crate) fn emit_ui(camera_uuid: Uuid, ui: CameraUiState) {
    emit(CameraStateEvent {
        camera_uuid,
        ui: Some(ui),
        ..Default::default()
    });
}

/// Broadcast without writing `event` into the cache.
///
/// Use after [`commit_fetched_snapshot`]: the cache is already authoritative, and
/// re-recording would clobber concurrent `record_partial` updates.
#[instrument(level = "debug", skip_all, fields(%event.camera_uuid))]
fn broadcast_event(event: CameraStateEvent) {
    if !has_interest(event.camera_uuid) {
        return;
    }
    if let Err(error) = state_sender().send(event) {
        debug!("No camera state subscribers: {error}");
    }
}

/// Returns true when `connection_id` is subscribed to `camera_uuid`.
pub(crate) fn connection_subscribed(connection_id: ConnectionId, camera_uuid: Uuid) -> bool {
    registry()
        .lock()
        .unwrap()
        .connections
        .get(&connection_id)
        .is_some_and(|cameras| cameras.contains(&camera_uuid))
}

/// Cameras currently subscribed by `connection_id`.
pub(crate) fn connection_cameras(connection_id: ConnectionId) -> Vec<Uuid> {
    registry()
        .lock()
        .unwrap()
        .connections
        .get(&connection_id)
        .map(|cameras| cameras.iter().copied().collect())
        .unwrap_or_default()
}

/// Idempotently subscribe `connection_id` to `camera_uuid` state updates.
///
/// Returns true when the connection is subscribed after this call (including when
/// it already was — callers re-push UI/snapshot for late remounts). Returns false
/// when rejected (e.g. at the per-connection camera cap).
#[instrument(level = "debug")]
pub(crate) fn subscribe(connection_id: ConnectionId, camera_uuid: Uuid) -> bool {
    {
        let mut registry = registry().lock().unwrap();
        let cameras = registry.connections.entry(connection_id).or_default();
        if cameras.contains(&camera_uuid) {
            return true;
        }
        if cameras.len() >= MAX_CAMERAS_PER_CONNECTION {
            warn!(
                %connection_id,
                %camera_uuid,
                limit = MAX_CAMERAS_PER_CONNECTION,
                "Rejecting subscribe; connection is at the camera cap"
            );
            return false;
        }
        cameras.insert(camera_uuid);

        let was_empty = registry.camera_interest.is_empty();
        *registry.camera_interest.entry(camera_uuid).or_insert(0) += 1;
        // Keep the autopilot interest handoff under the same lock as the registry so a
        // concurrent last-unsubscribe cannot disable SERVO after we just enabled it.
        if was_empty {
            autopilot::add_actuators_state_interest();
        }
    }

    ensure_actuators_bridge();
    ensure_slow_watcher_started();
    ensure_camera_list_watcher();

    true
}

/// Idempotently unsubscribe `connection_id` from `camera_uuid`.
#[instrument(level = "debug")]
pub(crate) fn unsubscribe(connection_id: ConnectionId, camera_uuid: Uuid) {
    let mut registry = registry().lock().unwrap();
    let Some(cameras) = registry.connections.get_mut(&connection_id) else {
        return;
    };
    if !cameras.remove(&camera_uuid) {
        return;
    }
    if cameras.is_empty() {
        registry.connections.remove(&connection_id);
    }

    let had_interest = !registry.camera_interest.is_empty();
    release_interest(&mut registry, camera_uuid);
    if had_interest && registry.camera_interest.is_empty() {
        autopilot::remove_actuators_state_interest();
    }
}

/// Remove every camera subscription owned by `connection_id`.
#[instrument(level = "debug")]
pub(crate) fn unsubscribe_connection(connection_id: ConnectionId) {
    let mut registry = registry().lock().unwrap();
    let Some(cameras) = registry.connections.remove(&connection_id) else {
        return;
    };
    let had_interest = !registry.camera_interest.is_empty();

    for camera_uuid in cameras {
        release_interest(&mut registry, camera_uuid);
    }

    if had_interest && registry.camera_interest.is_empty() {
        autopilot::remove_actuators_state_interest();
    }
}

/// True when any WebSocket connection is interested in `camera_uuid`.
pub(crate) fn has_interest(camera_uuid: Uuid) -> bool {
    registry()
        .lock()
        .unwrap()
        .camera_interest
        .contains_key(&camera_uuid)
}

/// Last known state for `camera_uuid`, without hitting the camera.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
pub(crate) fn cached_state_event(camera_uuid: Uuid) -> CameraStateEvent {
    snapshot_to_event(
        camera_uuid,
        last_state(camera_uuid),
        ACTUATORS_DEFAULT_CONFIG.get().cloned(),
    )
}

/// Actuators-only event from a fresh GetActuatorsState (for lag / permit-miss recovery).
///
/// Uses the same cache-vs-one-shot path as REST so defaults are not stamped into
/// `last_states` when the SERVO cache is stale for this camera.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
pub(crate) async fn actuators_state_event(camera_uuid: Uuid) -> Option<CameraStateEvent> {
    let actuators_state = match autopilot::handle_control(ActuatorsControl {
        camera_uuid,
        action: AutopilotAction::GetActuatorsState,
    })
    .await
    {
        Ok(value) => value,
        Err(error) => {
            warn!(%camera_uuid, "GetActuatorsState for actuators_state_event failed: {error:?}");
            return None;
        }
    };
    let mut event = CameraStateEvent {
        camera_uuid,
        actuators_state: Some(actuators_state),
        ..Default::default()
    };
    record_partial(&mut event);
    Some(event)
}

/// Fetch a full camera snapshot for initial subscribe / lag recovery.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
pub(crate) async fn snapshot(camera_uuid: Uuid) -> CameraStateEvent {
    let baseline = last_state(camera_uuid);
    let fetched = fetch_snapshot(camera_uuid, &baseline).await;
    let snapshot =
        commit_fetched_snapshot(camera_uuid, &baseline, fetched.clone()).unwrap_or(fetched);

    snapshot_to_event(camera_uuid, snapshot, actuators_default_config().await)
}

/// Push camera-control side effects into the shared state stream.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
pub(crate) fn emit_camera_control_update(
    camera_uuid: Uuid,
    action: &CameraAction,
    result: &serde_json::Value,
) {
    let mut event = CameraStateEvent {
        camera_uuid,
        ..Default::default()
    };

    match action {
        CameraAction::SetImageAdjustment(_) => event.base_parameters = Some(result.clone()),
        CameraAction::SetImageAdjustmentEx(_) => event.advanced_parameters = Some(result.clone()),
        CameraAction::SetVideoParameterSettings(settings) => {
            // Cache only MainStream; other channels share the same event field.
            if !matches!(settings.channel, None | Some(VideoChannelValue::MainStream)) {
                return;
            }
            event.video_parameters = Some(result.clone());
        }
        CameraAction::SetRecommendedCameraSettings | CameraAction::Restart => {
            tokio::spawn(reconcile_snapshot(camera_uuid).instrument(Span::current()));
            return;
        }
        CameraAction::SetImageAdjustmentExAll(_) => {
            // Applies to every camera; refresh all that currently have subscribers.
            let cameras: Vec<Uuid> = {
                let registry = registry().lock().unwrap();
                registry.camera_interest.keys().copied().collect()
            };
            let span = Span::current();
            for uuid in cameras {
                tokio::spawn(reconcile_snapshot(uuid).instrument(span.clone()));
            }
            return;
        }
        _ => return,
    }

    emit(event);
}

/// Push autopilot-control side effects into the shared state stream.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
pub(crate) fn emit_autopilot_control_update(
    camera_uuid: Uuid,
    action: &AutopilotAction,
    result: &serde_json::Value,
) {
    let mut event = CameraStateEvent {
        camera_uuid,
        ..Default::default()
    };

    match action {
        AutopilotAction::SetActuatorsState(_) => event.actuators_state = Some(result.clone()),
        AutopilotAction::SetActuatorsConfig(_)
        | AutopilotAction::ResetActuatorsConfig
        | AutopilotAction::ForceResetActuatorsConfig => {
            event.actuators_config = Some(result.clone());
            event.actuators_configured = Some(true);
        }
        AutopilotAction::ForgetActuatorsConfig => {
            event.actuators_configured = Some(false);
        }
        _ => return,
    }

    emit(event);
}

/// Re-fetch and emit changed camera fields after disruptive actions.
///
/// Does not refresh `actuators_state` (SERVO bridge owns that) and broadcasts
/// without re-recording so concurrent partial updates are not clobbered.
#[instrument(level = "debug")]
pub(crate) async fn reconcile_snapshot(camera_uuid: Uuid) {
    if !has_interest(camera_uuid) {
        return;
    }

    let baseline = last_state(camera_uuid);
    let fetched = fetch_slow_snapshot(camera_uuid, &baseline).await;
    if fetched == baseline {
        return;
    }

    let Some(merged) = commit_fetched_snapshot(camera_uuid, &baseline, fetched) else {
        return;
    };
    if merged == baseline {
        return;
    }

    let event = CameraStateEvent {
        camera_uuid,
        actuators_config: (merged.actuators_config != baseline.actuators_config)
            .then(|| merged.actuators_config.clone())
            .flatten(),
        actuators_configured: (merged.actuators_configured != baseline.actuators_configured)
            .then_some(merged.actuators_configured)
            .flatten(),
        video_parameters: (merged.video_parameters != baseline.video_parameters)
            .then(|| merged.video_parameters.clone())
            .flatten(),
        base_parameters: (merged.base_parameters != baseline.base_parameters)
            .then(|| merged.base_parameters.clone())
            .flatten(),
        advanced_parameters: (merged.advanced_parameters != baseline.advanced_parameters)
            .then(|| merged.advanced_parameters.clone())
            .flatten(),
        ..Default::default()
    };

    if event.actuators_config.is_none()
        && event.actuators_configured.is_none()
        && event.video_parameters.is_none()
        && event.base_parameters.is_none()
        && event.advanced_parameters.is_none()
    {
        return;
    }

    broadcast_event(event);
}

/// Apply `fetched` into the cache for fields that were not updated concurrently since
/// `baseline` was read. Returns the merged snapshot, or `None` when nobody is interested.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
fn commit_fetched_snapshot(
    camera_uuid: Uuid,
    baseline: &CameraSnapshot,
    fetched: CameraSnapshot,
) -> Option<CameraSnapshot> {
    let mut registry = registry().lock().unwrap();
    if !registry.camera_interest.contains_key(&camera_uuid) {
        return None;
    }

    let current = registry.last_states.entry(camera_uuid).or_default();
    take_if_unchanged(
        &mut current.actuators_config,
        &baseline.actuators_config,
        fetched.actuators_config,
    );
    take_if_unchanged(
        &mut current.actuators_state,
        &baseline.actuators_state,
        fetched.actuators_state,
    );
    take_if_unchanged(
        &mut current.actuators_configured,
        &baseline.actuators_configured,
        fetched.actuators_configured,
    );
    take_if_unchanged(
        &mut current.video_parameters,
        &baseline.video_parameters,
        fetched.video_parameters,
    );
    take_if_unchanged(
        &mut current.base_parameters,
        &baseline.base_parameters,
        fetched.base_parameters,
    );
    take_if_unchanged(
        &mut current.advanced_parameters,
        &baseline.advanced_parameters,
        fetched.advanced_parameters,
    );

    Some(current.clone())
}

fn take_if_unchanged<T: Clone + PartialEq>(
    current: &mut Option<T>,
    baseline: &Option<T>,
    fetched: Option<T>,
) {
    if current == baseline {
        *current = fetched;
    }
}

/// Last cached snapshot for `camera_uuid`, used as the fallback for failed fetches.
fn last_state(camera_uuid: Uuid) -> CameraSnapshot {
    registry()
        .lock()
        .unwrap()
        .last_states
        .get(&camera_uuid)
        .cloned()
        .unwrap_or_default()
}

/// Drop one interest count for `camera_uuid`, forgetting its cached state when it reaches zero.
fn release_interest(registry: &mut Registry, camera_uuid: Uuid) {
    let Some(count) = registry.camera_interest.get_mut(&camera_uuid) else {
        return;
    };
    *count = count.saturating_sub(1);
    if *count == 0 {
        registry.camera_interest.remove(&camera_uuid);
        registry.last_states.remove(&camera_uuid);
    }
}

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            connections: HashMap::new(),
            camera_interest: HashMap::new(),
            last_states: HashMap::new(),
        })
    })
}

fn state_sender() -> &'static broadcast::Sender<CameraStateEvent> {
    STATE_TX.get_or_init(|| {
        let (sender, _) = broadcast::channel(256);
        sender
    })
}

#[instrument(level = "debug", skip_all, fields(%event.camera_uuid))]
pub(crate) fn emit(mut event: CameraStateEvent) {
    record_partial(&mut event);
    // record_partial is a no-op without interest; skip waking every WS filter.
    if !has_interest(event.camera_uuid) {
        return;
    }
    if let Err(error) = state_sender().send(event) {
        debug!("No camera state subscribers: {error}");
    }
}

/// Merges `event` into the last-known snapshot, unless nobody is interested in that camera.
#[instrument(level = "debug", skip_all, fields(%event.camera_uuid))]
fn record_partial(event: &mut CameraStateEvent) {
    let mut registry = registry().lock().unwrap();
    if !registry.camera_interest.contains_key(&event.camera_uuid) {
        return;
    }
    let snapshot = registry.last_states.entry(event.camera_uuid).or_default();

    if let Some(value) = event.actuators_config.take() {
        snapshot.actuators_config = Some(value.clone());
        event.actuators_config = Some(value);
    }
    if let Some(value) = event.actuators_state.take() {
        snapshot.actuators_state = Some(value.clone());
        event.actuators_state = Some(value);
    }
    if let Some(value) = event.actuators_configured.take() {
        snapshot.actuators_configured = Some(value);
        event.actuators_configured = Some(value);
    }
    if let Some(value) = event.video_parameters.take() {
        snapshot.video_parameters = Some(value.clone());
        event.video_parameters = Some(value);
    }
    if let Some(value) = event.base_parameters.take() {
        snapshot.base_parameters = Some(value.clone());
        event.base_parameters = Some(value);
    }
    if let Some(value) = event.advanced_parameters.take() {
        snapshot.advanced_parameters = Some(value.clone());
        event.advanced_parameters = Some(value);
    }
}

#[instrument(level = "debug", skip_all)]
fn ensure_actuators_bridge() {
    if ACTUATORS_BRIDGE_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(async {
        let mut receiver = autopilot::subscribe_actuators_state();

        loop {
            match receiver.recv().await {
                Ok(update) => {
                    let subscribed = {
                        let registry = registry().lock().unwrap();
                        registry.camera_interest.contains_key(&update.camera_uuid)
                    };

                    if !subscribed {
                        continue;
                    }

                    let actuators_state = match serde_json::to_value(update.state) {
                        Ok(value) => value,
                        Err(error) => {
                            warn!("Failed serializing actuators state: {error}");
                            continue;
                        }
                    };

                    emit(CameraStateEvent {
                        camera_uuid: update.camera_uuid,
                        actuators_state: Some(actuators_state),
                        ..Default::default()
                    });
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // Sender is a process-lifetime OnceCell; Closed is unexpected.
                    // Exit rather than busy-resubscribe and spin a core.
                    warn!("Actuators state broadcast closed; stopping actuators bridge");
                    break;
                }
                Err(broadcast::error::RecvError::Lagged(samples)) => {
                    record_state_events_lagged(samples);
                    debug!("Actuators bridge lagged by {samples} updates; re-pushing from manager");
                    if ACTUATORS_LAG_RESYNCING
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_err()
                    {
                        ACTUATORS_LAG_NEEDS_RESYNC.store(true, Ordering::SeqCst);
                        continue;
                    }
                    tokio::spawn(async move {
                        let mut guard = ActuatorsLagResyncGuard::new();
                        loop {
                            ACTUATORS_LAG_NEEDS_RESYNC.store(false, Ordering::SeqCst);
                            let cameras: Vec<Uuid> = {
                                let registry = registry().lock().unwrap();
                                registry.camera_interest.keys().copied().collect()
                            };
                            resync_actuators_from_manager(cameras).await;
                            if ACTUATORS_LAG_NEEDS_RESYNC.load(Ordering::SeqCst) {
                                continue;
                            }
                            // Release before the final dirty check so a concurrent
                            // Lagged can either spawn a successor or set dirty.
                            ACTUATORS_LAG_RESYNCING.store(false, Ordering::SeqCst);
                            if !ACTUATORS_LAG_NEEDS_RESYNC.swap(false, Ordering::SeqCst) {
                                guard.disarm();
                                break;
                            }
                            if ACTUATORS_LAG_RESYNCING
                                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                                .is_ok()
                            {
                                continue;
                            }
                            // Successor already running; hand them the dirty bit.
                            ACTUATORS_LAG_NEEDS_RESYNC.store(true, Ordering::SeqCst);
                            guard.disarm();
                            break;
                        }
                    });
                }
            }
        }
    });
}

#[instrument(level = "debug", skip_all)]
fn ensure_slow_watcher_started() {
    if SLOW_WATCHER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(slow_state_watcher());
}

/// Spawns a background watcher on the first call that reacts to MCM camera-list changes.
///
/// Probes configured cameras missing from the discovery list, publishes connectivity
/// updates, and trims stale per-camera state when a camera is no longer discovered,
/// subscribed, or expected.
///
/// Subscriptions, interest counts, and cached snapshots are kept until the last
/// client unsubscribes via [`unsubscribe`] / [`unsubscribe_connection`].
#[instrument(level = "debug", skip_all)]
fn ensure_camera_list_watcher() {
    if CAMERA_LIST_WATCHER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(async {
        let mut receiver = mcm_client::subscribe_cameras();
        let mut interval = tokio::time::interval(CAMERA_LIST_WATCHER_TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = receiver.recv() => {
                    refresh_cameras_missing_from_mcm().await;
                }
                _ = interval.tick() => {
                    if connectivity::has_pending_debounce() {
                        refresh_cameras_missing_from_mcm().await;
                    }
                }
            }
        }
    });
}

#[instrument(level = "debug", skip_all)]
async fn slow_state_watcher() {
    let mut interval = tokio::time::interval(SLOW_WATCH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;

        let cameras: Vec<(Uuid, CameraSnapshot)> = {
            let registry = registry().lock().unwrap();
            registry
                .camera_interest
                .keys()
                .map(|camera_uuid| {
                    let previous = registry
                        .last_states
                        .get(camera_uuid)
                        .cloned()
                        .unwrap_or_default();
                    (*camera_uuid, previous)
                })
                .collect()
        };

        let results = stream::iter(cameras.into_iter())
            .map(|(camera_uuid, previous)| async move {
                let fetched = fetch_slow_snapshot(camera_uuid, &previous).await;
                (camera_uuid, previous, fetched)
            })
            .buffer_unordered(SLOW_FETCH_CONCURRENCY);

        // Emit each camera as soon as its fetch completes so one slow camera
        // cannot stall updates for the rest of the fleet.
        tokio::pin!(results);
        while let Some((camera_uuid, previous, fetched)) = results.next().await {
            let Some(merged) = commit_fetched_snapshot(camera_uuid, &previous, fetched) else {
                continue;
            };
            if merged == previous {
                continue;
            }

            // Emit only fields that differ from the pre-fetch baseline, using the merged
            // cache values so concurrent record_partial updates are what clients see.
            // Broadcast without re-recording: commit already wrote the cache.
            let event = CameraStateEvent {
                camera_uuid,
                actuators_config: (merged.actuators_config != previous.actuators_config)
                    .then(|| merged.actuators_config.clone())
                    .flatten(),
                actuators_configured: (merged.actuators_configured
                    != previous.actuators_configured)
                    .then_some(merged.actuators_configured)
                    .flatten(),
                video_parameters: (merged.video_parameters != previous.video_parameters)
                    .then(|| merged.video_parameters.clone())
                    .flatten(),
                base_parameters: (merged.base_parameters != previous.base_parameters)
                    .then(|| merged.base_parameters.clone())
                    .flatten(),
                advanced_parameters: (merged.advanced_parameters != previous.advanced_parameters)
                    .then(|| merged.advanced_parameters.clone())
                    .flatten(),
                ..Default::default()
            };

            // actuators_state is driven by the SERVO bridge; a mid-fetch change alone
            // must not push an empty camera/state event.
            if event.actuators_config.is_none()
                && event.actuators_configured.is_none()
                && event.video_parameters.is_none()
                && event.base_parameters.is_none()
                && event.advanced_parameters.is_none()
            {
                continue;
            }

            broadcast_event(event);
        }
    }
}

/// Re-push manager-cached actuators after the SERVO broadcast lagged.
///
/// Uses the autopilot manager (authoritative after the watcher wrote it), not
/// this module's possibly-stale `last_states`, so EmitGate-deduped positions
/// still reach clients.
#[instrument(level = "debug", skip_all)]
async fn resync_actuators_from_manager(cameras: Vec<Uuid>) {
    for camera_uuid in cameras {
        if !has_interest(camera_uuid) {
            continue;
        }
        let Some(state) = autopilot::cached_actuators_state(camera_uuid).await else {
            continue;
        };
        let actuators_state = match serde_json::to_value(state) {
            Ok(value) => value,
            Err(error) => {
                warn!("Failed serializing actuators state: {error}");
                continue;
            }
        };
        emit(CameraStateEvent {
            camera_uuid,
            actuators_state: Some(actuators_state),
            ..Default::default()
        });
    }
}

fn snapshot_to_event(
    camera_uuid: Uuid,
    snapshot: CameraSnapshot,
    actuators_default_config: Option<serde_json::Value>,
) -> CameraStateEvent {
    // Omit ui: live dismissals must not be overwritten by a long-running snapshot.
    // Subscribe pushes UI separately before the snapshot.
    CameraStateEvent {
        camera_uuid,
        actuators_default_config,
        actuators_config: snapshot.actuators_config,
        actuators_state: snapshot.actuators_state,
        actuators_configured: snapshot.actuators_configured,
        video_parameters: snapshot.video_parameters,
        base_parameters: snapshot.base_parameters,
        advanced_parameters: snapshot.advanced_parameters,
        ui: None,
    }
}

/// Fetches every field, falling back to `previous` for the ones that failed.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
async fn fetch_snapshot(camera_uuid: Uuid, previous: &CameraSnapshot) -> CameraSnapshot {
    let (actuators_config, actuators_state, video_parameters, base_parameters, advanced_parameters) = tokio::join!(
        fetch_actuators_config(camera_uuid),
        fetch_actuators_state(camera_uuid),
        camera_fetch_timeout(fetch_video_parameters(camera_uuid)),
        camera_fetch_timeout(fetch_base_parameters(camera_uuid)),
        camera_fetch_timeout(fetch_advanced_parameters(camera_uuid)),
    );

    CameraSnapshot {
        actuators_configured: actuators_configured(&actuators_config, previous),
        actuators_config: actuators_config
            .ok()
            .or_else(|| previous.actuators_config.clone()),
        actuators_state: actuators_state
            .ok()
            .or_else(|| previous.actuators_state.clone()),
        video_parameters: video_parameters
            .ok()
            .or_else(|| previous.video_parameters.clone()),
        base_parameters: base_parameters
            .ok()
            .or_else(|| previous.base_parameters.clone()),
        advanced_parameters: advanced_parameters
            .ok()
            .or_else(|| previous.advanced_parameters.clone()),
    }
}

/// Same as [`fetch_snapshot`], without refreshing actuator state (served by the SERVO stream).
///
/// Preserves `previous.actuators_state` so a merge does not wipe the live SERVO cache.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
async fn fetch_slow_snapshot(camera_uuid: Uuid, previous: &CameraSnapshot) -> CameraSnapshot {
    let camera = mcm_client::get_camera(&camera_uuid).await;
    if let Some(camera) = &camera {
        connectivity::observe_camera(camera_uuid, camera.hostname);
    }

    let in_mcm_list = camera.is_some();
    let expected = connectivity::is_expected(camera_uuid).await;

    // Absent from the MCM list is not a reason to stop talking to the camera: it keeps
    // answering its HTTP API on the hostname we last saw it on while ONVIF rediscovers.
    let hostname = connectivity::last_hostname(camera_uuid).await;
    let probed = hostname.is_some();
    let tcp_ok = match hostname {
        Some(hostname) => connectivity::tcp_probe(hostname, 80).await,
        None => false,
    };

    // Gate on the live probe, not the latched state: a camera can answer again long
    // before connectivity walks it back out of Unreachable.
    let skip_camera_http = !tcp_ok;

    let (actuators_config, video_parameters, base_parameters, advanced_parameters) =
        if skip_camera_http {
            (
                fetch_actuators_config(camera_uuid).await,
                Err("skipped".into()),
                Err("skipped".into()),
                Err("skipped".into()),
            )
        } else {
            tokio::join!(
                fetch_actuators_config(camera_uuid),
                camera_fetch_timeout(fetch_video_parameters(camera_uuid)),
                camera_fetch_timeout(fetch_base_parameters(camera_uuid)),
                camera_fetch_timeout(fetch_advanced_parameters(camera_uuid)),
            )
        };

    let camera_answered =
        video_parameters.is_ok() || base_parameters.is_ok() || advanced_parameters.is_ok();

    let connectivity_reason;
    let evidence = connectivity::ClassifyEvidence {
        in_mcm_list,
        expected,
        camera_answered,
        tcp_ok,
        probed,
    };
    let next = if skip_camera_http {
        connectivity_reason = if probed {
            "all camera fetches skipped; tcp :80 refused"
        } else {
            "all camera fetches skipped; no known hostname"
        };
        connectivity::classify_table(connectivity::ClassifyEvidence {
            camera_answered: false,
            ..evidence
        })
    } else if camera_answered {
        connectivity_reason = "camera HTTP answered";
        connectivity::classify_table(connectivity::ClassifyEvidence {
            camera_answered: true,
            tcp_ok: false,
            ..evidence
        })
    } else {
        connectivity_reason = "all camera fetches failed; tcp :80 accepted";
        connectivity::classify_table(evidence)
    };
    connectivity::publish(camera_uuid, next, connectivity_reason);

    CameraSnapshot {
        actuators_configured: actuators_configured(&actuators_config, previous),
        actuators_config: actuators_config
            .ok()
            .or_else(|| previous.actuators_config.clone()),
        actuators_state: previous.actuators_state.clone(),
        video_parameters: video_parameters
            .ok()
            .or_else(|| previous.video_parameters.clone()),
        base_parameters: base_parameters
            .ok()
            .or_else(|| previous.base_parameters.clone()),
        advanced_parameters: advanced_parameters
            .ok()
            .or_else(|| previous.advanced_parameters.clone()),
    }
}

/// True when an error means the camera has no actuators configured yet.
fn actuators_configured(
    actuators_config: &Result<serde_json::Value, String>,
    previous: &CameraSnapshot,
) -> Option<bool> {
    match actuators_config {
        Ok(_) => Some(true),
        Err(error) if autopilot::error_indicates_actuators_not_configured(error) => Some(false),
        Err(error) => {
            debug!("Actuators config probe failed: {error}");
            previous.actuators_configured
        }
    }
}

#[instrument(level = "debug", skip_all)]
async fn actuators_default_config() -> Option<serde_json::Value> {
    match ACTUATORS_DEFAULT_CONFIG
        .get_or_try_init(|| async {
            let value = autopilot::handle_control(ActuatorsControl {
                camera_uuid: Uuid::nil(),
                action: AutopilotAction::GetActuatorsDefaultConfig,
            })
            .await
            .map_err(|error| format!("{error:?}"))?;
            serde_json::to_value(value).map_err(|error| error.to_string())
        })
        .await
    {
        Ok(value) => Some(value.clone()),
        Err(error) => {
            debug!("Failed fetching default actuators config: {error}");
            None
        }
    }
}

#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
async fn fetch_actuators_config(camera_uuid: Uuid) -> Result<serde_json::Value, String> {
    let value = autopilot::handle_control(ActuatorsControl {
        camera_uuid,
        action: AutopilotAction::GetActuatorsConfig,
    })
    .await
    .map_err(|error| format!("{error:?}"))?;
    serde_json::to_value(value).map_err(|error| error.to_string())
}

#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
async fn fetch_actuators_state(camera_uuid: Uuid) -> Result<serde_json::Value, String> {
    let value = autopilot::handle_control(ActuatorsControl {
        camera_uuid,
        action: AutopilotAction::GetActuatorsState,
    })
    .await
    .map_err(|error| format!("{error:?}"))?;
    serde_json::to_value(value).map_err(|error| error.to_string())
}

#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
async fn fetch_video_parameters(camera_uuid: Uuid) -> Result<serde_json::Value, String> {
    let value = br4kcam_commands::handle_control(CameraControl {
        camera_uuid,
        action: CameraAction::GetVideoParameterSettings(VideoParameterSettings {
            channel: Some(VideoChannelValue::MainStream),
            ..Default::default()
        }),
    })
    .await
    .map_err(|error| format!("{error:?}"))?;
    serde_json::to_value(value).map_err(|error| error.to_string())
}

#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
async fn fetch_base_parameters(camera_uuid: Uuid) -> Result<serde_json::Value, String> {
    let value = br4kcam_commands::handle_control(CameraControl {
        camera_uuid,
        action: CameraAction::GetImageAdjustment,
    })
    .await
    .map_err(|error| format!("{error:?}"))?;
    serde_json::to_value(value).map_err(|error| error.to_string())
}

#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
async fn fetch_advanced_parameters(camera_uuid: Uuid) -> Result<serde_json::Value, String> {
    let value = br4kcam_commands::handle_control(CameraControl {
        camera_uuid,
        action: CameraAction::GetImageAdjustmentEx,
    })
    .await
    .map_err(|error| format!("{error:?}"))?;
    serde_json::to_value(value).map_err(|error| error.to_string())
}

async fn refresh_cameras_missing_from_mcm() {
    let discovered = mcm_client::cameras().await;
    let known: HashSet<Uuid> = discovered.keys().copied().collect();
    for (camera_uuid, camera) in &discovered {
        connectivity::observe_camera(*camera_uuid, camera.hostname);
    }

    let expected: HashSet<Uuid> = autopilot::configured_cameras().await.into_iter().collect();
    let missing: Vec<Uuid> = expected.difference(&known).copied().collect();
    let updates = stream::iter(missing)
        .map(|camera_uuid| async move {
            let next = if let Some(hostname) = connectivity::last_hostname(camera_uuid).await {
                let tcp_ok = connectivity::tcp_probe(hostname, 80).await;
                connectivity::classify_table(connectivity::ClassifyEvidence {
                    in_mcm_list: false,
                    expected: true,
                    camera_answered: false,
                    tcp_ok,
                    probed: true,
                })
            } else {
                connectivity::classify_table(connectivity::ClassifyEvidence {
                    in_mcm_list: false,
                    expected: true,
                    camera_answered: false,
                    tcp_ok: false,
                    probed: false,
                })
            };
            (camera_uuid, next)
        })
        .buffer_unordered(SLOW_FETCH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    for (camera_uuid, next) in updates {
        connectivity::publish(camera_uuid, next, "absent from MCM list");
    }

    let subscribed = {
        let registry = registry().lock().unwrap();
        registry
            .camera_interest
            .keys()
            .copied()
            .collect::<HashSet<_>>()
    };
    let keep = known
        .iter()
        .chain(subscribed.iter())
        .chain(expected.iter())
        .copied()
        .collect::<HashSet<_>>();
    crate::web::one_push_awb::retain_known_cameras(&keep);
    camera_ui::retain_known_cameras(&keep);
    connectivity::retain(&keep);
}

async fn camera_fetch_timeout(
    future: impl std::future::Future<Output = Result<serde_json::Value, String>>,
) -> Result<serde_json::Value, String> {
    match tokio::time::timeout(CAMERA_FETCH_TIMEOUT, future).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "camera fetch timed out after {}s",
            CAMERA_FETCH_TIMEOUT.as_secs()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn reboot_overlay_stays_until_finish_camera_action() {
        let camera_uuid = Uuid::from_u128(0xdead_beef_cafe_babe_u128);
        camera_ui::start_camera_action(camera_uuid, &CameraAction::Restart);
        assert!(camera_ui::get(camera_uuid).rebooting);

        // Successful fetches while still online must not clear the reboot overlay.
        emit(CameraStateEvent {
            camera_uuid,
            video_parameters: Some(serde_json::json!({ "ok": true })),
            ..Default::default()
        });
        assert!(camera_ui::get(camera_uuid).rebooting);

        camera_ui::finish_camera_action(camera_uuid, &CameraAction::Restart);
        assert!(!camera_ui::get(camera_uuid).rebooting);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn interest_is_dropped_with_the_last_subscription() {
        let connection_id: ConnectionId = 4242;
        let camera_uuid = Uuid::from_u128(0xfeed_face_0000_0001_u128);

        assert!(subscribe(connection_id, camera_uuid));
        assert!(subscribe(connection_id, camera_uuid));
        assert!(connection_subscribed(connection_id, camera_uuid));

        registry()
            .lock()
            .unwrap()
            .last_states
            .insert(camera_uuid, CameraSnapshot::default());

        unsubscribe_connection(connection_id);

        assert!(!connection_subscribed(connection_id, camera_uuid));
        let registry = registry().lock().unwrap();
        assert!(!registry.camera_interest.contains_key(&camera_uuid));
        assert!(!registry.last_states.contains_key(&camera_uuid));
    }

    #[test]
    fn commit_fetched_snapshot_keeps_concurrent_partial_updates() {
        let camera_uuid = Uuid::from_u128(0xfeed_face_0000_0002_u128);

        let baseline = CameraSnapshot {
            base_parameters: Some(serde_json::json!({ "gain": 1 })),
            video_parameters: Some(serde_json::json!({ "fps": 30 })),
            ..Default::default()
        };
        {
            let mut registry = registry().lock().unwrap();
            registry.camera_interest.insert(camera_uuid, 1);
            registry.last_states.insert(camera_uuid, baseline.clone());
            // Concurrent control result lands while a fetch is in flight.
            registry
                .last_states
                .get_mut(&camera_uuid)
                .unwrap()
                .base_parameters = Some(serde_json::json!({ "gain": 8 }));
        }

        let fetched = CameraSnapshot {
            base_parameters: Some(serde_json::json!({ "gain": 1 })),
            video_parameters: Some(serde_json::json!({ "fps": 60 })),
            ..Default::default()
        };

        let merged = commit_fetched_snapshot(camera_uuid, &baseline, fetched).unwrap();
        assert_eq!(
            merged.base_parameters,
            Some(serde_json::json!({ "gain": 8 }))
        );
        assert_eq!(
            merged.video_parameters,
            Some(serde_json::json!({ "fps": 60 }))
        );

        let mut registry = registry().lock().unwrap();
        registry.camera_interest.remove(&camera_uuid);
        registry.last_states.remove(&camera_uuid);
    }
}
