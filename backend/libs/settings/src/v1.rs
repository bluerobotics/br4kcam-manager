use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{RawSettingsData, SettingsDataImpl, v0::SettingsDataV0};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsDataV1 {
    pub actuators: IndexMap<Uuid, CameraActuatorsSettings>,
    // todo: pub image: IndexMap<Uuid, CameraImageSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraActuatorsSettings {
    pub parameters: ActuatorsParameters,
    pub calibration: Calibration,
    pub state: ActuatorsState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Calibration {
    pub closest_points: FocusZoomPoints,
    pub furthest_points: FocusZoomPoints,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActuatorsParameters {
    // Lens. Settings saved before lens types existed have no `lens` and load as motorized.
    #[serde(default)]
    pub lens: LensType,

    // Camera parameters
    pub camera_id: CameraID,

    // Focus channel parameters
    pub focus_channel: ServoChannel,
    pub focus_channel_min: u16,
    pub focus_channel_trim: u16,
    pub focus_channel_max: u16,
    pub focus_margin_gain: f32,

    // Script channel parameters
    pub script_function: ScriptFunction,
    pub script_channel: ServoChannel,
    pub script_channel_min: u16,
    pub script_channel_trim: u16,
    pub script_channel_max: u16,
    pub enable_focus_and_zoom_correlation: bool,

    // Zoom channel parameters
    pub zoom_channel: ServoChannel,
    pub zoom_channel_min: u16,
    pub zoom_channel_trim: u16,
    pub zoom_channel_max: u16,

    // Tilt channel parameters
    pub tilt_channel: ServoChannel,
    pub tilt_channel_min: u16,
    pub tilt_channel_trim: u16,
    pub tilt_channel_max: u16,
    pub tilt_channel_reversed: bool,

    // Mount (MNTx) parameters
    pub tilt_mnt_type: MountType,
    pub tilt_mnt_pitch_min: i32,
    pub tilt_mnt_pitch_max: i32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LensType {
    #[default]
    Motorized,
    FixedFocus,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraID {
    #[default]
    CAM1 = 1,
    CAM2 = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MountType {
    Servo = 1,
    #[default]
    BrushlessPWM = 7,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TiltChannelFunction {
    #[default]
    /// Mount1Pitch
    MNT1 = 7,
    /// Mount2Pitch
    MNT2 = 13,
}

pub type FocusZoomPoints = Vec<FocusZoomPoint>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct FocusZoomPoint {
    pub focus: u32,
    pub zoom: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ActuatorsState {
    pub focus: Option<f32>,
    pub zoom: Option<f32>,
    pub tilt: Option<f32>,
}

impl From<SettingsDataV0> for SettingsDataV1 {
    fn from(_v1: SettingsDataV0) -> Self {
        SettingsDataV1 {
            actuators: IndexMap::default(),
        }
    }
}

impl SettingsDataImpl for SettingsDataV1 {
    fn get_actuators(&self) -> &IndexMap<Uuid, CameraActuatorsSettings> {
        &self.actuators
    }

    fn get_actuators_mut(&mut self) -> &mut IndexMap<Uuid, CameraActuatorsSettings> {
        &mut self.actuators
    }

    fn to_raw(&self) -> RawSettingsData {
        RawSettingsData::V1(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera saved before lens types existed.
    const SETTINGS_WITHOUT_LENS: &str = r#"{
        "version": "V1",
        "actuators": {
            "00000000-0000-0000-0000-000000000001": {
                "parameters": {
                    "camera_id": "CAM1",
                    "focus_channel": "SERVO10",
                    "focus_channel_min": 870,
                    "focus_channel_trim": 918,
                    "focus_channel_max": 2130,
                    "focus_margin_gain": 1.0,
                    "script_function": "SCRIPT1",
                    "script_channel": "SERVO12",
                    "script_channel_min": 870,
                    "script_channel_trim": 1500,
                    "script_channel_max": 2130,
                    "enable_focus_and_zoom_correlation": true,
                    "zoom_channel": "SERVO11",
                    "zoom_channel_min": 935,
                    "zoom_channel_trim": 935,
                    "zoom_channel_max": 1850,
                    "tilt_channel": "SERVO16",
                    "tilt_channel_min": 865,
                    "tilt_channel_trim": 1500,
                    "tilt_channel_max": 2250,
                    "tilt_channel_reversed": false,
                    "tilt_mnt_type": "BrushlessPWM",
                    "tilt_mnt_pitch_min": -70,
                    "tilt_mnt_pitch_max": 70
                },
                "calibration": { "closest_points": [], "furthest_points": [] },
                "state": { "focus": null, "zoom": null, "tilt": null }
            }
        }
    }"#;

    #[test]
    fn settings_without_lens_load_as_motorized_and_keep_a_new_lens() {
        let RawSettingsData::V1(mut settings) =
            serde_json::from_str::<RawSettingsData>(SETTINGS_WITHOUT_LENS).unwrap()
        else {
            panic!("expected V1 settings");
        };
        let parameters = &mut settings.actuators[0].parameters;
        assert_eq!(parameters.lens, LensType::Motorized);

        parameters.lens = LensType::FixedFocus;
        let saved = serde_json::to_string(&settings.to_raw()).unwrap();
        let RawSettingsData::V1(reloaded) =
            serde_json::from_str::<RawSettingsData>(&saved).unwrap()
        else {
            panic!("expected V1 settings");
        };
        assert_eq!(reloaded.actuators[0].parameters.lens, LensType::FixedFocus);
    }
}
