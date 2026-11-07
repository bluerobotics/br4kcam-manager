//! Backend-owned one-push white balance lifecycle.
//!
//! The camera's one-push (onceAWB) is its own closed loop on the video: it sets manual
//! gains, measures a frame, adjusts, and writes its result once, at the end, as Manual white
//! balance (firmware, xipc `472210`; see tools/camera_specs.md). Only its first run after the
//! camera starts switches to auto white balance first; later runs start from whatever gains
//! are applied. So every trigger is primed here: white balance goes to Auto, settles, then
//! onceAWB goes out, which also makes the end visible (Auto turns Manual).
//!
//! Pure settle logic is unit-tested; the tracker polls base parameters and drives
//! [`camera_ui`] / [`camera_state`] so every client stays in sync.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use br4kcam_api::{CameraStateEvent, OnePushAwbPhase, OnePushAwbStatus};
use br4kcam_commands::{
    Action as CameraAction, CameraControl,
    protocol::display::base_display::{BaseAutoWhiteBalanceModeValue, BaseParameterSetting},
};
use once_cell::sync::OnceCell;
use tokio::task::JoinHandle;
use tracing::*;
use uuid::Uuid;

use crate::web::{camera_state, camera_ui};

/// Consecutive identical Manual RGB samples required after movement before settle.
pub const STABLE_SAMPLES: u8 = 2;
/// Ceiling on a primed run. A run measures a fresh substream frame per step, up to 300 steps
/// (firmware), so at a 5 fps substream it can pass a minute; typical runs take well under a
/// second (measured: 0.25 s). The end is seen as it happens, so this only bounds a stuck run.
pub const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
/// Ceiling on a run that could not be primed: its end shows only if the gains change.
pub const UNPRIMED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Minimum time WB controls stay busy from trigger (Cooldown fills any shortfall).
pub const MIN_BUSY: std::time::Duration = std::time::Duration::from_secs(1);
/// Poll interval while Running.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// How long auto white balance gets on Auto before onceAWB starts from its gains.
pub const PRIME_SETTLE: std::time::Duration = std::time::Duration::from_secs(1);

/// `auto_awb` value for Manual in the camera protocol (`BaseAutoWhiteBalanceModeValue::Manual`).
const AUTO_AWB_MANUAL: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbSample {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

/// WB fields pulled from a getImageAdjustment JSON body (ignores unrelated enums).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwbSample {
    pub rgb: Option<RgbSample>,
    pub auto_awb: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettleMachine {
    /// White balance was put on Auto right before the trigger.
    primed: bool,
    start_rgb: Option<RgbSample>,
    last_rgb: Option<RgbSample>,
    saw_change: bool,
    stable_count: u8,
    /// The last Manual gains seen, in a primed run.
    last_manual: Option<RgbSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserveResult {
    Continue,
    Settled,
}

struct TrackerEntry {
    handle: JoinHandle<()>,
    generation: u64,
}

static TRACKERS: OnceCell<Mutex<HashMap<Uuid, TrackerEntry>>> = OnceCell::new();
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

impl AwbSample {
    /// Read only AWB fields so unknown/extra camera enums cannot abort the tracker.
    pub fn from_json(value: &serde_json::Value) -> Self {
        let u8_field = |name: &str| value.get(name).and_then(|v| v.as_u64()).map(|v| v as u8);
        let red = u8_field("awb_red");
        let green = u8_field("awb_green");
        let blue = u8_field("awb_blue");
        let rgb = match (red, green, blue) {
            (Some(red), Some(green), Some(blue)) => Some(RgbSample { red, green, blue }),
            _ => None,
        };
        Self {
            rgb,
            auto_awb: u8_field("auto_awb"),
        }
    }
}

impl SettleMachine {
    /// `primed`: white balance was put on Auto right before the trigger (see [`prime`]).
    pub fn begin(start: &AwbSample, primed: bool) -> Self {
        let manual = start.auto_awb == Some(AUTO_AWB_MANUAL);
        Self {
            primed,
            start_rgb: start.rgb,
            last_rgb: start.rgb,
            saw_change: false,
            stable_count: 0,
            last_manual: if primed && manual { start.rgb } else { None },
        }
    }

    pub fn saw_change(&self) -> bool {
        self.saw_change
    }

    /// Feed a base-parameter sample.
    ///
    /// Primed, the run started on Auto and the firmware writes Manual once, at its end: two
    /// identical Manual samples settle, whether or not the gains changed. Unprimed, Manual
    /// may be the gains from before, so it settles only after RGB has moved and then held
    /// steady while Manual.
    pub fn observe(&mut self, sample: &AwbSample) -> ObserveResult {
        let manual = sample.auto_awb == Some(AUTO_AWB_MANUAL);

        if self.primed {
            match sample.rgb.filter(|_| manual) {
                Some(rgb) if self.last_manual == Some(rgb) => return ObserveResult::Settled,
                rgb => self.last_manual = rgb,
            }
            return ObserveResult::Continue;
        }

        if let Some(current) = sample.rgb {
            if let Some(last) = self.last_rgb {
                if current != last {
                    self.saw_change = true;
                    self.stable_count = 0;
                } else if self.saw_change && manual {
                    self.stable_count = self.stable_count.saturating_add(1);
                }
            }
            if let Some(start) = self.start_rgb
                && current != start
            {
                self.saw_change = true;
            }
            self.last_rgb = Some(current);
        }

        if self.saw_change && manual && self.stable_count >= STABLE_SAMPLES {
            return ObserveResult::Settled;
        }
        ObserveResult::Continue
    }
}

/// True while Running or Cooldown for this camera.
pub(crate) fn is_busy(camera_uuid: Uuid) -> bool {
    camera_ui::get(camera_uuid).one_push_awb.is_some()
}

/// True when `action` is a one-push AWB trigger.
pub(crate) fn action_triggers_once_awb(action: &CameraAction) -> bool {
    match action {
        CameraAction::SetImageAdjustmentEx(params)
        | CameraAction::SetImageAdjustmentExAll(params) => params.once_awb == Some(1),
        _ => false,
    }
}

/// Abort the poll task and clear UI phase (camera gone / pruned).
#[instrument(level = "debug")]
pub(crate) fn abort(camera_uuid: Uuid) {
    if let Some(entry) = trackers().lock().unwrap().remove(&camera_uuid) {
        entry.handle.abort();
    }
    camera_ui::set_one_push_awb(camera_uuid, None);
}

/// Abort every tracker whose camera is no longer known.
#[instrument(level = "debug", skip_all)]
pub(crate) fn retain_known_cameras(known: &std::collections::HashSet<Uuid>) {
    let stale: Vec<Uuid> = {
        let lock = trackers().lock().unwrap();
        lock.keys()
            .copied()
            .filter(|uuid| !known.contains(uuid))
            .collect()
    };
    for uuid in stale {
        abort(uuid);
    }
}

/// Puts each camera `action` triggers one-push on to Auto white balance and lets it settle,
/// before the trigger goes out. Answers the cameras that took it.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
pub(crate) async fn prime(camera_uuid: Uuid, action: &CameraAction) -> HashSet<Uuid> {
    let cameras: Vec<Uuid> = match action {
        CameraAction::SetImageAdjustmentEx(_) if !is_busy(camera_uuid) => vec![camera_uuid],
        CameraAction::SetImageAdjustmentExAll(_) => mcm_client::cameras()
            .await
            .keys()
            .copied()
            .filter(|uuid| !is_busy(*uuid))
            .collect(),
        _ => return HashSet::new(),
    };
    let mut primed = HashSet::new();
    for uuid in cameras {
        // Busy from now on, so the controls show it and a second trigger is a no-op.
        set_running(uuid);
        let auto = CameraAction::SetImageAdjustment(BaseParameterSetting {
            auto_awb: Some(BaseAutoWhiteBalanceModeValue::Auto),
            ..Default::default()
        });
        match br4kcam_commands::handle_control(CameraControl {
            camera_uuid: uuid,
            action: auto,
        })
        .await
        {
            Ok(_) => {
                primed.insert(uuid);
            }
            // Still triggered: the run starts from the gains in use, and its end shows only
            // if they change.
            Err(error) => {
                warn!("Could not put {uuid} on Auto before one-push: {error:?}");
                camera_ui::set_one_push_awb(uuid, None);
            }
        }
    }
    if !primed.is_empty() {
        tokio::time::sleep(PRIME_SETTLE).await;
    }
    primed
}

/// Clears the busy state [`prime`] set, for a trigger that did not go out.
pub(crate) fn unprime(primed: &HashSet<Uuid>) {
    for uuid in primed {
        camera_ui::set_one_push_awb(*uuid, None);
    }
}

fn set_running(camera_uuid: Uuid) {
    camera_ui::set_one_push_awb(
        camera_uuid,
        Some(OnePushAwbStatus {
            phase: OnePushAwbPhase::Running,
        }),
    );
}

/// Start Running after a successful onceAWB SET, or no-op if already busy (a primed camera
/// is busy since [`prime`]).
#[instrument(level = "debug")]
pub(crate) fn begin_after_trigger(camera_uuid: Uuid, primed: bool) {
    if is_busy(camera_uuid) && !primed {
        return;
    }

    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);

    {
        let mut lock = trackers().lock().unwrap();
        if let Some(entry) = lock.remove(&camera_uuid) {
            entry.handle.abort();
        }
    }

    set_running(camera_uuid);

    let handle =
        tokio::spawn(run_tracker(camera_uuid, generation, primed).instrument(Span::current()));
    trackers()
        .lock()
        .unwrap()
        .insert(camera_uuid, TrackerEntry { handle, generation });
}

/// After a successful control that included onceAWB, start trackers. `primed` are the
/// cameras [`prime`] put on Auto first.
#[instrument(level = "debug", skip_all, fields(%camera_uuid))]
pub(crate) fn on_control_success(camera_uuid: Uuid, action: &CameraAction, primed: &HashSet<Uuid>) {
    if !action_triggers_once_awb(action) {
        return;
    }

    match action {
        CameraAction::SetImageAdjustmentEx(_) => {
            begin_after_trigger(camera_uuid, primed.contains(&camera_uuid));
        }
        CameraAction::SetImageAdjustmentExAll(_) => {
            let span = Span::current();
            let primed = primed.clone();
            tokio::spawn(
                async move {
                    let cameras = mcm_client::cameras().await;
                    for uuid in cameras.keys().copied() {
                        begin_after_trigger(uuid, primed.contains(&uuid));
                    }
                }
                .instrument(span),
            );
        }
        _ => {}
    }
}

fn trackers() -> &'static Mutex<HashMap<Uuid, TrackerEntry>> {
    TRACKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn is_current_generation(camera_uuid: Uuid, generation: u64) -> bool {
    trackers()
        .lock()
        .unwrap()
        .get(&camera_uuid)
        .is_some_and(|entry| entry.generation == generation)
}

#[instrument(level = "debug", skip_all, fields(%camera_uuid, generation))]
async fn run_tracker(camera_uuid: Uuid, generation: u64, primed: bool) {
    let start = Instant::now();
    let timeout = if primed {
        RUN_TIMEOUT
    } else {
        UNPRIMED_TIMEOUT
    };

    // Retry the opening snapshot — a single failed GET right after onceAWB used to
    // finish_timeout immediately and clear the button in well under a second.
    let (initial_json, initial) = loop {
        match fetch_base_json(camera_uuid).await {
            Ok(value) => {
                let sample = AwbSample::from_json(&value);
                break (value, sample);
            }
            Err(error) => {
                warn!("one-push AWB start snapshot failed: {error}");
                if !is_current_generation(camera_uuid, generation) {
                    return;
                }
                if start.elapsed() >= timeout {
                    finish_timeout(camera_uuid, generation);
                    return;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    };

    if !is_current_generation(camera_uuid, generation) {
        return;
    }

    camera_state::emit(CameraStateEvent {
        camera_uuid,
        base_parameters: Some(initial_json),
        ..Default::default()
    });

    let mut machine = SettleMachine::begin(&initial, primed);
    let mut last_emitted = initial.clone();

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        if !is_current_generation(camera_uuid, generation) {
            return;
        }

        let sample_json = match fetch_base_json(camera_uuid).await {
            Ok(value) => value,
            Err(error) => {
                warn!("one-push AWB poll failed: {error}");
                if start.elapsed() >= timeout {
                    if machine.saw_change() {
                        finish_timeout(camera_uuid, generation);
                    } else {
                        finish_settled(camera_uuid, start, generation).await;
                    }
                    return;
                }
                continue;
            }
        };
        let sample = AwbSample::from_json(&sample_json);

        if sample != last_emitted {
            camera_state::emit(CameraStateEvent {
                camera_uuid,
                base_parameters: Some(sample_json),
                ..Default::default()
            });
            last_emitted = sample.clone();
        }

        if machine.observe(&sample) == ObserveResult::Settled {
            finish_settled(camera_uuid, start, generation).await;
            return;
        }

        if start.elapsed() >= timeout {
            if machine.saw_change() {
                finish_timeout(camera_uuid, generation);
            } else {
                finish_settled(camera_uuid, start, generation).await;
            }
            return;
        }
    }
}

/// Hold Cooldown only long enough to honour [`MIN_BUSY`] from `started_at`.
async fn finish_settled(camera_uuid: Uuid, started_at: Instant, generation: u64) {
    if !is_current_generation(camera_uuid, generation) {
        return;
    }
    let remaining = MIN_BUSY.saturating_sub(started_at.elapsed());
    if !remaining.is_zero() {
        camera_ui::set_one_push_awb(
            camera_uuid,
            Some(OnePushAwbStatus {
                phase: OnePushAwbPhase::Cooldown,
            }),
        );
        tokio::time::sleep(remaining).await;
        if !is_current_generation(camera_uuid, generation) {
            return;
        }
    }
    camera_ui::set_one_push_awb(camera_uuid, None);
    remove_tracker_if_current(camera_uuid, generation);
}

fn finish_timeout(camera_uuid: Uuid, generation: u64) {
    if !is_current_generation(camera_uuid, generation) {
        return;
    }
    camera_ui::set_warning(camera_uuid, "One-push white balance timed out".to_string());
    camera_ui::set_one_push_awb(camera_uuid, None);
    remove_tracker_if_current(camera_uuid, generation);
}

fn remove_tracker_if_current(camera_uuid: Uuid, generation: u64) {
    let mut lock = trackers().lock().unwrap();
    if lock
        .get(&camera_uuid)
        .is_some_and(|entry| entry.generation == generation)
    {
        lock.remove(&camera_uuid);
    }
}

async fn fetch_base_json(camera_uuid: Uuid) -> Result<serde_json::Value, String> {
    br4kcam_commands::handle_control(CameraControl {
        camera_uuid,
        action: CameraAction::GetImageAdjustment,
    })
    .await
    .map_err(|error| format!("{error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(r: u8, g: u8, b: u8, manual: bool) -> AwbSample {
        AwbSample {
            rgb: Some(RgbSample {
                red: r,
                green: g,
                blue: b,
            }),
            auto_awb: Some(if manual { AUTO_AWB_MANUAL } else { 0 }),
        }
    }

    #[test]
    fn awb_sample_ignores_unrelated_fields() {
        let value = serde_json::json!({
            "sceneMode": 0,
            "auto_awb": 1,
            "awb_red": 10,
            "awb_green": 20,
            "awb_blue": 30,
            "extra_future_field": true,
        });
        assert_eq!(AwbSample::from_json(&value), sample(10, 20, 30, true));
    }

    #[test]
    fn manual_after_rgb_change_needs_stable_samples() {
        let start = sample(10, 20, 30, false);
        let mut machine = SettleMachine::begin(&start, false);
        assert_eq!(
            machine.observe(&sample(11, 20, 30, false)),
            ObserveResult::Continue
        );
        assert_eq!(
            machine.observe(&sample(11, 20, 30, true)),
            ObserveResult::Continue
        );
        assert_eq!(
            machine.observe(&sample(11, 20, 30, true)),
            ObserveResult::Settled
        );
    }

    #[test]
    fn already_manual_needs_stable_rgb_after_change() {
        let start = sample(10, 20, 30, true);
        let mut machine = SettleMachine::begin(&start, false);
        assert_eq!(
            machine.observe(&sample(40, 20, 30, true)),
            ObserveResult::Continue
        );
        assert_eq!(
            machine.observe(&sample(40, 20, 30, true)),
            ObserveResult::Continue
        );
        assert_eq!(
            machine.observe(&sample(40, 20, 30, true)),
            ObserveResult::Settled
        );
    }

    #[test]
    fn unchanged_manual_does_not_settle_early() {
        // Repro: Manual gains set, onceAWB triggered — camera lags before RGB moves.
        let start = sample(41, 128, 81, true);
        let mut machine = SettleMachine::begin(&start, false);
        for _ in 0..5 {
            assert_eq!(
                machine.observe(&sample(41, 128, 81, true)),
                ObserveResult::Continue
            );
        }
        assert!(!machine.saw_change());
    }

    #[test]
    fn auto_rgb_stability_waits_for_manual_then_hold() {
        let start = sample(10, 20, 30, false);
        let mut machine = SettleMachine::begin(&start, false);
        assert_eq!(
            machine.observe(&sample(40, 20, 30, false)),
            ObserveResult::Continue
        );
        // Auto identical samples do not accumulate Manual stability.
        assert_eq!(
            machine.observe(&sample(40, 20, 30, false)),
            ObserveResult::Continue
        );
        assert_eq!(
            machine.observe(&sample(40, 20, 30, true)),
            ObserveResult::Continue
        );
        assert_eq!(
            machine.observe(&sample(40, 20, 30, true)),
            ObserveResult::Settled
        );
    }

    #[test]
    fn primed_settles_on_the_manual_result() {
        // Primed to Auto: the firmware writes Manual once, at the end of its run.
        let start = sample(10, 20, 30, false);
        let mut machine = SettleMachine::begin(&start, true);
        assert_eq!(
            machine.observe(&sample(10, 20, 30, false)),
            ObserveResult::Continue
        );
        assert_eq!(
            machine.observe(&sample(13, 0, 22, true)),
            ObserveResult::Continue
        );
        assert_eq!(
            machine.observe(&sample(13, 0, 22, true)),
            ObserveResult::Settled
        );
    }

    #[test]
    fn primed_settles_when_the_result_matches_the_old_gains() {
        // The same gains as before the run: unprimed, this waited for the timeout.
        let start = sample(41, 128, 81, false);
        let mut machine = SettleMachine::begin(&start, true);
        assert_eq!(
            machine.observe(&sample(41, 128, 81, true)),
            ObserveResult::Continue
        );
        assert_eq!(
            machine.observe(&sample(41, 128, 81, true)),
            ObserveResult::Settled
        );
    }

    #[test]
    fn primed_run_that_ended_before_the_first_snapshot_settles() {
        // Runs can take 0.25 s: the opening snapshot may already be the Manual result.
        let start = sample(13, 0, 22, true);
        let mut machine = SettleMachine::begin(&start, true);
        assert_eq!(
            machine.observe(&sample(13, 0, 22, true)),
            ObserveResult::Settled
        );
    }

    #[test]
    fn primed_waits_while_white_balance_is_auto() {
        let start = sample(10, 20, 30, false);
        let mut machine = SettleMachine::begin(&start, true);
        for _ in 0..5 {
            assert_eq!(
                machine.observe(&sample(10, 20, 30, false)),
                ObserveResult::Continue
            );
        }
    }

    #[test]
    fn min_busy_remaining_is_zero_after_floor() {
        assert_eq!(MIN_BUSY.saturating_sub(MIN_BUSY), std::time::Duration::ZERO);
        assert_eq!(
            MIN_BUSY.saturating_sub(MIN_BUSY + std::time::Duration::from_millis(1)),
            std::time::Duration::ZERO
        );
        assert_eq!(
            MIN_BUSY.saturating_sub(std::time::Duration::from_millis(250)),
            std::time::Duration::from_millis(750)
        );
    }
}
