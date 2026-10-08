use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{CameraActuators, parameters::ActuatorsParameters};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActuatorsControl {
    #[ts(as = "String")]
    pub camera_uuid: Uuid,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "action", content = "json")]
pub enum Action {
    #[serde(rename = "exportLuaScript")]
    ExportLuaScript,
    #[serde(rename = "getActuatorsState")]
    GetActuatorsState,
    #[serde(rename = "setActuatorsState")]
    SetActuatorsState(ActuatorsState),
    #[serde(rename = "getActuatorsDefaultConfig")]
    GetActuatorsDefaultConfig,
    #[serde(rename = "getActuatorsConfig")]
    GetActuatorsConfig,
    #[serde(rename = "setActuatorsConfig")]
    SetActuatorsConfig(ActuatorsConfig),
    #[serde(rename = "resetActuatorsConfig")]
    ResetActuatorsConfig,
    #[serde(rename = "forceResetActuatorsConfig")]
    ForceResetActuatorsConfig,
    #[serde(rename = "forgetActuatorsConfig")]
    ForgetActuatorsConfig,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone, Copy, TS)]
pub struct ActuatorsState {
    pub focus: Option<f32>,
    pub zoom: Option<f32>,
    pub tilt: Option<f32>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone, TS)]
pub struct ActuatorsConfig {
    pub parameters: Option<ActuatorsParametersConfig>,
    pub closest_points: Option<FocusZoomPoints>,
    pub furthest_points: Option<FocusZoomPoints>,
}

impl From<&CameraActuators> for ActuatorsConfig {
    fn from(value: &CameraActuators) -> Self {
        Self {
            parameters: Some((&value.parameters).into()),
            closest_points: Some(value.closest_points.clone()),
            furthest_points: Some(value.furthest_points.clone()),
        }
    }
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone, TS)]
pub struct ActuatorsParametersConfig {
    // Camera parameters
    pub camera_id: Option<CameraID>,

    // Focus channel parameters
    pub focus_channel: Option<ServoChannel>,
    pub focus_channel_min: Option<u16>,
    pub focus_channel_trim: Option<u16>,
    pub focus_channel_max: Option<u16>,
    pub focus_margin_gain: Option<f32>,

    // Script channel parameters
    pub script_function: Option<ScriptFunction>,
    pub script_channel: Option<ServoChannel>,
    pub script_channel_min: Option<u16>,
    pub script_channel_trim: Option<u16>,
    pub script_channel_max: Option<u16>,
    pub enable_focus_and_zoom_correlation: Option<bool>,

    // Zoom channel parameters
    pub zoom_channel: Option<ServoChannel>,
    pub zoom_channel_min: Option<u16>,
    pub zoom_channel_trim: Option<u16>,
    pub zoom_channel_max: Option<u16>,

    // Tilt channel parameters
    pub tilt_channel: Option<ServoChannel>,
    pub tilt_channel_min: Option<u16>,
    pub tilt_channel_trim: Option<u16>,
    pub tilt_channel_max: Option<u16>,
    pub tilt_channel_reversed: Option<bool>,

    // Mount (MNTx) parameters
    pub tilt_mnt_type: Option<MountType>,
    pub tilt_mnt_pitch_min: Option<i32>,
    pub tilt_mnt_pitch_max: Option<i32>,
}

impl From<&ActuatorsParameters> for ActuatorsParametersConfig {
    fn from(value: &ActuatorsParameters) -> Self {
        Self {
            camera_id: Some(value.camera_id),
            focus_channel: Some(value.focus_channel),
            focus_channel_min: Some(value.focus_channel_min),
            focus_channel_trim: Some(value.focus_channel_trim),
            focus_channel_max: Some(value.focus_channel_max),
            focus_margin_gain: Some(value.focus_margin_gain),
            script_function: Some(value.script_function),
            script_channel: Some(value.script_channel),
            script_channel_min: Some(value.script_channel_min),
            script_channel_trim: Some(value.script_channel_trim),
            script_channel_max: Some(value.script_channel_max),
            enable_focus_and_zoom_correlation: Some(value.enable_focus_and_zoom_correlation),
            zoom_channel: Some(value.zoom_channel),
            zoom_channel_min: Some(value.zoom_channel_min),
            zoom_channel_trim: Some(value.zoom_channel_trim),
            zoom_channel_max: Some(value.zoom_channel_max),
            tilt_channel: Some(value.tilt_channel),
            tilt_channel_min: Some(value.tilt_channel_min),
            tilt_channel_trim: Some(value.tilt_channel_trim),
            tilt_channel_max: Some(value.tilt_channel_max),
            tilt_channel_reversed: Some(value.tilt_channel_reversed),
            tilt_mnt_type: Some(value.tilt_mnt_type),
            tilt_mnt_pitch_min: Some(value.tilt_mnt_pitch_min),
            tilt_mnt_pitch_max: Some(value.tilt_mnt_pitch_max),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum CameraID {
    #[default]
    CAM1 = 1,
    CAM2 = 2,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[repr(u8)]
pub enum TiltChannelFunction {
    #[default]
    /// Mount1Pitch
    MNT1 = 7,
    /// Mount2Pitch
    MNT2 = 13,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[repr(u8)]
pub enum MountType {
    #[default]
    Servo = 1,
    BrushlessPWM = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[repr(u8)]
pub enum ServoChannel {
    SERVO1 = 1,
    SERVO2 = 2,
    SERVO3 = 3,
    SERVO4 = 4,
    SERVO5 = 5,
    SERVO6 = 6,
    SERVO7 = 7,
    SERVO8 = 8,
    SERVO9 = 9,
    SERVO10 = 10,
    SERVO11 = 11,
    SERVO12 = 12,
    SERVO13 = 13,
    SERVO14 = 14,
    SERVO15 = 15,
    SERVO16 = 16,
    SERVO17 = 17,
    SERVO18 = 18,
    SERVO19 = 19,
    SERVO20 = 20,
    SERVO21 = 21,
    SERVO22 = 22,
    SERVO23 = 23,
    SERVO24 = 24,
    SERVO25 = 25,
    SERVO26 = 26,
    SERVO27 = 27,
    SERVO28 = 28,
    SERVO29 = 29,
    SERVO30 = 30,
    SERVO31 = 31,
    SERVO32 = 32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[repr(u8)]
pub enum ScriptFunction {
    #[default]
    SCRIPT1 = 94,
    SCRIPT2 = 95,
    SCRIPT3 = 96,
    SCRIPT4 = 97,
    SCRIPT5 = 98,
    SCRIPT6 = 99,
    SCRIPT7 = 100,
    SCRIPT8 = 101,
    SCRIPT9 = 102,
    SCRIPT10 = 103,
    SCRIPT11 = 104,
    SCRIPT12 = 105,
    SCRIPT13 = 106,
    SCRIPT14 = 107,
    SCRIPT15 = 108,
    SCRIPT16 = 109,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, PartialOrd, TS)]
pub struct FocusZoomPoints(pub Vec<FocusZoomPoint>);
impl FocusZoomPoints {
    pub fn to_lua(&self) -> String {
        let entries: Vec<String> = self
            .0
            .iter()
            .map(|point| format!("    {{zoom = {}, focus = {}}}", point.zoom, point.focus))
            .collect();

        format!("{{\n{}\n}}", entries.join(",\n"))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, TS)]
pub struct FocusZoomPoint {
    pub zoom: u32,
    pub focus: u32,
}
