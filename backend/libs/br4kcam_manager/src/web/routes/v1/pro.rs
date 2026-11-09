//! `/pro`: traditional camera controls for integrators (PASM exposure modes, shutter,
//! ISO, white balance, focus, zoom, tilt), mapped onto the camera and autopilot controls.
//!
//! Served over HTTP and the WebSocket alike. Every change goes through
//! [`control_bridge`], so the UI and WebSocket clients follow it.

use autopilot::api::{
    Action as AutopilotAction, ActuatorsConfig, ActuatorsControl, ActuatorsState, LensType,
};
use axum::{
    Json, Router,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use br4kcam_api::CameraStateEvent;
use br4kcam_api::pro::{
    Aperture, ExposureMode, ExposureWarning, FullGain, MaxIso, ProCapabilities, ProSettings,
    ProState, Range, WhiteBalance, WhiteBalanceScene,
};
use br4kcam_commands::{
    Action as CameraAction, CameraControl,
    protocol::display::{
        advanced_display::{AdvancedDisplayAutoIrisValue, AdvancedParameterSetting},
        base_display::{
            BaseAutoGainModeValue, BaseAutoWhiteBalanceModeValue, BaseAutoWhiteBalanceSceneValue,
            BaseExposureModeValue, BaseExposureTimeValue, BaseManualAGainEnableValue,
            BaseManualDGainEnableValue, BaseMaxExposureValue, BaseParameterSetting,
        },
    },
};
use indexmap::IndexMap;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::*;
use uuid::Uuid;

use crate::web::control_bridge::{self, ControlError, UNKNOWN_CAMERA};

const MODES: [ExposureMode; 3] = [
    ExposureMode::Program,
    ExposureMode::Shutter,
    ExposureMode::Manual,
];

/// ISO stops and the analog and digital gains (0..=255) that give them, measured with
/// tools/iso_calibration/calibrate_iso.py: each stop doubles the brightness of the one
/// before it, at a reference brightness near the ISO standard's mid-gray. ISO 100 is no gain.
/// ISO 204800 is the top: full gain falls ~0.15 stop short of ISO 409600. The camera's
/// max_sys_gain does not limit these (it only caps auto exposure's gain).
const ISO_GAINS: [(u32, u8, u8); 12] = [
    (100, 0, 0),
    (200, 11, 0),
    (400, 20, 0),
    (800, 40, 0),
    (1600, 83, 0),
    (3200, 167, 0),
    (6400, 179, 8),
    (12800, 191, 16),
    (25600, 235, 28),
    (51200, 252, 56),
    (102400, 255, 104),
    (204800, 255, 160),
];

/// The camera's own ISO (100 x the gain it applies, from `getISPInfo`) and the ISO it is on
/// our scale, at each manual stop of `ISO_GAINS`: brightness does not follow the applied gain
/// exactly (up to ~0.6 stop apart at high gain), so the camera's number is converted through
/// this, interpolated in log(ISO), to keep Program and Manual on one scale.
const CAMERA_ISO: [(u32, u32); 12] = [
    (100, 100),
    (199, 200),
    (489, 400),
    (977, 800),
    (1949, 1600),
    (4027, 3200),
    (8630, 6400),
    (18494, 12800),
    (39819, 25600),
    (85334, 51200),
    (158478, 102400),
    (243812, 204800),
];

/// Maximum ISOs for Program mode and the camera's `max_sys_gain` that gives each: the ceiling
/// on the gain its auto exposure may use (manual gains ignore it). Read with the lens covered,
/// where auto exposure sits at its ceiling (tools/iso_calibration/data/*_max_sys_gain_dark.csv):
/// the ceiling is linear, ~camera ISO 4804 per unit, analog gain first, so ISO 200 to 3200
/// cannot be ceilings; the camera's default, 12, allows ~ISO 36000. It saturates around 120,
/// full gain ([`MaxIso::Full`], written as [`FULL_SYS_GAIN`]): past the calibrated ISO 204800,
/// so it has no ISO. Shutter priority shares the maximum ISO, kept here (Manual does not
/// touch `max_sys_gain`), and is capped by [`shutter_gain_caps`] as well.
const MAX_ISO_GAINS: [(u32, u8); 7] = [
    (100, 0),
    (6400, 2),
    (12800, 4),
    (25600, 8),
    (51200, 18),
    (102400, 33),
    (204800, 51),
];

/// `max_sys_gain` for [`MaxIso::Full`]; anything above the last of [`MAX_ISO_GAINS`] reads
/// as full gain.
const FULL_SYS_GAIN: u8 = u8::MAX;

/// Shutter priority's maximum ISO when it is entered with none (full gain): uncapped, it
/// climbs to camera ISO 1.9 M in the dark (measured), a picture of noise.
const SHUTTER_DEFAULT_MAX_ISO: u32 = 51200;

/// Program's longest shutter when it is entered with none kept: in dim water it parks there,
/// and 1/30 smears on a moving vehicle.
const PROGRAM_DEFAULT_MAX_EXPOSURE: BaseMaxExposureValue = BaseMaxExposureValue::_60;

/// Auto exposure's analog gain tops out at 75x (getISPInfo AGain 76789, measured).
const MAX_ANALOG_GAIN: f64 = 75.0;

/// How far off its target auto exposure may read (getISPInfo HistError, positive when dark)
/// before the picture counts as too dark or too bright: settled on target it read -10 to +23,
/// held at its gain ceiling in the dark 86 to 118 (measured).
const EXPOSURE_ERROR_LIMIT: i64 = 40;

/// Colour temperatures the camera's Temperature white balance takes (`wb_temp`; it clamps to
/// this, measured to change the gains smoothly across it on both firmwares).
const KELVIN_RANGE: std::ops::RangeInclusive<u16> = 1500..=15000;

/// The colour temperature the Temperature white balance starts at when the camera has none in
/// range: daylight.
const DEFAULT_KELVIN: u16 = 5500;

/// Exposure (shutter, ISO, compensation and the limits) moves toward a requested value by at
/// most this many stops per step: big jumps make the camera pause or drop its stream.
const SLEW_STOPS: f64 = 1.0;

/// At most one exposure step per this long: about how long a change takes to reach the video.
const SLEW_INTERVAL: Duration = Duration::from_millis(250);

/// Exposure compensations in EV and the camera's `blc_level` that gives each: the brightness
/// auto exposure aims for (50, the camera's default, is 0 EV). Measured from the exposure
/// auto exposure settled on (`getISPInfo` exposure time x gain) at each `blc_level`, in dim
/// light, interpolated: 0 is -3.1 EV, 128 is +1.8 EV; above +1.5 EV auto exposure ran out of
/// gain there.
const EXPOSURE_COMPENSATION: [(f32, u8); 8] = [
    (-2.0, 6),
    (-1.5, 9),
    (-1.0, 20),
    (-0.5, 32),
    (0.0, 50),
    (0.5, 74),
    (1.0, 98),
    (1.5, 118),
];

/// Shutters that expose what they say at 30 fps: longer ones are held to the frame time unless
/// the frame rate is lowered, and 1/100000 s is below the camera's 30 us floor (with
/// `auto_iris` off, as PRO sets it; on, the floor is 200 us). The others are still accepted,
/// as uncalibrated.
fn calibrated_shutter(time: &BaseExposureTimeValue) -> bool {
    (30..=10000).contains(&time.denominator())
}

/// Maximum shutters auto exposure was seen to hold to: it ignored a 1/8000 cap, measured with
/// `auto_iris` on (its 200 us floor); not retried with it off.
fn calibrated_max_shutter(time: &BaseExposureTimeValue) -> bool {
    (30..=4000).contains(&time.denominator())
}

/// White balance scenes, in the order of their protocol values.
const WHITE_BALANCE_SCENES: [(WhiteBalanceScene, BaseAutoWhiteBalanceSceneValue); 2] = [
    (
        WhiteBalanceScene::GreenWater,
        BaseAutoWhiteBalanceSceneValue::Scene1,
    ),
    (
        WhiteBalanceScene::BlueWater,
        BaseAutoWhiteBalanceSceneValue::Scene2,
    ),
];

#[instrument(level = "trace")]
pub fn router() -> Router {
    Router::new()
        .route("/{camera_uuid}", get(http_state).patch(http_set))
        .route("/{camera_uuid}/capabilities", get(http_capabilities))
        .route("/{camera_uuid}/white_balance/one_push", post(http_one_push))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}

/// Serves a `/pro/...` WebSocket request, as `(status, body)`, or `None` when no route matches.
pub(crate) async fn ws_request(
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Option<(u16, Value)> {
    let rest = path.strip_prefix("/pro/")?;
    let (uuid, route) = rest.split_once('/').unwrap_or((rest, ""));
    let Ok(camera_uuid) = uuid.parse::<Uuid>() else {
        return Some((400, json!(format!("Invalid camera UUID {uuid:?}"))));
    };

    let result = match (method, route) {
        ("GET", "") => state(camera_uuid).await.map(|state| (200, json!(state))),
        ("PATCH", "") => match serde_json::from_value(body.unwrap_or(Value::Null)) {
            Ok(settings) => set(camera_uuid, settings)
                .await
                .map(|state| (200, json!(state))),
            Err(error) => Err(bad_request(format!("{error}"))),
        },
        ("GET", "capabilities") => capabilities(camera_uuid)
            .await
            .map(|capabilities| (200, json!(capabilities))),
        ("POST", "white_balance/one_push") => {
            one_push(camera_uuid).await.map(|()| (202, Value::Null))
        }
        _ => return None,
    };

    Some(result.unwrap_or_else(|error| (error.status, Value::String(error.message))))
}

fn respond<T: serde::Serialize>(result: Result<T, ControlError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}

fn error_response(error: ControlError) -> Response {
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, error.message).into_response()
}

#[instrument(level = "debug")]
async fn http_state(Path(camera_uuid): Path<Uuid>) -> Response {
    respond(state(camera_uuid).await)
}

#[instrument(level = "debug")]
async fn http_set(Path(camera_uuid): Path<Uuid>, Json(settings): Json<ProSettings>) -> Response {
    respond(set(camera_uuid, settings).await)
}

#[instrument(level = "debug")]
async fn http_capabilities(Path(camera_uuid): Path<Uuid>) -> Response {
    respond(capabilities(camera_uuid).await)
}

#[instrument(level = "debug")]
async fn http_one_push(Path(camera_uuid): Path<Uuid>) -> Response {
    match one_push(camera_uuid).await {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(error) => error_response(error),
    }
}

fn bad_request(message: impl Into<String>) -> ControlError {
    ControlError {
        status: 400,
        message: message.into(),
    }
}

/// Current controls of the camera. While its exposure ramps (see [`Slew`]), the values ramping
/// read as where they are going, so a dragged slider stays where it was left.
async fn state(camera_uuid: Uuid) -> Result<ProState, ControlError> {
    let params = read_camera(camera_uuid, CameraAction::GetImageAdjustment).await?;
    let mut state = state_of(&params);
    // Best effort: the state stands without what the image pipeline reports.
    if let Ok(info) = read_camera(camera_uuid, CameraAction::GetIspInfo).await {
        apply_isp_info(&mut state, &isp_info(&info));
    }
    if let Some(target) = Slew::of(camera_uuid).lock().await.target.as_ref() {
        show_target(&mut state, target);
    }
    if let Some(actuators) = Actuators::of(camera_uuid).await {
        let positions =
            read_autopilot::<ActuatorsState>(camera_uuid, AutopilotAction::GetActuatorsState)
                .await
                .unwrap_or_default();
        (state.focus, state.zoom, state.tilt) = actuators.read(positions);
    }
    Ok(state)
}

/// [`state`] as the cached `snapshot` has it, without asking the camera or the autopilot: the
/// settings and the lens positions, none of what the image pipeline reports. `None` before
/// the image parameters are cached.
pub(crate) fn cached_state(snapshot: &CameraStateEvent) -> Option<ProState> {
    let mut state = state_of(snapshot.base_parameters.as_ref()?);
    let config = snapshot
        .actuators_config
        .clone()
        .and_then(|config| serde_json::from_value::<ActuatorsConfig>(config).ok());
    if let Some(config) = config {
        let positions = snapshot
            .actuators_state
            .clone()
            .and_then(|positions| serde_json::from_value(positions).ok())
            .unwrap_or_default();
        (state.focus, state.zoom, state.tilt) = Actuators::from_config(&config).read(positions);
    }
    Some(state)
}

/// What the camera's image pipeline reports now, by field name (see [`isp_info`]).
pub(crate) async fn isp_readings(camera_uuid: Uuid) -> Result<HashMap<String, i64>, ControlError> {
    read_camera(camera_uuid, CameraAction::GetIspInfo)
        .await
        .map(|info| isp_info(&info))
}

/// Applies `settings`, validating everything before anything is written, and answers the
/// new state. Exposure values ramp there (see [`Slew`]); the rest is written at once.
async fn set(camera_uuid: Uuid, settings: ProSettings) -> Result<ProState, ControlError> {
    let touches_camera = settings.mode.is_some()
        || settings.shutter.is_some()
        || settings.iso.is_some()
        || settings.max_iso.is_some()
        || settings.max_shutter.is_some()
        || settings.exposure_compensation.is_some()
        || settings.white_balance.is_some();
    let touches_actuators =
        settings.focus.is_some() || settings.zoom.is_some() || settings.tilt.is_some();
    let setpoints = if touches_actuators {
        let actuators = Actuators::of(camera_uuid)
            .await
            .ok_or_else(|| bad_request("Actuators are not set up on the autopilot"))?;
        Some(actuators.setpoints(&settings)?)
    } else {
        None
    };

    // Actuators alone do not need the camera, which keeps a dragged slider light.
    if touches_camera {
        let slew = Slew::of(camera_uuid);
        let mut slew = slew.lock().await;
        let params = read_camera(camera_uuid, CameraAction::GetImageAdjustment).await?;
        // The whole change is checked here, ramped values included, so a refused one fails
        // the request instead of stopping a ramp later.
        base_parameters(&params, &settings)?;
        let (now, ramp) = match settings.mode {
            Some(mode) if mode != mode_of(&params) => {
                // Best effort: without what the camera uses now, the landing is its last
                // manual values.
                let info = read_camera(camera_uuid, CameraAction::GetIspInfo)
                    .await
                    .ok();
                slew.target = None;
                land(
                    &params,
                    info.map(|info| isp_info(&info)).as_ref(),
                    &settings,
                )
            }
            // The mode it is in already: no change of mode, so its values ramp.
            _ => split_ramp(&ProSettings {
                mode: None,
                ..settings.clone()
            }),
        };
        apply(camera_uuid, &params, &now).await?;
        if let Some(ramp) = ramp {
            slew.target = Some(retarget(slew.target.take(), ramp));
            if !slew.walking {
                slew.walking = true;
                tokio::spawn(walk(camera_uuid));
            }
        }
    }
    if let Some(setpoints) = setpoints {
        autopilot(camera_uuid, AutopilotAction::SetActuatorsState(setpoints)).await?;
    }
    state(camera_uuid).await
}

/// Writes `settings` to the camera at once, given its current `params`.
async fn apply(
    camera_uuid: Uuid,
    params: &Value,
    settings: &ProSettings,
) -> Result<(), ControlError> {
    let mut base = base_parameters(params, settings)?;
    // Only now, with everything validated, so a refused change keeps the kept limit.
    let unlatch = manual_unlatch(params, settings);
    if let Some(manager) = settings.mode.and(::settings::MANAGER.get()) {
        let stored = &mut manager.write().await.settings;
        carry_shutter_limit(
            camera_uuid,
            params,
            settings,
            &mut base,
            stored.get_program_max_exposure_mut(),
        );
        // A failed save shows in the health report; the camera change still goes out.
        if let Err(error) = stored.save().await {
            warn!("Failed saving the kept shutter limit: {error:?}");
        }
    }
    if settings.mode.is_some() {
        // The lens has no driven iris: auto_iris only raises the camera's shortest
        // exposure from 30 to 200 us (firmware), so the full shutter range needs it off.
        let iris = AdvancedParameterSetting {
            auto_iris: Some(AdvancedDisplayAutoIrisValue::Open),
            ..Default::default()
        };
        camera(camera_uuid, CameraAction::SetImageAdjustmentEx(iris)).await?;
    }
    if let Some(unlatch) = unlatch {
        camera(camera_uuid, CameraAction::SetImageAdjustment(unlatch)).await?;
    }
    if base != BaseParameterSetting::default() {
        camera(camera_uuid, CameraAction::SetImageAdjustment(base)).await?;
    }
    Ok(())
}

/// Slew-rate limit on one camera's exposure, for every client, not only the UI: shutter, ISO,
/// compensation and the limits move toward what was asked by at most [`SLEW_STOPS`] a step,
/// a step per [`SLEW_INTERVAL`] at most. A change during a ramp retargets it. Mode changes
/// jump (the UI picks values that keep the picture), and so does white balance. The lock also
/// keeps one change at a time per camera.
#[derive(Default)]
struct Slew {
    /// Where the exposure is ramping to, or None when it is not.
    target: Option<ProSettings>,
    /// Whether a [`walk`] is running for the camera.
    walking: bool,
}

static SLEWS: LazyLock<Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<Slew>>>>> =
    LazyLock::new(Default::default);

impl Slew {
    fn of(camera_uuid: Uuid) -> Arc<tokio::sync::Mutex<Slew>> {
        let mut slews = SLEWS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slews.entry(camera_uuid).or_default().clone()
    }
}

/// `settings` (no mode change) as what is written at once and what ramps: the exposure
/// values ramp, the rest goes at once.
fn split_ramp(settings: &ProSettings) -> (ProSettings, Option<ProSettings>) {
    let mut now = settings.clone();
    let ramp = ProSettings {
        shutter: now.shutter.take(),
        iso: now.iso.take(),
        max_iso: now.max_iso.take(),
        max_shutter: now.max_shutter.take(),
        exposure_compensation: now.exposure_compensation.take(),
        ..Default::default()
    };
    let ramps = ramp != ProSettings::default();
    (now, ramps.then_some(ramp))
}

/// A change of mode as a landing written at once and what ramps from there. The landing is the
/// new mode at the shutter and ISO the camera uses now (`params`, and `info` from getISPInfo
/// where it picks them), so the picture holds: a jump of several stops at once froze one
/// camera's stream for 26 s and dropped another's (measured). The values `settings` ask for
/// ramp from there.
fn land(
    params: &Value,
    info: Option<&HashMap<String, i64>>,
    settings: &ProSettings,
) -> (ProSettings, Option<ProSettings>) {
    let mut current = state_of(params);
    if let Some(info) = info {
        apply_isp_info(&mut current, info);
    }
    let mode = settings.mode;
    let (mut now, ramp) = split_ramp(&ProSettings {
        mode: None,
        ..settings.clone()
    });
    now.mode = mode;
    if mode != Some(ExposureMode::Program) {
        now.shutter = current
            .shutter
            .as_deref()
            .and_then(seconds_of)
            .map(nearest_shutter);
    }
    if mode == Some(ExposureMode::Manual) {
        now.iso = current.iso.map(nearest_iso);
    }
    (now, ramp)
}

/// The camera's shutter closest to `seconds`, in stops.
fn nearest_shutter(seconds: f64) -> String {
    let stops =
        |time: &BaseExposureTimeValue| (f64::from(time.denominator()) * seconds).log2().abs();
    let closest = BaseExposureTimeValue::ALL
        .iter()
        .min_by(|a, b| stops(a).total_cmp(&stops(b)))
        .map_or(1, BaseExposureTimeValue::denominator);
    format!("1/{closest}")
}

/// The manual ISO stop closest to `iso`, in stops.
fn nearest_iso(iso: u32) -> u32 {
    let stops = |stop: u32| (f64::from(stop) / f64::from(iso.max(1))).log2().abs();
    ISO_GAINS
        .iter()
        .map(|(stop, _, _)| *stop)
        .min_by(|a, b| stops(*a).total_cmp(&stops(*b)))
        .unwrap_or(ISO_GAINS[0].0)
}

/// A ramp's target with `newer` over it.
fn retarget(target: Option<ProSettings>, newer: ProSettings) -> ProSettings {
    let target = target.unwrap_or_default();
    ProSettings {
        shutter: newer.shutter.or(target.shutter),
        iso: newer.iso.or(target.iso),
        max_iso: newer.max_iso.or(target.max_iso),
        max_shutter: newer.max_shutter.or(target.max_shutter),
        exposure_compensation: newer.exposure_compensation.or(target.exposure_compensation),
        ..Default::default()
    }
}

/// `state` with the values `target` ramps to.
fn show_target(state: &mut ProState, target: &ProSettings) {
    if target.shutter.is_some() {
        state.shutter = target.shutter.clone();
    }
    if target.iso.is_some() {
        state.iso = target.iso;
    }
    if target.max_iso.is_some() {
        state.max_iso = target.max_iso;
    }
    if target.max_shutter.is_some() {
        state.max_shutter = target.max_shutter.clone();
    }
    if target.exposure_compensation.is_some() {
        state.exposure_compensation = target.exposure_compensation;
    }
}

/// Ramps a camera's exposure to its [`Slew`] target, a step per [`SLEW_INTERVAL`], and stops
/// there, or on an error.
async fn walk(camera_uuid: Uuid) {
    let slew = Slew::of(camera_uuid);
    let mut last_step: Option<ProSettings> = None;
    loop {
        let mut slew_now = slew.lock().await;
        let next = match slew_now.target.clone() {
            None => None,
            Some(target) => {
                match read_camera(camera_uuid, CameraAction::GetImageAdjustment).await {
                    Ok(params) => next_step(&state_of(&params), &target).map(|step| (params, step)),
                    Err(error) => {
                        warn!("Stopped the exposure ramp of {camera_uuid}: {error:?}");
                        None
                    }
                }
            }
        };
        // A step the camera did not take would come back forever.
        let next = next.filter(|(_, step)| last_step.as_ref() != Some(step));
        let Some((params, step)) = next else {
            (slew_now.target, slew_now.walking) = (None, false);
            return;
        };
        if let Err(error) = apply(camera_uuid, &params, &step).await {
            warn!("Stopped the exposure ramp of {camera_uuid}: {error:?}");
            (slew_now.target, slew_now.walking) = (None, false);
            return;
        }
        last_step = Some(step);
        drop(slew_now);
        tokio::time::sleep(SLEW_INTERVAL).await;
    }
}

/// The next step from `state` toward `target`: one value at a time, by at most
/// [`SLEW_STOPS`], or None once every value is there.
fn next_step(state: &ProState, target: &ProSettings) -> Option<ProSettings> {
    let shutters = |calibrated: fn(&BaseExposureTimeValue) -> bool| -> Vec<(String, f64)> {
        BaseExposureTimeValue::ALL
            .iter()
            .filter(|time| calibrated(time))
            .map(|time| {
                let denominator = time.denominator();
                (format!("1/{denominator}"), -f64::from(denominator).log2())
            })
            .collect()
    };
    let shutter_stops = |shutter: &String| seconds_of(shutter).map(f64::log2);

    let all = shutters(|_| true);
    let to = target
        .shutter
        .as_ref()
        .and_then(|t| all.iter().position(|(s, _)| s == t));
    if let Some(shutter) = ramp_step(&all, state.shutter.as_ref().and_then(shutter_stops), to) {
        return Some(ProSettings {
            shutter: Some(shutter),
            ..Default::default()
        });
    }

    let isos: Vec<(u32, f64)> = ISO_GAINS
        .iter()
        .map(|(iso, _, _)| (*iso, f64::from(*iso).log2()))
        .collect();
    let to = target
        .iso
        .and_then(|t| isos.iter().position(|(iso, _)| *iso == t));
    if let Some(iso) = ramp_step(&isos, state.iso.map(|iso| f64::from(iso).log2()), to) {
        return Some(ProSettings {
            iso: Some(iso),
            ..Default::default()
        });
    }

    let evs: Vec<(f32, f64)> = EXPOSURE_COMPENSATION
        .iter()
        .map(|(ev, _)| (*ev, f64::from(*ev)))
        .collect();
    let to = target
        .exposure_compensation
        .and_then(|t| evs.iter().position(|(ev, _)| (ev - t).abs() < 0.01));
    if let Some(ev) = ramp_step(&evs, state.exposure_compensation.map(f64::from), to) {
        return Some(ProSettings {
            exposure_compensation: Some(ev),
            ..Default::default()
        });
    }

    let limit_stops = |limit: &MaxIso| match limit {
        MaxIso::Iso(iso) => f64::from(*iso).log2(),
        // Past the top ISO: one more step.
        MaxIso::Full(_) => f64::from(MAX_ISO_GAINS[MAX_ISO_GAINS.len() - 1].0).log2() + 1.0,
    };
    let limits: Vec<(MaxIso, f64)> = MAX_ISO_GAINS
        .iter()
        .map(|(iso, _)| MaxIso::Iso(*iso))
        .chain([MaxIso::Full(FullGain::Full)])
        .map(|limit| (limit, limit_stops(&limit)))
        .collect();
    let to = target
        .max_iso
        .and_then(|t| limits.iter().position(|(l, _)| *l == t));
    if let Some(limit) = ramp_step(&limits, state.max_iso.as_ref().map(limit_stops), to) {
        return Some(ProSettings {
            max_iso: Some(limit),
            ..Default::default()
        });
    }

    let caps = shutters(calibrated_max_shutter);
    let to = target
        .max_shutter
        .as_ref()
        .and_then(|t| caps.iter().position(|(s, _)| s == t));
    let from = state.max_shutter.as_ref().and_then(shutter_stops);
    ramp_step(&caps, from, to).map(|cap| ProSettings {
        max_shutter: Some(cap),
        ..Default::default()
    })
}

/// [`step_toward`] when both ends are known.
fn ramp_step<T: Clone>(values: &[(T, f64)], from: Option<f64>, to: Option<usize>) -> Option<T> {
    from.zip(to)
        .and_then(|(from, to)| step_toward(values, from, to))
}

/// The value of `values` (each with its position in stops, in order) one step from `from`
/// toward the one at `to`: the farthest within [`SLEW_STOPS`], or the next one when even that
/// is farther. None when `from` is already there.
fn step_toward<T: Clone>(values: &[(T, f64)], from: f64, to: usize) -> Option<T> {
    let at = (0..values.len()).min_by(|&a, &b| {
        (values[a].1 - from)
            .abs()
            .total_cmp(&(values[b].1 - from).abs())
    })?;
    let toward: Vec<usize> = match to.cmp(&at) {
        std::cmp::Ordering::Equal => return None,
        std::cmp::Ordering::Greater => (at + 1..=to).collect(),
        std::cmp::Ordering::Less => (to..at).rev().collect(),
    };
    let within = toward
        .iter()
        .take_while(|&&next| (values[next].1 - values[at].1).abs() <= SLEW_STOPS + 1e-6)
        .last();
    within
        .or(toward.first())
        .map(|&next| values[next].0.clone())
}

/// Seconds of a `"1/x"` or whole-seconds shutter.
fn seconds_of(shutter: &str) -> Option<f64> {
    match shutter.strip_prefix("1/") {
        Some(x) => x.parse::<f64>().ok().map(|x| 1.0 / x),
        None => shutter.parse::<f64>().ok(),
    }
}

async fn capabilities(camera_uuid: Uuid) -> Result<ProCapabilities, ControlError> {
    if mcm_client::camera_address(&camera_uuid).await.is_none() {
        return Err(unknown_camera());
    }
    let actuators = Actuators::of(camera_uuid).await;
    let shutters = |calibrated: fn(&BaseExposureTimeValue) -> bool| -> Vec<String> {
        BaseExposureTimeValue::ALL
            .iter()
            .filter(|time| calibrated(time))
            .map(|time| format!("1/{}", time.denominator()))
            .collect()
    };
    Ok(ProCapabilities {
        modes: MODES.to_vec(),
        shutter: BaseExposureTimeValue::ALL
            .iter()
            .map(|time| format!("1/{}", time.denominator()))
            .collect(),
        shutter_calibrated: shutters(calibrated_shutter),
        max_shutter: shutters(calibrated_max_shutter),
        iso: ISO_GAINS.iter().map(|(iso, _, _)| *iso).collect(),
        max_iso: MAX_ISO_GAINS
            .iter()
            .map(|(iso, _)| MaxIso::Iso(*iso))
            .chain([MaxIso::Full(FullGain::Full)])
            .collect(),
        exposure_compensation: EXPOSURE_COMPENSATION.iter().map(|(ev, _)| *ev).collect(),
        white_balance_kelvin: Range {
            min: f32::from(*KELVIN_RANGE.start()),
            max: f32::from(*KELVIN_RANGE.end()),
        },
        white_balance_scenes: WHITE_BALANCE_SCENES
            .iter()
            .map(|(scene, _)| *scene)
            .collect(),
        focus: actuators.and_then(|actuators| actuators.focus_range()),
        zoom: actuators.and_then(|actuators| actuators.zoom_range()),
        tilt: actuators.and_then(|actuators| actuators.tilt_range()),
        aperture: Aperture::Fixed,
    })
}

/// Focus travel, in %.
const FOCUS_RANGE: Range = Range {
    min: 0.0,
    max: 100.0,
};
/// Zoom magnification across the zoom travel, as the Basic tab shows it.
const ZOOM_RANGE: Range = Range { min: 1.0, max: 2.0 };

/// The actuators the autopilot has set up for a camera. The autopilot positions each one
/// in % of its travel; this converts to and from the units of the PRO API.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Actuators {
    motorized: bool,
    /// Pitch in degrees at 0 % and at 100 % of tilt travel, or `None` without a tilt servo.
    tilt_ends: Option<(f32, f32)>,
}

impl Actuators {
    async fn of(camera_uuid: Uuid) -> Option<Self> {
        read_autopilot::<ActuatorsConfig>(camera_uuid, AutopilotAction::GetActuatorsConfig)
            .await
            .ok()
            .map(|config| Self::from_config(&config))
    }

    fn from_config(config: &ActuatorsConfig) -> Self {
        let parameters = config.parameters.clone().unwrap_or_default();
        // Same defaults as the Basic tab's tilt slider.
        let min = parameters.tilt_mnt_pitch_min.unwrap_or(-90) as f32;
        let max = parameters.tilt_mnt_pitch_max.unwrap_or(90) as f32;
        let reversed = parameters.tilt_channel_reversed.unwrap_or(false);
        Self {
            motorized: parameters.lens != Some(LensType::FixedFocus),
            tilt_ends: parameters
                .tilt_channel
                .map(|_| if reversed { (max, min) } else { (min, max) }),
        }
    }

    fn focus_range(&self) -> Option<Range> {
        self.motorized.then_some(FOCUS_RANGE)
    }

    fn zoom_range(&self) -> Option<Range> {
        self.motorized.then_some(ZOOM_RANGE)
    }

    fn tilt_range(&self) -> Option<Range> {
        self.tilt_ends.map(|(start, end)| Range {
            min: start.min(end),
            max: start.max(end),
        })
    }

    /// (focus, zoom, tilt) of the servo `positions`, each `None` when not set up.
    fn read(&self, positions: ActuatorsState) -> (Option<f32>, Option<f32>, Option<f32>) {
        let focus = positions.focus.filter(|_| self.motorized);
        let zoom = positions
            .zoom
            .filter(|_| self.motorized)
            .map(|percent| ZOOM_RANGE.min + percent / 100.0 * (ZOOM_RANGE.max - ZOOM_RANGE.min));
        let tilt = positions
            .tilt
            .zip(self.tilt_ends)
            .map(|(percent, (start, end))| start + percent / 100.0 * (end - start));
        (focus, zoom, tilt)
    }

    /// Servo positions for the actuator fields of `settings`, validated against what is set up.
    fn setpoints(&self, settings: &ProSettings) -> Result<ActuatorsState, ControlError> {
        let within = |name: &str, value: f32, range: Option<Range>| match range {
            None => Err(bad_request(format!("{name} is not set up on this camera"))),
            Some(range) if !(range.min..=range.max).contains(&value) => Err(bad_request(format!(
                "{name} must be between {} and {}",
                range.min, range.max
            ))),
            Some(range) => Ok(range),
        };
        let mut setpoints = ActuatorsState::default();
        if let Some(focus) = settings.focus {
            within("Focus", focus, self.focus_range())?;
            setpoints.focus = Some(focus);
        }
        if let Some(zoom) = settings.zoom {
            let range = within("Zoom", zoom, self.zoom_range())?;
            setpoints.zoom = Some(100.0 * (zoom - range.min) / (range.max - range.min));
        }
        if let (Some(tilt), Some((start, end))) = (settings.tilt, self.tilt_ends) {
            within("Tilt", tilt, self.tilt_range())?;
            setpoints.tilt = Some(if start == end {
                0.0
            } else {
                100.0 * (tilt - start) / (end - start)
            });
        } else if settings.tilt.is_some() {
            return Err(bad_request("Tilt is not set up on this camera"));
        }
        Ok(setpoints)
    }
}

/// Starts a one-push white balance; it settles in the background.
async fn one_push(camera_uuid: Uuid) -> Result<(), ControlError> {
    let action = CameraAction::SetImageAdjustmentEx(AdvancedParameterSetting {
        once_awb: Some(1),
        ..Default::default()
    });
    camera(camera_uuid, action).await.map(|_| ())
}

fn unknown_camera() -> ControlError {
    ControlError {
        status: 404,
        message: UNKNOWN_CAMERA.to_string(),
    }
}

fn internal(error: impl std::fmt::Debug) -> ControlError {
    ControlError {
        status: 500,
        message: format!("{error:?}"),
    }
}

/// Reads skip [`control_bridge`]: like the state watchers, a failed read (an autopilot still
/// starting, say) must not raise warnings on every client.
async fn read_camera(camera_uuid: Uuid, action: CameraAction) -> Result<Value, ControlError> {
    if mcm_client::camera_address(&camera_uuid).await.is_none() {
        return Err(unknown_camera());
    }
    br4kcam_commands::handle_control(CameraControl {
        camera_uuid,
        action,
    })
    .await
    .map_err(internal)
}

async fn read_autopilot<T: DeserializeOwned>(
    camera_uuid: Uuid,
    action: AutopilotAction,
) -> Result<T, ControlError> {
    let value = autopilot::handle_control(ActuatorsControl {
        camera_uuid,
        action,
    })
    .await
    .map_err(internal)?;
    serde_json::from_value(value).map_err(internal)
}

async fn camera(camera_uuid: Uuid, action: CameraAction) -> Result<Value, ControlError> {
    control_bridge::camera_control(CameraControl {
        camera_uuid,
        action,
    })
    .await
}

async fn autopilot(camera_uuid: Uuid, action: AutopilotAction) -> Result<Value, ControlError> {
    control_bridge::autopilot_control(ActuatorsControl {
        camera_uuid,
        action,
    })
    .await
}

/// A numeric field of a camera reply. Read raw, so a value the protocol enums do not
/// know cannot fail the whole read.
fn field(params: &Value, name: &str) -> u8 {
    params.get(name).and_then(Value::as_u64).unwrap_or(0) as u8
}

/// The protocol value meaning Manual in `auto_exposureEx`, `auto_gain_mode` and `auto_awb`.
const MANUAL: u8 = 1;

/// The protocol value meaning on in `manual_AGain_enable` and `manual_DGain_enable`.
const ENABLED: u8 = 1;

/// With auto exposure on (`auto_exposureEx` 0) the camera picks shutter and gain and ignores
/// the manual ones. With it off the shutter is manual, and so is each gain whose manual
/// switch is on; with the switches off auto exposure drives the gain: shutter priority
/// (measured; the firmware ignores `auto_gain_mode`).
fn mode_of(params: &Value) -> ExposureMode {
    if field(params, "auto_exposureEx") != MANUAL {
        ExposureMode::Program
    } else if field(params, "manual_AGain_enable") == ENABLED {
        ExposureMode::Manual
    } else {
        ExposureMode::Shutter
    }
}

/// The ISO stop closest to the manual gains.
fn iso_of(params: &Value) -> u32 {
    let gain = |enable: &str, value: &str| {
        if field(params, enable) == 1 {
            i32::from(field(params, value))
        } else {
            0
        }
    };
    let analog = gain("manual_AGain_enable", "manual_AGain");
    let digital = gain("manual_DGain_enable", "manual_DGain");
    ISO_GAINS
        .iter()
        .min_by_key(|(_, a, d)| (analog - i32::from(*a)).abs() + (digital - i32::from(*d)).abs())
        .map(|(iso, _, _)| *iso)
        .unwrap_or(ISO_GAINS[0].0)
}

/// `getISPInfo`'s `isp_info` text (`"ISP Info: ISO:2089 AGain:21394 ... ColorTemp:5847 "`)
/// as name and value pairs. Fields can be missing from a read.
fn isp_info(response: &Value) -> HashMap<String, i64> {
    let text = response
        .get("isp_info")
        .and_then(Value::as_str)
        .unwrap_or_default();
    text.split_whitespace()
        .filter_map(|pair| pair.split_once(':'))
        .filter_map(|(name, value)| Some((name.to_string(), value.parse().ok()?)))
        .collect()
}

/// What the camera is using now, where it picks it: shutter in Program, ISO in Program and
/// Shutter, and how far auto exposure is off its target. Manual has no auto exposure (its
/// error reads a constant 50), so nothing there.
fn apply_isp_info(state: &mut ProState, info: &HashMap<String, i64>) {
    state.color_temperature = info.get("ColorTemp").and_then(|&k| u32::try_from(k).ok());
    if state.mode == ExposureMode::Manual {
        return;
    }
    if state.mode == ExposureMode::Program {
        state.shutter = info
            .get("ExpTime")
            .and_then(|&us| u64::try_from(us).ok())
            .filter(|&us| us > 0)
            .map(shutter_of_micros);
    }
    state.iso = info
        .get("ISO")
        .and_then(|&iso| u64::try_from(iso).ok())
        .map(iso_of_camera);
    state.exposure_warning = match info.get("HistError") {
        Some(&error) if error > EXPOSURE_ERROR_LIMIT => Some(ExposureWarning::TooDark),
        Some(&error) if error < -EXPOSURE_ERROR_LIMIT => Some(ExposureWarning::TooBright),
        _ => None,
    };
}

/// `"1/<x>"` for an exposure time in microseconds, or whole seconds from one second up.
pub(crate) fn shutter_of_micros(micros: u64) -> String {
    if micros >= 1_000_000 {
        format!("{}", (micros as f64 / 1e6).round())
    } else {
        format!("1/{}", (1e6 / micros as f64).round())
    }
}

/// The camera's ISO on our scale (see `CAMERA_ISO`), extrapolated past both ends.
pub(crate) fn iso_of_camera(camera_iso: u64) -> u32 {
    interpolate_iso(
        CAMERA_ISO.map(|(camera, ours)| (camera, ours)),
        camera_iso as f64,
    )
}

/// The camera's ISO for an ISO on our scale: the inverse of [`iso_of_camera`].
fn camera_iso_of(iso: u32) -> u32 {
    interpolate_iso(CAMERA_ISO.map(|(camera, ours)| (ours, camera)), iso.into())
}

/// `x` through `points`, interpolated in log(ISO) and extrapolated past both ends.
fn interpolate_iso(points: [(u32, u32); 12], x: f64) -> u32 {
    let log = |x: f64| x.max(1.0).log2();
    let x = log(x);
    let points = points.map(|(from, to)| (log(from.into()), log(to.into())));
    let segment = points
        .windows(2)
        .find(|pair| x <= pair[1].0)
        .unwrap_or(&points[points.len() - 2..]);
    let ((x0, y0), (x1, y1)) = (segment[0], segment[1]);
    let iso = 2f64.powf(y0 + (x - x0) * (y1 - y0) / (x1 - x0));
    iso.round().clamp(1.0, u32::MAX.into()) as u32
}

/// `auto_AGain_max` and `auto_DGain_max` that hold shutter priority's gain to `max_iso`. In
/// that mode they are the ceiling, not `max_sys_gain` (measured: 58/2, 95/12 and 100/33
/// stopped it at camera ISO 8630, 83980 and 247464, for ISO 6400, 51200 and 204800). The
/// firmware applies `auto_AGain_max` as 0.75x its value and `auto_DGain_max` as its value;
/// analog gain comes first, as in auto exposure.
fn shutter_gain_caps(max_iso: MaxIso) -> (u8, u8) {
    let MaxIso::Iso(iso) = max_iso else {
        return (u8::MAX, u8::MAX);
    };
    let gain = (f64::from(camera_iso_of(iso)) / 100.0).max(1.0);
    let digital = (gain / MAX_ANALOG_GAIN).ceil().clamp(1.0, 255.0);
    let analog = (gain / digital / 0.75).round().clamp(1.0, 255.0);
    (analog as u8, digital as u8)
}

/// The camera's `max_sys_gain` for a maximum ISO.
fn sys_gain_of(max_iso: MaxIso) -> Result<u8, ControlError> {
    match max_iso {
        MaxIso::Full(_) => Ok(FULL_SYS_GAIN),
        MaxIso::Iso(iso) => MAX_ISO_GAINS
            .iter()
            .find(|(stop, _)| *stop == iso)
            .map(|(_, gain)| *gain)
            .ok_or_else(|| bad_request(format!("Unsupported maximum ISO {iso}, see capabilities"))),
    }
}

/// The exposure compensation closest to the camera's `blc_level`.
fn exposure_compensation_of(params: &Value) -> Option<f32> {
    let level = i32::from(field(params, "blc_level"));
    EXPOSURE_COMPENSATION
        .iter()
        .min_by_key(|(_, at)| (i32::from(*at) - level).abs())
        .map(|(ev, _)| *ev)
}

/// The highest maximum ISO the camera's `max_sys_gain` allows: full gain above the last
/// calibrated one.
fn max_iso_of(params: &Value) -> MaxIso {
    let gain = field(params, "max_sys_gain");
    match MAX_ISO_GAINS.last() {
        Some((_, top)) if gain > *top => MaxIso::Full(FullGain::Full),
        _ => MAX_ISO_GAINS
            .iter()
            .take_while(|(_, needs)| *needs <= gain)
            .last()
            .map(|(iso, _)| MaxIso::Iso(*iso))
            .unwrap_or(MaxIso::Iso(MAX_ISO_GAINS[0].0)),
    }
}

fn white_balance_of(params: &Value) -> WhiteBalance {
    // The camera holds the scene in Manual too: it is the one Auto returns to.
    let scene = WHITE_BALANCE_SCENES
        .get(usize::from(field(params, "awb_auto_mode")))
        .unwrap_or(&WHITE_BALANCE_SCENES[0])
        .0;
    if field(params, "auto_awb") == MANUAL {
        WhiteBalance::Manual {
            red: Some(field(params, "awb_red")),
            green: Some(field(params, "awb_green")),
            blue: Some(field(params, "awb_blue")),
            scene: Some(scene),
        }
    } else if field(params, "auto_awb") == TEMPERATURE {
        // The camera clamps what it holds into its range when it applies it.
        let raw = params.get("wb_temp").and_then(Value::as_u64).unwrap_or(0);
        let (low, high) = (*KELVIN_RANGE.start(), *KELVIN_RANGE.end());
        WhiteBalance::Temperature {
            kelvin: Some(raw.clamp(low.into(), high.into()) as u32),
            scene: Some(scene),
        }
    } else {
        WhiteBalance::Auto { scene: Some(scene) }
    }
}

/// The protocol value meaning Temperature in `auto_awb`.
const TEMPERATURE: u8 = 2;

/// The colour temperature the camera holds, when it is in range.
fn kelvin_of(params: &Value) -> Option<u16> {
    params
        .get("wb_temp")
        .and_then(Value::as_u64)
        .and_then(|kelvin| u16::try_from(kelvin).ok())
        .filter(|kelvin| KELVIN_RANGE.contains(kelvin))
}

/// The camera's value for a water scene.
fn scene_value(scene: WhiteBalanceScene) -> Option<BaseAutoWhiteBalanceSceneValue> {
    WHITE_BALANCE_SCENES
        .iter()
        .find(|(known, _)| *known == scene)
        .map(|(_, value)| value.clone())
}

/// The camera's side of the state, without actuators.
fn state_of(params: &Value) -> ProState {
    let mode = mode_of(params);
    let manual = mode == ExposureMode::Manual;
    let program = mode == ExposureMode::Program;
    ProState {
        mode,
        shutter: (!program).then(|| shutter_of(params, "exposure_time")),
        iso: manual.then(|| iso_of(params)),
        max_iso: (!manual).then(|| max_iso_of(params)),
        max_shutter: program.then(|| shutter_of(params, "max_exposure")),
        exposure_compensation: if manual {
            None
        } else {
            exposure_compensation_of(params)
        },
        exposure_warning: None,
        color_temperature: None,
        white_balance: white_balance_of(params),
        focus: None,
        zoom: None,
        tilt: None,
    }
}

/// The camera's exposure time field `name` (`exposure_time` or `max_exposure`) as `"1/x"`. A
/// value the protocol does not list (a camera can hold one) is shown as the camera reports it.
fn shutter_of(params: &Value, name: &str) -> String {
    let raw = params.get(name).cloned().unwrap_or(json!(0));
    match serde_json::from_value::<BaseExposureTimeValue>(raw.clone()) {
        Ok(time) => format!("1/{}", time.denominator()),
        Err(_) => format!("1/{raw}"),
    }
}

fn parse_shutter(text: &str) -> Result<BaseExposureTimeValue, ControlError> {
    text.strip_prefix("1/")
        .and_then(|x| x.parse::<u32>().ok())
        .and_then(|x| {
            BaseExposureTimeValue::ALL
                .into_iter()
                .find(|time| time.denominator() == x)
        })
        .ok_or_else(|| bad_request(format!("Unsupported shutter {text:?}, see capabilities")))
}

/// The camera's `max_exposure` for a maximum shutter: calibrated ones only.
fn max_exposure_of(text: &str) -> Result<BaseMaxExposureValue, ControlError> {
    let time = parse_shutter(text)?;
    calibrated_max_shutter(&time)
        .then(|| serde_json::from_value(json!(time.denominator())).ok())
        .flatten()
        .ok_or_else(|| {
            bad_request(format!(
                "Unsupported maximum shutter {text:?}, see capabilities"
            ))
        })
}

/// The camera holds a manual shutter to the `max_exposure` it had when Manual was entered:
/// with 1/1000 there, 1/30 and 1/250 both expose 1/1000 s. Changing it in Manual, or in the
/// request that enters it, does not help; changing it in an earlier request does (measured;
/// the firmware writes it only while applying auto exposure). So switching to Manual or
/// Shutter (the camera's manual exposure too) first lifts it to the longest, 1 s, in a
/// request of its own.
fn manual_unlatch(params: &Value, settings: &ProSettings) -> Option<BaseParameterSetting> {
    let longest = params.get("max_exposure").and_then(Value::as_u64) == Some(1);
    let manual_exposure = settings
        .mode
        .is_some_and(|mode| mode != ExposureMode::Program);
    (manual_exposure && !longest).then(|| BaseParameterSetting {
        max_exposure: Some(BaseMaxExposureValue::_1),
        ..Default::default()
    })
}

/// Leaving Program lifts its shutter limit (see `manual_unlatch`), so it is kept in `kept`
/// (the settings file, as the camera's raw `max_exposure`, so it survives a restart) when
/// `settings` take the camera from Program to another mode, and put back in `base` when they
/// take it back to Program, unless they set their own; with none kept, Program gets
/// [`PROGRAM_DEFAULT_MAX_EXPOSURE`].
fn carry_shutter_limit(
    camera_uuid: Uuid,
    params: &Value,
    settings: &ProSettings,
    base: &mut BaseParameterSetting,
    kept: &mut IndexMap<Uuid, u32>,
) {
    match (mode_of(params), settings.mode) {
        (ExposureMode::Program, Some(mode)) if mode != ExposureMode::Program => {
            let limit = params.get("max_exposure").and_then(Value::as_u64);
            match limit.and_then(|limit| u32::try_from(limit).ok()) {
                Some(limit) => kept.insert(camera_uuid, limit),
                None => kept.shift_remove(&camera_uuid),
            };
        }
        (mode, Some(ExposureMode::Program)) if mode != ExposureMode::Program => {
            let limit = kept
                .shift_remove(&camera_uuid)
                .and_then(|raw| serde_json::from_value(json!(raw)).ok());
            base.max_exposure = base
                .max_exposure
                .take()
                .or(limit)
                .or(Some(PROGRAM_DEFAULT_MAX_EXPOSURE));
        }
        _ => {}
    }
}

/// Camera parameters for `settings`, given the camera's current `params`.
fn base_parameters(
    params: &Value,
    settings: &ProSettings,
) -> Result<BaseParameterSetting, ControlError> {
    let mode = settings.mode.unwrap_or_else(|| mode_of(params));
    let manual = mode == ExposureMode::Manual;
    let program = mode == ExposureMode::Program;
    let mut base = BaseParameterSetting::default();

    if settings.mode.is_some() {
        base.auto_exposure_ex = Some(match program {
            true => BaseExposureModeValue::Auto,
            false => BaseExposureModeValue::Manual,
        });
        // Ignored by the firmware; kept in step for the camera's own pages.
        base.auto_gain_mode = Some(match manual {
            true => BaseAutoGainModeValue::Manual,
            false => BaseAutoGainModeValue::Auto,
        });
        // In the camera's manual exposure, the gain switches pick Manual or Shutter.
        if !program {
            (base.manual_a_gain_enable, base.manual_d_gain_enable) = match manual {
                true => (
                    Some(BaseManualAGainEnableValue::Open),
                    Some(BaseManualDGainEnableValue::Open),
                ),
                false => (
                    Some(BaseManualAGainEnableValue::Close),
                    Some(BaseManualDGainEnableValue::Close),
                ),
            };
        }
    }

    if let Some(shutter) = &settings.shutter {
        if program {
            return Err(bad_request(format!(
                "Shutter is automatic in {mode:?} mode"
            )));
        }
        base.exposure_time = Some(parse_shutter(shutter)?);
    }

    if let Some(iso) = settings.iso {
        if !manual {
            return Err(bad_request(format!("ISO is automatic in {mode:?} mode")));
        }
        let (_, analog, digital) = ISO_GAINS
            .iter()
            .find(|(stop, _, _)| *stop == iso)
            .ok_or_else(|| bad_request(format!("Unsupported ISO {iso}, see capabilities")))?;
        base.manual_a_gain_enable = Some(BaseManualAGainEnableValue::Open);
        base.manual_a_gain = Some(*analog);
        base.manual_d_gain_enable = Some(BaseManualDGainEnableValue::Open);
        base.manual_d_gain = Some(*digital);
    }

    if let Some(ev) = settings.exposure_compensation {
        if manual {
            return Err(bad_request(
                "Exposure compensation is for modes where the camera picks the exposure",
            ));
        }
        let (_, level) = EXPOSURE_COMPENSATION
            .iter()
            .find(|(stop, _)| (stop - ev).abs() < 0.01)
            .ok_or_else(|| {
                bad_request(format!(
                    "Unsupported exposure compensation {ev}, see capabilities"
                ))
            })?;
        base.blc_level = Some(*level);
    }

    // The maximum ISO is kept in max_sys_gain for both Program and Shutter. Shutter priority
    // entered with none gets a default, so it does not climb into noise.
    let mut max_iso = settings.max_iso;
    if let Some(limit) = max_iso {
        if manual {
            return Err(bad_request(
                "Maximum ISO is for modes where the camera picks ISO",
            ));
        }
        base.max_sys_gain = Some(sys_gain_of(limit)?);
    } else if settings.mode == Some(ExposureMode::Shutter)
        && max_iso_of(params) == MaxIso::Full(FullGain::Full)
    {
        max_iso = Some(MaxIso::Iso(SHUTTER_DEFAULT_MAX_ISO));
        base.max_sys_gain = Some(sys_gain_of(MaxIso::Iso(SHUTTER_DEFAULT_MAX_ISO))?);
    }

    if let Some(shutter) = &settings.max_shutter {
        if !program {
            return Err(bad_request(
                "Maximum shutter is for Program mode, where the camera picks the shutter",
            ));
        }
        base.max_exposure = Some(max_exposure_of(shutter)?);
    }

    // auto_AGain_max and auto_DGain_max cap auto exposure's gain (measured: 32 and 0 stop
    // Program near camera ISO 2400 whatever max_sys_gain is) and clamp the manual gains as
    // well (with 32 and 0, ISO 3200 and up fall short). Program and Manual open them fully,
    // leaving max_sys_gain and the ISO table as the limits; in Shutter they are the limit.
    if settings.mode.is_some() || max_iso.is_some() || settings.iso.is_some() {
        let (analog, digital) = match mode {
            ExposureMode::Shutter => {
                shutter_gain_caps(max_iso.unwrap_or_else(|| max_iso_of(params)))
            }
            _ => (u8::MAX, u8::MAX),
        };
        base.auto_a_gain_max = Some(analog);
        base.auto_d_gain_max = Some(digital);
    }

    match settings.white_balance {
        Some(WhiteBalance::Auto { scene }) => {
            base.auto_awb = Some(BaseAutoWhiteBalanceModeValue::Auto);
            // No scene keeps the camera's, which it holds while in Manual.
            base.awb_auto_mode = scene.and_then(scene_value);
        }
        Some(WhiteBalance::Manual {
            red,
            green,
            blue,
            scene,
        }) => {
            base.auto_awb = Some(BaseAutoWhiteBalanceModeValue::Manual);
            base.awb_auto_mode = scene.and_then(scene_value);
            base.awb_red = red;
            base.awb_green = green;
            base.awb_blue = blue;
        }
        Some(WhiteBalance::Temperature { kelvin, scene }) => {
            let kelvin = match kelvin {
                Some(kelvin) => u16::try_from(kelvin)
                    .ok()
                    .filter(|kelvin| KELVIN_RANGE.contains(kelvin))
                    .ok_or_else(|| {
                        bad_request(format!(
                            "Unsupported colour temperature {kelvin} K, see capabilities"
                        ))
                    })?,
                None => kelvin_of(params).unwrap_or(DEFAULT_KELVIN),
            };
            base.auto_awb = Some(BaseAutoWhiteBalanceModeValue::Temperature);
            base.awb_auto_mode = scene.and_then(scene_value);
            base.wb_temp = Some(kelvin);
        }
        None => {}
    }

    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera_params() -> Value {
        json!({
            "auto_exposureEx": 1,
            "exposure_time": 1000,
            "auto_gain_mode": 1,
            "manual_AGain_enable": 1,
            "manual_AGain": 26,
            "manual_DGain_enable": 0,
            "manual_DGain": 99,
            "auto_awb": 0,
            "awb_auto_mode": 1,
        })
    }

    #[test]
    fn every_shutter_is_a_protocol_value() {
        for time in BaseExposureTimeValue::ALL {
            let text = format!("1/{}", time.denominator());
            // Every value reads back and can be set, calibrated or not.
            assert_eq!(parse_shutter(&text).unwrap(), time, "{text}");
            assert_eq!(
                shutter_of(&json!({ "exposure_time": time.clone() }), "exposure_time"),
                text
            );
        }
        let calibrated = |text: &str| calibrated_shutter(&parse_shutter(text).unwrap());
        assert!(calibrated("1/30") && calibrated("1/4000") && calibrated("1/10000"));
        assert!(!calibrated("1/25") && !calibrated("1/100000"));
        // The camera's number for 1/100000 s is not a shutter of its own.
        assert!(parse_shutter("1/34464").is_err());
        assert_eq!(
            shutter_of(&json!({ "exposure_time": 34464 }), "exposure_time"),
            "1/100000"
        );
        assert_eq!(
            shutter_of(&json!({ "exposure_time": 1 }), "exposure_time"),
            "1/1"
        );
        assert!(parse_shutter("1/123").is_err());
        assert!(parse_shutter("250").is_err());
    }

    #[test]
    fn reads_the_camera_as_pro_state() {
        let state = state_of(&camera_params());
        assert_eq!(state.mode, ExposureMode::Manual);
        assert_eq!(state.shutter.as_deref(), Some("1/1000"));
        // A gain 26 with D gain disabled is closest to the ISO 400 stop.
        assert_eq!(state.iso, Some(400));
        assert_eq!(
            state.white_balance,
            WhiteBalance::Auto {
                scene: Some(WhiteBalanceScene::BlueWater)
            }
        );

        // Auto exposure with manual gain behaves as Program: the camera ignores the gains.
        let mixed = state_of(&json!({"auto_exposureEx": 0, "auto_gain_mode": 1}));
        assert_eq!((mixed.mode, mixed.iso), (ExposureMode::Program, None));

        let program = state_of(&json!({}));
        assert_eq!(program.mode, ExposureMode::Program);
        assert_eq!((program.shutter, program.iso), (None, None));
    }

    #[test]
    fn maps_pro_settings_to_camera_parameters() {
        let settings = ProSettings {
            mode: Some(ExposureMode::Manual),
            shutter: Some("1/1000".into()),
            white_balance: Some(WhiteBalance::Manual {
                red: Some(1),
                green: None,
                blue: Some(3),
                scene: None,
            }),
            ..Default::default()
        };
        let base = base_parameters(&camera_params(), &settings).unwrap();
        assert_eq!(base.auto_exposure_ex, Some(BaseExposureModeValue::Manual));
        assert_eq!(base.auto_gain_mode, Some(BaseAutoGainModeValue::Manual));
        assert_eq!(base.exposure_time, Some(BaseExposureTimeValue::_1000));
        assert_eq!(base.auto_awb, Some(BaseAutoWhiteBalanceModeValue::Manual));
        assert_eq!(
            (base.awb_red, base.awb_green, base.awb_blue),
            (Some(1), None, Some(3))
        );

        let iso = ProSettings {
            iso: Some(800),
            white_balance: Some(WhiteBalance::Auto {
                scene: Some(WhiteBalanceScene::GreenWater),
            }),
            ..Default::default()
        };
        let base = base_parameters(&camera_params(), &iso).unwrap();
        assert_eq!(
            (base.manual_a_gain, base.manual_d_gain),
            (Some(40), Some(0))
        );
        // The auto gain caps clamp manual gains too, so setting an ISO opens them.
        assert_eq!(
            (base.auto_a_gain_max, base.auto_d_gain_max),
            (Some(u8::MAX), Some(u8::MAX))
        );
        assert_eq!(
            base.awb_auto_mode,
            Some(BaseAutoWhiteBalanceSceneValue::Scene1)
        );
    }

    #[test]
    fn maps_max_iso_to_the_auto_gain_ceiling() {
        let program = json!({ "auto_exposureEx": 0, "max_sys_gain": 12 });
        // Reported as the highest maximum ISO the camera's ceiling allows.
        assert_eq!(state_of(&program).max_iso, Some(MaxIso::Iso(25600)));
        // Past the calibrated ceiling (51, ISO 204800) it is full gain, written as 255.
        for gain in [52, 120, 255] {
            let read = state_of(&json!({ "max_sys_gain": gain })).max_iso;
            assert_eq!(read, Some(MaxIso::Full(FullGain::Full)), "{gain}");
        }
        assert_eq!(sys_gain_of(MaxIso::Full(FullGain::Full)).unwrap(), 255);
        assert_eq!(
            state_of(&json!({ "max_sys_gain": 0 })).max_iso,
            Some(MaxIso::Iso(100))
        );
        assert_eq!(state_of(&camera_params()).max_iso, None);

        let settings = ProSettings {
            max_iso: Some(MaxIso::Iso(102400)),
            ..Default::default()
        };
        let base = base_parameters(&program, &settings).unwrap();
        assert_eq!(base.max_sys_gain, Some(33));
        // The camera's own auto gain caps are opened, or they would cap first.
        assert_eq!(
            (base.auto_a_gain_max, base.auto_d_gain_max),
            (Some(255), Some(255))
        );
        let manual = ProSettings {
            mode: Some(ExposureMode::Manual),
            ..Default::default()
        };
        // Manual opens them too: they clamp the manual gains as well.
        let base = base_parameters(&program, &manual).unwrap();
        assert_eq!(
            (base.auto_a_gain_max, base.auto_d_gain_max),
            (Some(255), Some(255))
        );
        // Switching to Program in the same change is fine.
        let switching = ProSettings {
            mode: Some(ExposureMode::Program),
            max_iso: Some(MaxIso::Iso(6400)),
            ..Default::default()
        };
        assert_eq!(
            base_parameters(&camera_params(), &switching)
                .unwrap()
                .max_sys_gain,
            Some(2)
        );
        for (iso, gain) in MAX_ISO_GAINS {
            let settings = ProSettings {
                max_iso: Some(MaxIso::Iso(iso)),
                ..Default::default()
            };
            let base = base_parameters(&program, &settings).unwrap();
            assert_eq!(
                state_of(&json!({ "max_sys_gain": base.max_sys_gain })).max_iso,
                Some(MaxIso::Iso(iso)),
                "{gain}"
            );
        }
    }

    #[test]
    fn maps_white_balance_by_colour_temperature() {
        let read = |params: Value| state_of(&params).white_balance;
        assert_eq!(
            read(json!({ "auto_awb": 2, "wb_temp": 4200, "awb_auto_mode": 0 })),
            WhiteBalance::Temperature {
                kelvin: Some(4200),
                scene: Some(WhiteBalanceScene::GreenWater)
            }
        );
        // The camera applies what it holds clamped into its range.
        let held = read(json!({ "auto_awb": 2, "wb_temp": 0 }));
        assert!(matches!(
            held,
            WhiteBalance::Temperature {
                kelvin: Some(1500),
                ..
            }
        ));

        let to = |kelvin: Option<u32>| ProSettings {
            white_balance: Some(WhiteBalance::Temperature {
                kelvin,
                scene: Some(WhiteBalanceScene::BlueWater),
            }),
            ..Default::default()
        };
        let base = base_parameters(&camera_params(), &to(Some(5600))).unwrap();
        assert_eq!(
            base.auto_awb,
            Some(BaseAutoWhiteBalanceModeValue::Temperature)
        );
        assert_eq!(base.wb_temp, Some(5600));
        assert_eq!(
            base.awb_auto_mode,
            Some(BaseAutoWhiteBalanceSceneValue::Scene2)
        );
        for kelvin in [1499, 15001] {
            let refused = base_parameters(&camera_params(), &to(Some(kelvin)));
            assert_eq!(refused.unwrap_err().status, 400, "{kelvin}");
        }
        // Left out: the camera's when in range, else daylight.
        let holding = json!({ "auto_awb": 0, "wb_temp": 3200 });
        assert_eq!(
            base_parameters(&holding, &to(None)).unwrap().wb_temp,
            Some(3200)
        );
        assert_eq!(
            base_parameters(&camera_params(), &to(None))
                .unwrap()
                .wb_temp,
            Some(5500)
        );
        // White balance is written at once, never ramped.
        assert_eq!(split_ramp(&to(Some(5600))).1, None);
    }

    fn shutter_params() -> Value {
        json!({
            "auto_exposureEx": 1,
            "exposure_time": 250,
            "manual_AGain_enable": 0,
            "manual_DGain_enable": 0,
            "max_sys_gain": 18,
            "max_exposure": 1,
            "blc_level": 50,
        })
    }

    #[test]
    fn reads_shutter_priority() {
        let state = state_of(&shutter_params());
        assert_eq!(state.mode, ExposureMode::Shutter);
        // The shutter is set; ISO comes from the camera, which picks it.
        assert_eq!(state.shutter.as_deref(), Some("1/250"));
        assert_eq!(state.iso, None);
        assert_eq!(state.max_iso, Some(MaxIso::Iso(51200)));
        assert_eq!(state.max_shutter, None);
        assert_eq!(state.exposure_compensation, Some(0.0));

        let mut live = state.clone();
        let info = isp_info(&json!({
            "isp_info": "ISP Info: ISO:85334 ExpTime:3998 HistError:116 ColorTemp:5000 "
        }));
        apply_isp_info(&mut live, &info);
        assert_eq!(live.iso, Some(51200));
        assert_eq!(live.shutter.as_deref(), Some("1/250"), "set, not measured");
        assert_eq!(live.exposure_warning, Some(ExposureWarning::TooDark));
        let bright = isp_info(&json!({ "isp_info": "ISP Info: ISO:100 HistError:-80 " }));
        apply_isp_info(&mut live, &bright);
        assert_eq!(live.exposure_warning, Some(ExposureWarning::TooBright));
        let settled = isp_info(&json!({ "isp_info": "ISP Info: ISO:100 HistError:-9 " }));
        apply_isp_info(&mut live, &settled);
        assert_eq!(live.exposure_warning, None);

        // Manual has no auto exposure: its constant error is no warning.
        let mut manual = state_of(&camera_params());
        let constant = isp_info(&json!({ "isp_info": "ISP Info: ISO:100 HistError:50 " }));
        apply_isp_info(&mut manual, &constant);
        assert_eq!((manual.exposure_warning, manual.iso), (None, Some(400)));
    }

    #[test]
    fn caps_shutter_priority_gain_at_the_maximum_iso() {
        // Measured on the camera: these caps stopped it at the camera ISO each maximum ISO is.
        assert_eq!(shutter_gain_caps(MaxIso::Iso(6400)), (58, 2));
        assert_eq!(shutter_gain_caps(MaxIso::Iso(51200)), (95, 12));
        assert_eq!(shutter_gain_caps(MaxIso::Iso(204800)), (99, 33));
        assert_eq!(shutter_gain_caps(MaxIso::Iso(100)), (1, 1));
        assert_eq!(shutter_gain_caps(MaxIso::Full(FullGain::Full)), (255, 255));
        for (camera, ours) in CAMERA_ISO {
            assert_eq!(camera_iso_of(ours), camera);
            assert_eq!(iso_of_camera(camera.into()), ours);
        }

        let entering = ProSettings {
            mode: Some(ExposureMode::Shutter),
            shutter: Some("1/250".into()),
            ..Default::default()
        };
        let program = json!({ "auto_exposureEx": 0, "max_sys_gain": 18 });
        let base = base_parameters(&program, &entering).unwrap();
        assert_eq!(base.auto_exposure_ex, Some(BaseExposureModeValue::Manual));
        assert_eq!(
            (base.manual_a_gain_enable, base.manual_d_gain_enable),
            (
                Some(BaseManualAGainEnableValue::Close),
                Some(BaseManualDGainEnableValue::Close)
            )
        );
        assert_eq!(base.exposure_time, Some(BaseExposureTimeValue::_250));
        // Program's maximum ISO (51200) carries over, as the caps.
        assert_eq!(base.max_sys_gain, None);
        assert_eq!(
            (base.auto_a_gain_max, base.auto_d_gain_max),
            (Some(95), Some(12))
        );

        // Entered with no maximum ISO (full gain), it gets the default.
        let full = json!({ "auto_exposureEx": 0, "max_sys_gain": 255 });
        let base = base_parameters(&full, &entering).unwrap();
        assert_eq!(base.max_sys_gain, Some(18));
        assert_eq!(
            (base.auto_a_gain_max, base.auto_d_gain_max),
            (Some(95), Some(12))
        );

        // A maximum ISO set in Shutter is kept for Program too.
        let limit = ProSettings {
            max_iso: Some(MaxIso::Iso(6400)),
            ..Default::default()
        };
        let base = base_parameters(&shutter_params(), &limit).unwrap();
        assert_eq!(base.max_sys_gain, Some(2));
        assert_eq!(
            (base.auto_a_gain_max, base.auto_d_gain_max),
            (Some(58), Some(2))
        );

        // Leaving Shutter opens the caps again.
        for mode in [ExposureMode::Program, ExposureMode::Manual] {
            let leaving = ProSettings {
                mode: Some(mode),
                ..Default::default()
            };
            let base = base_parameters(&shutter_params(), &leaving).unwrap();
            assert_eq!(
                (base.auto_a_gain_max, base.auto_d_gain_max),
                (Some(255), Some(255))
            );
        }

        // The camera picks ISO in Shutter, and the shutter limit is Program's.
        let reject = |settings: ProSettings| {
            base_parameters(&shutter_params(), &settings)
                .unwrap_err()
                .status
        };
        assert_eq!(
            reject(ProSettings {
                iso: Some(800),
                ..Default::default()
            }),
            400
        );
        assert_eq!(
            reject(ProSettings {
                max_shutter: Some("1/250".into()),
                ..Default::default()
            }),
            400
        );
    }

    #[test]
    fn shutter_priority_lifts_and_returns_the_shutter_limit() {
        let to = |mode| ProSettings {
            mode: Some(mode),
            ..Default::default()
        };
        let capped = json!({ "auto_exposureEx": 0, "max_exposure": 1000 });
        assert_eq!(
            manual_unlatch(&capped, &to(ExposureMode::Shutter))
                .unwrap()
                .max_exposure,
            Some(BaseMaxExposureValue::_1)
        );

        let camera = Uuid::nil();
        let mut kept = IndexMap::new();
        let mut base = BaseParameterSetting::default();
        carry_shutter_limit(
            camera,
            &capped,
            &to(ExposureMode::Shutter),
            &mut base,
            &mut kept,
        );
        assert_eq!(kept.get(&camera), Some(&1000));
        let mut base = BaseParameterSetting::default();
        let shutter = shutter_params();
        carry_shutter_limit(
            camera,
            &shutter,
            &to(ExposureMode::Program),
            &mut base,
            &mut kept,
        );
        assert_eq!(base.max_exposure, Some(BaseMaxExposureValue::_1000));
        // With none kept, Program gets its default.
        let mut base = BaseParameterSetting::default();
        carry_shutter_limit(
            camera,
            &shutter,
            &to(ExposureMode::Program),
            &mut base,
            &mut kept,
        );
        assert_eq!(base.max_exposure, Some(PROGRAM_DEFAULT_MAX_EXPOSURE));
    }

    #[test]
    fn ramps_exposure_a_stop_at_a_time() {
        // ISO stops are a stop apart: one per step.
        let isos: Vec<(u32, f64)> = ISO_GAINS
            .iter()
            .map(|(iso, _, _)| (*iso, f64::from(*iso).log2()))
            .collect();
        let at = |iso: u32| isos.iter().position(|(stop, _)| *stop == iso).unwrap();
        assert_eq!(step_toward(&isos, 100f64.log2(), at(3200)), Some(200));
        assert_eq!(step_toward(&isos, 3200f64.log2(), at(100)), Some(1600));
        assert_eq!(step_toward(&isos, 800f64.log2(), at(800)), None);

        // Compensation is in half stops: two per step.
        let evs: Vec<(f32, f64)> = EXPOSURE_COMPENSATION
            .iter()
            .map(|(ev, _)| (*ev, f64::from(*ev)))
            .collect();
        assert_eq!(step_toward(&evs, -2.0, evs.len() - 1), Some(-1.0));

        // A value more than a stop away still moves, by one position.
        let limits = [(100, 100f64.log2()), (6400, 6400f64.log2())];
        assert_eq!(step_toward(&limits, 100f64.log2(), 1), Some(6400));

        let manual = state_of(&camera_params()); // 1/1000, ISO 400
        let target = ProSettings {
            shutter: Some("1/250".into()),
            iso: Some(100),
            ..Default::default()
        };
        // One value at a time: the shutter first, within a stop, then ISO.
        let step = next_step(&manual, &target).unwrap();
        assert_eq!(step.shutter.as_deref(), Some("1/600"));
        assert_eq!(step.iso, None);
        let mut walked = manual.clone();
        let mut steps = vec![];
        while let Some(step) = next_step(&walked, &target) {
            steps.push(step.clone());
            show_target(&mut walked, &step);
            assert!(steps.len() < 20, "never got there: {steps:?}");
        }
        assert_eq!(
            (walked.shutter.as_deref(), walked.iso),
            (Some("1/250"), Some(100))
        );
        let shutters: Vec<_> = steps
            .iter()
            .filter_map(|step| step.shutter.clone())
            .collect();
        assert_eq!(shutters, ["1/600", "1/300", "1/250"]);
        let isos: Vec<_> = steps.iter().filter_map(|step| step.iso).collect();
        assert_eq!(isos, [200, 100]);

        // Limits ramp too; full gain is one step past ISO 204800.
        let mut auto = state_of(&shutter_params()); // max ISO 51200
        let full = ProSettings {
            max_iso: Some(MaxIso::Full(FullGain::Full)),
            ..Default::default()
        };
        let mut limits = vec![];
        while let Some(step) = next_step(&auto, &full) {
            limits.push(step.max_iso.unwrap());
            show_target(&mut auto, &step);
        }
        assert_eq!(
            limits,
            [
                MaxIso::Iso(102400),
                MaxIso::Iso(204800),
                MaxIso::Full(FullGain::Full)
            ]
        );
    }

    #[test]
    fn ramps_exposure_but_jumps_modes_and_white_balance() {
        let change = ProSettings {
            shutter: Some("1/250".into()),
            exposure_compensation: Some(1.0),
            white_balance: Some(WhiteBalance::Auto { scene: None }),
            focus: Some(10.0),
            ..Default::default()
        };
        let (now, ramp) = split_ramp(&change);
        let ramp = ramp.unwrap();
        assert_eq!(
            (ramp.shutter.as_deref(), ramp.exposure_compensation),
            (Some("1/250"), Some(1.0))
        );
        assert_eq!((now.shutter, now.exposure_compensation), (None, None));
        assert!(now.white_balance.is_some() && now.focus.is_some());

        {
            // A mode change lands at the picture the camera has now, then ramps to what it asks.
            let program = json!({ "auto_exposureEx": 0, "max_sys_gain": 18, "max_exposure": 60 });
            let info =
                isp_info(&json!({ "isp_info": "ISP Info: ISO:39819 ExpTime:4100 HistError:0 " }));
            let to_manual = ProSettings {
                mode: Some(ExposureMode::Manual),
                shutter: Some("1/30".into()),
                iso: Some(800),
                white_balance: Some(WhiteBalance::Auto { scene: None }),
                ..Default::default()
            };
            let (now, ramp) = land(&program, Some(&info), &to_manual);
            assert_eq!(now.mode, Some(ExposureMode::Manual));
            // 4100 us is closest to 1/250 s; camera ISO 39819 is ISO 25600 on our scale.
            assert_eq!(
                (now.shutter.as_deref(), now.iso),
                (Some("1/250"), Some(25600))
            );
            assert!(now.white_balance.is_some());
            let ramp = ramp.unwrap();
            assert_eq!(
                (ramp.shutter.as_deref(), ramp.iso),
                (Some("1/30"), Some(800))
            );
            // Shutter priority lands at the shutter only; the camera picks ISO.
            let to_shutter = ProSettings {
                mode: Some(ExposureMode::Shutter),
                ..Default::default()
            };
            let (now, ramp) = land(&program, Some(&info), &to_shutter);
            assert_eq!(
                (now.shutter.as_deref(), now.iso, ramp),
                (Some("1/250"), None, None)
            );
            // Program lands as is: auto exposure takes over from the picture.
            let to_program = ProSettings {
                mode: Some(ExposureMode::Program),
                exposure_compensation: Some(1.0),
                ..Default::default()
            };
            let (now, ramp) = land(&camera_params(), None, &to_program);
            assert_eq!((now.shutter, now.iso), (None, None));
            assert_eq!(ramp.unwrap().exposure_compensation, Some(1.0));
        }
        let white_balance = ProSettings {
            white_balance: Some(WhiteBalance::Auto { scene: None }),
            ..Default::default()
        };
        assert_eq!(split_ramp(&white_balance).1, None);

        // A change during a ramp retargets it, keeping what it does not set.
        let iso = ProSettings {
            iso: Some(800),
            ..Default::default()
        };
        let target = retarget(Some(ramp), iso);
        assert_eq!(
            (
                target.shutter.as_deref(),
                target.iso,
                target.exposure_compensation
            ),
            (Some("1/250"), Some(800), Some(1.0))
        );
    }

    #[test]
    fn maps_max_shutter_to_the_auto_exposure_cap() {
        let program = json!({ "auto_exposureEx": 0, "max_exposure": 250 });
        assert_eq!(state_of(&program).max_shutter.as_deref(), Some("1/250"));
        assert_eq!(state_of(&camera_params()).max_shutter, None);

        let cap = |shutter: &str| ProSettings {
            max_shutter: Some(shutter.into()),
            ..Default::default()
        };
        let base = base_parameters(&program, &cap("1/1000")).unwrap();
        assert_eq!(base.max_exposure, Some(BaseMaxExposureValue::_1000));
        for time in BaseExposureTimeValue::ALL
            .iter()
            .filter(|t| calibrated_max_shutter(t))
        {
            let text = format!("1/{}", time.denominator());
            let base = base_parameters(&program, &cap(&text)).unwrap();
            let read = json!({ "max_exposure": base.max_exposure });
            assert_eq!(state_of(&read).max_shutter, Some(text));
        }
        // Uncalibrated caps (auto exposure ignored 1/8000) and Manual are refused.
        let reject = |params: &Value, shutter: &str| {
            base_parameters(params, &cap(shutter)).unwrap_err().status
        };
        assert_eq!(reject(&program, "1/8000"), 400);
        assert_eq!(reject(&program, "1/25"), 400);
        assert_eq!(reject(&camera_params(), "1/250"), 400);
    }

    #[test]
    fn entering_manual_first_lifts_the_shutter_cap() {
        let manual = ProSettings {
            mode: Some(ExposureMode::Manual),
            ..Default::default()
        };
        let capped = json!({ "auto_exposureEx": 0, "max_exposure": 1000 });
        assert_eq!(
            manual_unlatch(&capped, &manual).unwrap().max_exposure,
            Some(BaseMaxExposureValue::_1)
        );
        let open = json!({ "auto_exposureEx": 0, "max_exposure": 1 });
        assert_eq!(manual_unlatch(&open, &manual), None);
        let program = ProSettings {
            mode: Some(ExposureMode::Program),
            ..Default::default()
        };
        assert_eq!(manual_unlatch(&capped, &program), None);
        assert_eq!(manual_unlatch(&capped, &ProSettings::default()), None);
    }

    #[test]
    fn keeps_the_shutter_limit_across_manual() {
        let camera = Uuid::nil();
        let to = |mode| ProSettings {
            mode: Some(mode),
            ..Default::default()
        };
        let mut kept = IndexMap::new();
        let mut base = BaseParameterSetting::default();

        // Program with 1/250 to Manual keeps it, and nothing else is written for it.
        let program = json!({ "auto_exposureEx": 0, "max_exposure": 250 });
        carry_shutter_limit(
            camera,
            &program,
            &to(ExposureMode::Manual),
            &mut base,
            &mut kept,
        );
        assert_eq!(kept.get(&camera), Some(&250));
        assert_eq!(base.max_exposure, None);

        // A change that stays in Manual leaves it kept.
        let manual = json!({ "auto_exposureEx": 1, "max_exposure": 1 });
        carry_shutter_limit(
            camera,
            &manual,
            &to(ExposureMode::Manual),
            &mut base,
            &mut kept,
        );
        assert_eq!(kept.get(&camera), Some(&250));

        // Back to Program puts it back, once.
        carry_shutter_limit(
            camera,
            &manual,
            &to(ExposureMode::Program),
            &mut base,
            &mut kept,
        );
        assert_eq!(base.max_exposure, Some(BaseMaxExposureValue::_250));
        assert!(kept.is_empty());

        // A limit set with the switch back wins over the kept one.
        kept.insert(camera, 250);
        let mut base = BaseParameterSetting {
            max_exposure: Some(BaseMaxExposureValue::_1000),
            ..Default::default()
        };
        carry_shutter_limit(
            camera,
            &manual,
            &to(ExposureMode::Program),
            &mut base,
            &mut kept,
        );
        assert_eq!(base.max_exposure, Some(BaseMaxExposureValue::_1000));
        assert!(kept.is_empty());

        // Nothing kept (a manager restart): Program comes back with its default limit.
        let mut base = BaseParameterSetting::default();
        carry_shutter_limit(
            camera,
            &manual,
            &to(ExposureMode::Program),
            &mut base,
            &mut kept,
        );
        assert_eq!(base.max_exposure, Some(PROGRAM_DEFAULT_MAX_EXPOSURE));
    }

    #[test]
    fn maps_exposure_compensation_to_the_auto_exposure_target() {
        let program = json!({ "auto_exposureEx": 0, "blc_level": 50 });
        assert_eq!(state_of(&program).exposure_compensation, Some(0.0));
        assert_eq!(
            state_of(&json!({ "blc_level": 100 })).exposure_compensation,
            Some(1.0)
        );
        assert_eq!(state_of(&camera_params()).exposure_compensation, None);
        for (ev, level) in EXPOSURE_COMPENSATION {
            let settings = ProSettings {
                exposure_compensation: Some(ev),
                ..Default::default()
            };
            let base = base_parameters(&program, &settings).unwrap();
            assert_eq!(base.blc_level, Some(level));
            assert_eq!(
                state_of(&json!({ "blc_level": level })).exposure_compensation,
                Some(ev)
            );
        }
        let reject = |params: &Value, ev: f32| {
            let settings = ProSettings {
                exposure_compensation: Some(ev),
                ..Default::default()
            };
            base_parameters(params, &settings).unwrap_err().status
        };
        assert_eq!(reject(&program, 0.3), 400);
        assert_eq!(reject(&camera_params(), 0.5), 400);
    }

    #[test]
    fn reads_what_the_camera_is_using_in_program() {
        let info = isp_info(&json!({
            "isp_info": "ISP Info: ISO:57706 AGain:76789 DGain:1024 ISPDGain:7884 ExpTime:33273 ColorTemp:6172 Saturation:92 "
        }));
        assert_eq!(info.get("ExpTime"), Some(&33273));
        assert_eq!(info.get("HistError"), None);

        let mut program = state_of(&json!({ "auto_exposureEx": 0 }));
        apply_isp_info(&mut program, &info);
        assert_eq!(program.shutter.as_deref(), Some("1/30"));
        assert_eq!(program.color_temperature, Some(6172));
        // Between the camera's 39819 (ours 25600) and 85334 (ours 51200).
        assert!(
            (35000..37000).contains(&program.iso.unwrap()),
            "{:?}",
            program.iso
        );

        // In Manual the exposure time is auto exposure's last, not the shutter set.
        let mut manual = state_of(&camera_params());
        apply_isp_info(&mut manual, &info);
        assert_eq!(manual.shutter.as_deref(), Some("1/1000"));
        assert_eq!(manual.color_temperature, Some(6172));
    }

    #[test]
    fn converts_the_camera_iso_to_ours() {
        for (camera, ours) in CAMERA_ISO {
            assert_eq!(iso_of_camera(camera.into()), ours);
        }
        // Past the top, the last segment's slope carries on.
        assert!((380_000..440_000).contains(&iso_of_camera(375_000)));
        assert_eq!(shutter_of_micros(257), "1/3891");
        assert_eq!(shutter_of_micros(2_000_000), "2");
    }

    #[test]
    fn manual_white_balance_reports_the_scene_auto_returns_to() {
        let manual = state_of(&json!({ "auto_awb": 1, "awb_auto_mode": 1, "awb_red": 9 }));
        assert_eq!(
            manual.white_balance,
            WhiteBalance::Manual {
                red: Some(9),
                green: Some(0),
                blue: Some(0),
                scene: Some(WhiteBalanceScene::BlueWater),
            }
        );
    }

    #[test]
    fn back_to_auto_white_balance_keeps_the_scene() {
        let settings = ProSettings {
            white_balance: Some(WhiteBalance::Auto { scene: None }),
            ..Default::default()
        };
        let base = base_parameters(&camera_params(), &settings).unwrap();
        assert_eq!(base.auto_awb, Some(BaseAutoWhiteBalanceModeValue::Auto));
        // The camera's own scene stays: nothing is written for it.
        assert_eq!(base.awb_auto_mode, None);
    }

    #[test]
    fn rejects_what_the_mode_or_camera_does_not_take() {
        let reject = |settings: ProSettings| {
            base_parameters(&camera_params(), &settings)
                .unwrap_err()
                .status
        };
        let program = Some(ExposureMode::Program);
        let shutter = Some("1/1000".to_string());
        assert_eq!(
            reject(ProSettings {
                mode: program,
                shutter,
                ..Default::default()
            }),
            400
        );
        assert_eq!(
            reject(ProSettings {
                mode: program,
                iso: Some(400),
                ..Default::default()
            }),
            400
        );
        assert_eq!(
            reject(ProSettings {
                iso: Some(500),
                ..Default::default()
            }),
            400
        );
        assert_eq!(
            reject(ProSettings {
                shutter: Some("1/123".to_string()),
                ..Default::default()
            }),
            400
        );
        // camera_params() is in Manual, where ISO is set by hand.
        assert_eq!(
            reject(ProSettings {
                max_iso: Some(MaxIso::Iso(6400)),
                ..Default::default()
            }),
            400
        );
        assert_eq!(
            reject(ProSettings {
                mode: program,
                max_iso: Some(MaxIso::Iso(800)),
                ..Default::default()
            }),
            400
        );
    }

    #[test]
    fn converts_actuators_to_pro_units() {
        let config = |lens, reversed| ActuatorsConfig {
            parameters: Some(autopilot::api::ActuatorsParametersConfig {
                lens: Some(lens),
                tilt_channel: Some(autopilot::api::ServoChannel::SERVO3),
                tilt_channel_reversed: Some(reversed),
                tilt_mnt_pitch_min: Some(-70),
                tilt_mnt_pitch_max: Some(70),
                ..Default::default()
            }),
            ..Default::default()
        };
        let positions = ActuatorsState {
            focus: Some(40.0),
            zoom: Some(50.0),
            tilt: Some(25.0),
        };

        let motorized = Actuators::from_config(&config(LensType::Motorized, false));
        assert_eq!(
            motorized.read(positions),
            (Some(40.0), Some(1.5), Some(-35.0))
        );
        let settings = ProSettings {
            focus: Some(40.0),
            zoom: Some(1.5),
            tilt: Some(-35.0),
            ..Default::default()
        };
        assert_eq!(motorized.setpoints(&settings).unwrap(), positions);
        assert_eq!(
            motorized.tilt_range(),
            Some(Range {
                min: -70.0,
                max: 70.0
            })
        );

        // Reversed: 0 % of travel is the top of the pitch range.
        let reversed = Actuators::from_config(&config(LensType::Motorized, true));
        assert_eq!(reversed.read(positions).2, Some(35.0));
        let up = ProSettings {
            tilt: Some(35.0),
            ..Default::default()
        };
        assert_eq!(reversed.setpoints(&up).unwrap().tilt, Some(25.0));

        let fixed = Actuators::from_config(&config(LensType::FixedFocus, false));
        assert_eq!(fixed.read(positions), (None, None, Some(-35.0)));
        assert_eq!(fixed.focus_range(), None);
        let reject = |actuators: Actuators, settings: ProSettings| {
            actuators.setpoints(&settings).unwrap_err().status
        };
        assert_eq!(
            reject(
                fixed,
                ProSettings {
                    zoom: Some(1.5),
                    ..Default::default()
                }
            ),
            400
        );
        assert_eq!(
            reject(
                motorized,
                ProSettings {
                    zoom: Some(2.5),
                    ..Default::default()
                }
            ),
            400
        );
        assert_eq!(
            reject(
                motorized,
                ProSettings {
                    tilt: Some(80.0),
                    ..Default::default()
                }
            ),
            400
        );
    }

    #[tokio::test]
    async fn ws_routes_only_pro_paths() {
        let nil = Uuid::nil();
        assert!(ws_request("GET", "/camera/list", None).await.is_none());
        assert!(
            ws_request("DELETE", &format!("/pro/{nil}"), None)
                .await
                .is_none()
        );
        assert_eq!(ws_request("GET", "/pro/nope", None).await.unwrap().0, 400);
        let bogus = Some(json!({"bogus": 1}));
        let (status, _) = ws_request("PATCH", &format!("/pro/{nil}"), bogus)
            .await
            .unwrap();
        assert_eq!(status, 400);
    }
}
