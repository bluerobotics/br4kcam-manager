use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use mavlink::{
    Message as _, MessageData,
    ardupilotmega::{MavMessage, SERVO_OUTPUT_RAW_DATA},
};
use once_cell::sync::{Lazy, OnceCell};
use tokio::sync::{Notify, broadcast};
use tracing::*;
use uuid::Uuid;

use crate::{
    api,
    manager::{self, MANAGER},
    mavlink::Message,
};

const MIN_EMIT_INTERVAL: Duration = Duration::from_millis(100);
/// Requesting the stream faster than the UI emit gating would only waste bandwidth.
const SERVO_STREAM_INTERVAL_US: f32 = MIN_EMIT_INTERVAL.as_micros() as f32;
/// `MAV_CMD_SET_MESSAGE_INTERVAL` disables a stream when given a negative interval.
const STREAM_DISABLED_INTERVAL_US: f32 = -1.0;
const STREAM_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const STREAM_STALE_AFTER: Duration = Duration::from_secs(3);
const RECEIVER_REOPEN_BACKOFF: Duration = Duration::from_secs(1);
/// Bound SERVO interval requests so an unresponsive autopilot cannot stall the watcher.
const SERVO_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

static STATE_TX: OnceCell<broadcast::Sender<ActuatorsStateUpdate>> = OnceCell::new();
static WATCHER_STARTED: AtomicBool = AtomicBool::new(false);
static INTEREST: AtomicUsize = AtomicUsize::new(0);
static INTEREST_CHANGED: Notify = Notify::const_new();
/// Instant of the last SERVO-backed sample per camera (missing = never / invalidated).
static LAST_SERVO_AT: Lazy<Mutex<HashMap<Uuid, Instant>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static LAST_SERVO_GLOBAL_AT: Lazy<Mutex<Option<Instant>>> = Lazy::new(|| Mutex::new(None));
static LAST_STREAM_REQUEST_AT: Lazy<Mutex<Option<Instant>>> = Lazy::new(|| Mutex::new(None));

/// Liveness of the high-rate `SERVO_OUTPUT_RAW` stream.
pub struct ServoStreamHealth {
    /// Current interest count; 0 means we deliberately asked for no stream.
    pub interest: usize,
    /// When the stream was last requested, if ever.
    pub last_request_at: Option<Instant>,
    /// Newest SERVO sample seen by the watcher, regardless of configured cameras.
    pub last_sample_at: Option<Instant>,
}

/// Actuator positions derived from a MAVLink `SERVO_OUTPUT_RAW` sample.
#[derive(Debug, Clone)]
pub struct ActuatorsStateUpdate {
    /// Camera whose actuators changed.
    pub camera_uuid: Uuid,
    /// Latest focus/zoom/tilt percentages.
    pub state: api::ActuatorsState,
}

struct EmitGate {
    last_emitted: HashMap<Uuid, api::ActuatorsState>,
    last_emit_at: HashMap<Uuid, Instant>,
    pending: HashMap<Uuid, api::ActuatorsState>,
}

impl EmitGate {
    #[instrument(level = "debug", skip_all, fields(%camera_uuid))]
    fn try_emit(
        &mut self,
        camera_uuid: Uuid,
        state: api::ActuatorsState,
        sender: &broadcast::Sender<ActuatorsStateUpdate>,
    ) {
        if self.last_emitted.get(&camera_uuid) == Some(&state) {
            self.pending.remove(&camera_uuid);
            return;
        }

        let now = Instant::now();
        if let Some(last_emit) = self.last_emit_at.get(&camera_uuid)
            && now.duration_since(*last_emit) < MIN_EMIT_INTERVAL
        {
            self.pending.insert(camera_uuid, state);
            return;
        }

        self.emit(camera_uuid, state, sender, now);
    }

    #[instrument(level = "debug", skip_all)]
    fn flush_pending(&mut self, sender: &broadcast::Sender<ActuatorsStateUpdate>) {
        let now = Instant::now();
        let pending: Vec<(Uuid, api::ActuatorsState)> = self.pending.drain().collect();

        for (camera_uuid, state) in pending {
            if self.last_emitted.get(&camera_uuid) == Some(&state) {
                continue;
            }

            let Some(last_emit) = self.last_emit_at.get(&camera_uuid) else {
                self.emit(camera_uuid, state, sender, now);
                continue;
            };

            if now.duration_since(*last_emit) >= MIN_EMIT_INTERVAL {
                self.emit(camera_uuid, state, sender, now);
            } else {
                self.pending.insert(camera_uuid, state);
            }
        }
    }

    #[instrument(level = "debug", skip_all, fields(%camera_uuid))]
    fn emit(
        &mut self,
        camera_uuid: Uuid,
        state: api::ActuatorsState,
        sender: &broadcast::Sender<ActuatorsStateUpdate>,
        now: Instant,
    ) {
        // Ignore lag when nobody is listening yet.
        if sender
            .send(ActuatorsStateUpdate { camera_uuid, state })
            .is_err()
        {
            debug!("No actuators state subscribers");
            // Do not advance last_emitted: the sample was never delivered and
            // must be eligible for replay once a receiver attaches.
            self.pending.insert(camera_uuid, state);
            return;
        }
        self.last_emitted.insert(camera_uuid, state);
        self.last_emit_at.insert(camera_uuid, now);
        self.pending.remove(&camera_uuid);
    }
}

/// Current interest count for the SERVO stream (WS subscribers / bridges).
pub fn interest_count() -> usize {
    INTEREST.load(Ordering::SeqCst)
}

/// Age of the last SERVO-backed sample for `camera_uuid`, if any.
pub fn last_servo_age(camera_uuid: Uuid) -> Option<Duration> {
    let at = *LAST_SERVO_AT.lock().ok()?.get(&camera_uuid)?;
    Some(at.elapsed())
}

/// True when a SERVO mark appeared or got younger between two [`last_servo_age`] snapshots.
pub fn servo_mark_advanced(before: Option<Duration>, after: Option<Duration>) -> bool {
    match (before, after) {
        (None, Some(_)) => true,
        (Some(before), Some(after)) => after < before,
        _ => false,
    }
}

/// True when this camera's cached actuators state is fresh enough to serve without a one-shot wait.
pub fn cache_is_fresh(camera_uuid: Uuid) -> bool {
    cache_is_fresh_age(last_servo_age(camera_uuid))
}

fn cache_is_fresh_age(age: Option<Duration>) -> bool {
    age.is_some_and(|age| age < STREAM_STALE_AFTER)
}

/// SERVO stream interest, request and sample timestamps for health classification.
pub fn stream_health() -> ServoStreamHealth {
    ServoStreamHealth {
        interest: INTEREST.load(Ordering::SeqCst),
        last_request_at: LAST_STREAM_REQUEST_AT.lock().ok().and_then(|at| *at),
        last_sample_at: LAST_SERVO_GLOBAL_AT.lock().ok().and_then(|at| *at),
    }
}

fn mark_servo_sample(camera_uuid: Uuid) {
    if let Ok(mut map) = LAST_SERVO_AT.lock() {
        map.insert(camera_uuid, Instant::now());
    }
}

fn clear_servo_freshness() {
    if let Ok(mut map) = LAST_SERVO_AT.lock() {
        map.clear();
    }
    if let Ok(mut at) = LAST_SERVO_GLOBAL_AT.lock() {
        *at = None;
    }
    if let Ok(mut at) = LAST_STREAM_REQUEST_AT.lock() {
        *at = None;
    }
}

/// Record that actuators.state was refreshed by a one-shot SERVO wait for this camera.
pub(crate) fn mark_servo_from_get_state(camera_uuid: Uuid) {
    mark_servo_sample(camera_uuid);
}

/// Latest actuators state cached on the manager for `camera_uuid`, if any.
#[instrument(level = "debug")]
pub async fn cached_actuators_state(camera_uuid: Uuid) -> Option<api::ActuatorsState> {
    let manager = MANAGER.get()?.read().await;
    manager
        .settings
        .actuators
        .get(&camera_uuid)
        .map(|actuators| actuators.state)
}

/// Subscribe to throttled actuator state updates from the SERVO stream.
pub fn subscribe() -> broadcast::Receiver<ActuatorsStateUpdate> {
    state_sender().subscribe()
}

/// Start the background SERVO_OUTPUT_RAW watcher once.
///
/// The watcher stays idle until [`add_interest`] is called: no high-rate stream
/// is requested from the autopilot while there are no interested consumers.
pub fn start() {
    if WATCHER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(actuators_watcher());
}

/// Register interest in the SERVO stream, requesting it when the first one arrives.
///
/// Every call must be paired with a [`remove_interest`] call.
#[instrument(level = "debug")]
pub fn add_interest() {
    if INTEREST.fetch_add(1, Ordering::SeqCst) == 0 {
        INTEREST_CHANGED.notify_one();
    }
}

/// Release an interest registered with [`add_interest`], disabling the SERVO
/// stream when the last one is gone.
#[instrument(level = "debug")]
pub fn remove_interest() {
    match INTEREST.try_update(Ordering::SeqCst, Ordering::SeqCst, |interests| {
        interests.checked_sub(1)
    }) {
        Ok(1) => {
            // Stream is about to stop; do not serve pre-disable cache as fresh.
            clear_servo_freshness();
            INTEREST_CHANGED.notify_one();
        }
        Ok(_) => (),
        Err(_current) => warn!("Unbalanced remove_interest call, no interest registered"),
    }
}

/// Disable the high-rate SERVO stream before process exit.
///
/// Safe to call with outstanding interest; clears the interest count and asks the
/// autopilot to stop streaming `SERVO_OUTPUT_RAW`. Bounded so an unreachable
/// autopilot cannot stall process exit for the full MAVLink ACK retry budget.
#[instrument(level = "debug")]
pub async fn shutdown() {
    INTEREST.store(0, Ordering::SeqCst);
    clear_servo_freshness();
    INTEREST_CHANGED.notify_waiters();
    if tokio::time::timeout(
        SERVO_REQUEST_TIMEOUT,
        request_servo_stream(STREAM_DISABLED_INTERVAL_US),
    )
    .await
    .is_err()
    {
        warn!("Timed out disabling SERVO_OUTPUT_RAW during shutdown");
    }
}

fn state_sender() -> &'static broadcast::Sender<ActuatorsStateUpdate> {
    STATE_TX.get_or_init(|| {
        let (sender, _) = broadcast::channel(64);
        sender
    })
}

#[instrument(level = "debug", skip_all)]
async fn actuators_watcher() {
    let sender = state_sender().clone();

    let mut gate = EmitGate {
        last_emitted: HashMap::new(),
        last_emit_at: HashMap::new(),
        pending: HashMap::new(),
    };

    let mut refresh = tokio::time::interval(STREAM_REFRESH_INTERVAL);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // None until a real SERVO sample arrives so interest-enable retries immediately.
    let mut last_servo_at: Option<Instant> = None;

    loop {
        let (mut receiver, target_system) = match open_receiver().await {
            Some(pair) => pair,
            None => {
                tokio::time::sleep(STREAM_REFRESH_INTERVAL).await;
                continue;
            }
        };

        if INTEREST.load(Ordering::SeqCst) > 0 {
            // Best-effort; refresh tick retries if the autopilot is quiet.
            let _ = tokio::time::timeout(
                SERVO_REQUEST_TIMEOUT,
                request_servo_stream(SERVO_STREAM_INTERVAL_US),
            )
            .await;
        }

        loop {
            tokio::select! {
                message = receiver.recv() => {
                    match message {
                        Ok(Message::Received((header, message)))
                            if header.system_id == target_system
                                && header.component_id
                                    == mavlink::ardupilotmega::MavComponent::MAV_COMP_ID_AUTOPILOT1 as u8
                                && message.message_id() == SERVO_OUTPUT_RAW_DATA::ID =>
                        {
                            let MavMessage::SERVO_OUTPUT_RAW(servo_output_raw) = message else {
                                continue;
                            };

                            if servo_output_raw.port != 0 {
                                continue;
                            }

                            if let Ok(mut at) = LAST_SERVO_GLOBAL_AT.lock() {
                                *at = Some(Instant::now());
                            }

                            let Some(manager) = MANAGER.get() else {
                                continue;
                            };

                            let interested = INTEREST.load(Ordering::SeqCst) > 0;

                            let mut manager = manager.write().await;
                            let mut updated = Vec::new();
                            for (camera_uuid, actuators) in &mut manager.settings.actuators {
                                let state =
                                    manager::actuators_state_from_servo(actuators, &servo_output_raw);
                                actuators.state = state;
                                if interested {
                                    updated.push(*camera_uuid);
                                    gate.try_emit(*camera_uuid, state, &sender);
                                }
                            }
                            drop(manager);

                            if interested {
                                last_servo_at = Some(Instant::now());
                                for camera_uuid in updated {
                                    mark_servo_sample(camera_uuid);
                                }
                                gate.flush_pending(&sender);
                            }
                        }
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Closed) => {
                            debug!("MAVLink receiver closed; reopening actuators watcher");
                            tokio::time::sleep(RECEIVER_REOPEN_BACKOFF).await;
                            break;
                        }
                        Err(broadcast::error::RecvError::Lagged(samples)) => {
                            debug!("Actuators watcher lagged by {samples} MAVLink messages");
                        }
                    }
                }
                _ = refresh.tick() => {
                    // Flush throttle-stranded samples even when SERVO goes quiet.
                    gate.flush_pending(&sender);
                    let stale = last_servo_at
                        .map(|at| at.elapsed() >= STREAM_STALE_AFTER)
                        .unwrap_or(true);
                    if INTEREST.load(Ordering::SeqCst) > 0 && stale {
                        debug!("SERVO_OUTPUT_RAW stale; re-requesting message interval");
                        // Best-effort; next tick retries if the autopilot is quiet.
                        let _ = tokio::time::timeout(
                            SERVO_REQUEST_TIMEOUT,
                            request_servo_stream(SERVO_STREAM_INTERVAL_US),
                        )
                        .await;
                    }
                }
                _ = INTEREST_CHANGED.notified() => {
                    if INTEREST.load(Ordering::SeqCst) > 0 {
                        // Rising edge 0→1: drop stale emit coalescing so the first
                        // fresh sample is not suppressed by a pre-disable snapshot.
                        gate.last_emitted.clear();
                        gate.last_emit_at.clear();
                        gate.pending.clear();
                        // Best-effort; refresh tick retries if the autopilot is quiet.
                        // Do not bump last_servo_at — that would fake freshness.
                        let _ = tokio::time::timeout(
                            SERVO_REQUEST_TIMEOUT,
                            request_servo_stream(SERVO_STREAM_INTERVAL_US),
                        )
                        .await;
                    } else {
                        last_servo_at = None;
                        clear_servo_freshness();
                        gate.last_emitted.clear();
                        gate.last_emit_at.clear();
                        gate.pending.clear();
                        // Best-effort disable; shutdown() also times this out.
                        let _ = tokio::time::timeout(
                            SERVO_REQUEST_TIMEOUT,
                            request_servo_stream(STREAM_DISABLED_INTERVAL_US),
                        )
                        .await;
                    }
                }
            }
        }
    }
}

#[instrument(level = "debug", skip_all)]
async fn open_receiver() -> Option<(broadcast::Receiver<Message>, u8)> {
    let mavlink = crate::mavlink::component().ok()?;
    let receiver = mavlink.get_receiver().await;
    let target_system = mavlink.system_id();
    Some((receiver, target_system))
}

#[instrument(level = "debug")]
async fn request_servo_stream(interval_us: f32) {
    if interval_us != STREAM_DISABLED_INTERVAL_US
        && let Ok(mut at) = LAST_STREAM_REQUEST_AT.lock()
    {
        *at = Some(Instant::now());
    }

    let Ok(mavlink) = crate::mavlink::component() else {
        return;
    };
    if let Err(error) = mavlink
        .set_message_interval(SERVO_OUTPUT_RAW_DATA::ID, interval_us)
        .await
    {
        debug!("Failed setting SERVO_OUTPUT_RAW stream interval: {error:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_gate_dedups_identical_state() {
        let (sender, mut receiver) = broadcast::channel(8);
        let mut gate = EmitGate {
            last_emitted: HashMap::new(),
            last_emit_at: HashMap::new(),
            pending: HashMap::new(),
        };
        let camera_uuid = Uuid::nil();
        let state = api::ActuatorsState {
            focus: Some(10.0),
            zoom: Some(20.0),
            tilt: Some(30.0),
        };

        gate.try_emit(camera_uuid, state, &sender);
        gate.try_emit(camera_uuid, state, &sender);

        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn interest_refcount_saturates_at_zero() {
        add_interest();
        add_interest();
        assert_eq!(INTEREST.load(Ordering::SeqCst), 2);

        remove_interest();
        remove_interest();
        remove_interest();
        assert_eq!(INTEREST.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn emit_gate_queues_when_throttled() {
        let (sender, mut receiver) = broadcast::channel(8);
        let mut gate = EmitGate {
            last_emitted: HashMap::new(),
            last_emit_at: HashMap::new(),
            pending: HashMap::new(),
        };
        let camera_uuid = Uuid::nil();
        let first = api::ActuatorsState {
            focus: Some(1.0),
            zoom: None,
            tilt: None,
        };
        let second = api::ActuatorsState {
            focus: Some(2.0),
            zoom: None,
            tilt: None,
        };

        gate.try_emit(camera_uuid, first, &sender);
        gate.try_emit(camera_uuid, second, &sender);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
        assert!(gate.pending.contains_key(&camera_uuid));

        // Pretend enough time passed.
        gate.last_emit_at
            .insert(camera_uuid, Instant::now() - MIN_EMIT_INTERVAL);
        gate.flush_pending(&sender);
        let update = receiver.try_recv().expect("pending flush");
        assert_eq!(update.state.focus, Some(2.0));
    }

    #[test]
    fn emit_gate_keeps_pending_when_send_fails() {
        let (sender, receiver) = broadcast::channel(8);
        drop(receiver);
        let mut gate = EmitGate {
            last_emitted: HashMap::new(),
            last_emit_at: HashMap::new(),
            pending: HashMap::new(),
        };
        let camera_uuid = Uuid::nil();
        let state = api::ActuatorsState {
            focus: Some(10.0),
            zoom: None,
            tilt: None,
        };

        gate.try_emit(camera_uuid, state, &sender);
        assert!(!gate.last_emitted.contains_key(&camera_uuid));
        assert!(gate.pending.contains_key(&camera_uuid));
    }

    #[test]
    fn cache_freshness_uses_stale_threshold() {
        assert!(!cache_is_fresh_age(None));
        assert!(cache_is_fresh_age(Some(Duration::ZERO)));
        assert!(cache_is_fresh_age(Some(
            STREAM_STALE_AFTER - Duration::from_millis(1)
        )));
        assert!(!cache_is_fresh_age(Some(STREAM_STALE_AFTER)));
        assert!(!cache_is_fresh_age(Some(
            STREAM_STALE_AFTER + Duration::from_secs(1)
        )));
    }

    #[test]
    fn servo_sample_marks_are_per_camera() {
        // Hold the map lock across mark+read so a parallel test cannot clear
        // LAST_SERVO_AT between the two (the flake that failed CI-style probes).
        let camera_a = Uuid::nil();
        let camera_b = Uuid::from_u128(1);
        let mut map = LAST_SERVO_AT.lock().expect("servo map");
        map.clear();
        assert!(!map.contains_key(&camera_a));
        assert!(!map.contains_key(&camera_b));
        map.insert(camera_a, Instant::now());
        assert!(map.contains_key(&camera_a));
        assert!(!map.contains_key(&camera_b));
    }
}
