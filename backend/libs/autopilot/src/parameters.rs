use anyhow::{Result, anyhow};
use mavlink::ardupilotmega::{MavParamType, PARAM_VALUE_DATA};
use num_enum::TryFromPrimitive;
use serde::{Deserialize, Serialize};

use crate::{
    api::{self, ActuatorsParametersConfig},
    mavlink::parameters::ParamEncodingType,
};

/// Tolerance for [`ParamType::REAL32`] / [`ParamType::REAL64`] actuator values only.
/// Integer parameters are compared exactly after normalising to a common numeric
/// representation. Lua param-table values such as `BR4KCAM1_ENABLE` arrive as `REAL32`
/// while the expectation holds `UINT8`, so the comparison must coerce by value, not
/// by encoded MAVLink wire form.
pub(crate) const PARAM_DRIFT_TOLERANCE: f32 = 0.5;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub value: ParamType,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
pub enum ParamType {
    UINT8(u8),
    INT8(i8),
    UINT16(u16),
    INT16(i16),
    UINT32(u32),
    INT32(i32),
    UINT64(u64),
    INT64(i64),
    REAL32(f32),
    REAL64(f64),
}

impl ParamType {
    pub fn encode(&self, encoding: ParamEncodingType) -> Result<f32> {
        use ParamEncodingType::*;

        let value = match (self, encoding) {
            // C_CAST
            (ParamType::UINT8(v), CCast) => *v as f32,
            (ParamType::INT8(v), CCast) => *v as f32,
            (ParamType::UINT16(v), CCast) => *v as f32,
            (ParamType::INT16(v), CCast) => *v as f32,
            (ParamType::UINT32(v), CCast) => *v as f32,
            (ParamType::INT32(v), CCast) => *v as f32,
            (ParamType::REAL32(v), CCast) => *v,
            (ParamType::REAL64(v), CCast) => *v as f32,
            (ParamType::UINT64(_) | ParamType::INT64(_), CCast) => {
                panic!("Use PARAM_EXT_* for 64-bit values")
            }

            // ByteWise
            (ParamType::UINT8(v), ByteWise) => f32::from_bits(*v as u32),
            (ParamType::INT8(v), ByteWise) => f32::from_bits(*v as u8 as u32),
            (ParamType::UINT16(v), ByteWise) => f32::from_bits(*v as u32),
            (ParamType::INT16(v), ByteWise) => f32::from_bits(*v as u16 as u32),
            (ParamType::UINT32(v), ByteWise) => f32::from_bits(*v),
            (ParamType::INT32(v), ByteWise) => f32::from_bits(*v as u32),
            (ParamType::REAL32(v), ByteWise) => *v,
            (ParamType::UINT64(_) | ParamType::INT64(_) | ParamType::REAL64(_), ByteWise) => {
                panic!("Use PARAM_EXT_* for 64-bit values")
            }

            // Unsupported
            (_, Unsupported) => return Err(anyhow!("Unsupported encoding")),
        };

        Ok(value)
    }

    fn decode(data: &PARAM_VALUE_DATA, encoding: ParamEncodingType) -> Result<Self> {
        use MavParamType::*;
        use ParamEncodingType::*;

        let param = match (data.param_type, encoding) {
            // C_CAST
            (MAV_PARAM_TYPE_UINT8, CCast) => ParamType::UINT8(data.param_value as u8),
            (MAV_PARAM_TYPE_INT8, CCast) => ParamType::INT8(data.param_value as i8),
            (MAV_PARAM_TYPE_UINT16, CCast) => ParamType::UINT16(data.param_value as u16),
            (MAV_PARAM_TYPE_INT16, CCast) => ParamType::INT16(data.param_value as i16),
            (MAV_PARAM_TYPE_UINT32, CCast) => ParamType::UINT32(data.param_value as u32),
            (MAV_PARAM_TYPE_INT32, CCast) => ParamType::INT32(data.param_value as i32),
            (MAV_PARAM_TYPE_REAL32, CCast) => ParamType::REAL32(data.param_value),
            (MAV_PARAM_TYPE_REAL64, CCast) => ParamType::REAL64(data.param_value as f64),
            (MAV_PARAM_TYPE_UINT64 | MAV_PARAM_TYPE_INT64, CCast) => {
                panic!("Use PARAM_EXT_* for 64-bit values")
            }

            // ByteWise
            (MAV_PARAM_TYPE_UINT8, ByteWise) => ParamType::UINT8(data.param_value.to_bits() as u8),
            (MAV_PARAM_TYPE_INT8, ByteWise) => ParamType::INT8(data.param_value.to_bits() as i8),
            (MAV_PARAM_TYPE_UINT16, ByteWise) => {
                ParamType::UINT16(data.param_value.to_bits() as u16)
            }
            (MAV_PARAM_TYPE_INT16, ByteWise) => ParamType::INT16(data.param_value.to_bits() as i16),
            (MAV_PARAM_TYPE_UINT32, ByteWise) => ParamType::UINT32(data.param_value.to_bits()),
            (MAV_PARAM_TYPE_INT32, ByteWise) => ParamType::INT32(data.param_value.to_bits() as i32),
            (MAV_PARAM_TYPE_REAL32, ByteWise) => {
                ParamType::REAL32(f32::from_bits(data.param_value.to_bits()))
            }

            // 64-bit and REAL64 require the *extended* protocol
            (MAV_PARAM_TYPE_UINT64 | MAV_PARAM_TYPE_INT64 | MAV_PARAM_TYPE_REAL64, ByteWise) => {
                panic!("Use PARAM_EXT_* for 64-bit values")
            }

            (_, Unsupported) => return Err(anyhow!("Unsupported encoding")),
        };

        Ok(param)
    }

    pub fn set_value(&mut self, new_value: ParamType, encoding: ParamEncodingType) -> Result<()> {
        let encoded_value = new_value.encode(encoding)?;

        use ParamEncodingType::*;
        use ParamType::*;

        match (self, encoding) {
            // C_CAST
            (UINT8(value), CCast) => *value = encoded_value as u8,
            (INT8(value), CCast) => *value = encoded_value as i8,
            (UINT16(value), CCast) => *value = encoded_value as u16,
            (INT16(value), CCast) => *value = encoded_value as i16,
            (UINT32(value), CCast) => *value = encoded_value as u32,
            (INT32(value), CCast) => *value = encoded_value as i32,
            (UINT64(value), CCast) => *value = encoded_value as u64,
            (INT64(value), CCast) => *value = encoded_value as i64,
            (REAL32(value), CCast) => *value = encoded_value,
            (REAL64(value), CCast) => *value = encoded_value as f64,

            // ByteWise
            (UINT8(value), ByteWise) => *value = encoded_value.to_bits() as u8,
            (INT8(value), ByteWise) => *value = encoded_value.to_bits() as i8,
            (UINT16(value), ByteWise) => *value = encoded_value.to_bits() as u16,
            (INT16(value), ByteWise) => *value = encoded_value.to_bits() as i16,
            (UINT32(value), ByteWise) => *value = encoded_value.to_bits(),
            (INT32(value), ByteWise) => *value = encoded_value.to_bits() as i32,
            (UINT64(value), ByteWise) => *value = encoded_value.to_bits() as u64,
            (INT64(value), ByteWise) => *value = encoded_value.to_bits() as i64,
            (REAL32(value), ByteWise) => *value = encoded_value,
            (REAL64(value), ByteWise) => *value = encoded_value.to_bits() as f64,

            //
            (_, Unsupported) => return Err(anyhow!("Unsupported encoding")),
        }

        Ok(())
    }
}

impl Parameter {
    pub fn try_new(data: &PARAM_VALUE_DATA, encoding: ParamEncodingType) -> Result<Self> {
        Ok(Self {
            name: data.param_id.to_str()?.to_string(),
            value: ParamType::decode(data, encoding)?,
        })
    }

    pub fn param_value(&self, encoding: ParamEncodingType) -> Result<f32> {
        self.value.encode(encoding)
    }

    pub fn param_type(&self) -> MavParamType {
        match &self.value {
            ParamType::UINT8(_) => MavParamType::MAV_PARAM_TYPE_UINT8,
            ParamType::INT8(_) => MavParamType::MAV_PARAM_TYPE_INT8,
            ParamType::UINT16(_) => MavParamType::MAV_PARAM_TYPE_UINT16,
            ParamType::INT16(_) => MavParamType::MAV_PARAM_TYPE_INT16,
            ParamType::UINT32(_) => MavParamType::MAV_PARAM_TYPE_UINT32,
            ParamType::INT32(_) => MavParamType::MAV_PARAM_TYPE_INT32,
            ParamType::UINT64(_) => MavParamType::MAV_PARAM_TYPE_UINT64,
            ParamType::INT64(_) => MavParamType::MAV_PARAM_TYPE_INT64,
            ParamType::REAL32(_) => MavParamType::MAV_PARAM_TYPE_REAL32,
            ParamType::REAL64(_) => MavParamType::MAV_PARAM_TYPE_REAL64,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ActuatorsParameters {
    // Camera parameters
    pub camera_id: api::CameraID,

    // Focus channel parameters
    pub focus_channel: api::ServoChannel,
    pub focus_channel_min: u16,
    pub focus_channel_trim: u16,
    pub focus_channel_max: u16,
    pub focus_margin_gain: f32,

    // Script channel parameters
    pub script_function: api::ScriptFunction,
    pub script_channel: api::ServoChannel,
    pub script_channel_min: u16,
    pub script_channel_trim: u16,
    pub script_channel_max: u16,
    pub enable_focus_and_zoom_correlation: bool,

    // Zoom channel parameters
    pub zoom_channel: api::ServoChannel,
    pub zoom_channel_min: u16,
    pub zoom_channel_trim: u16,
    pub zoom_channel_max: u16,

    // Tilt channel parameters
    pub tilt_channel: api::ServoChannel,
    pub tilt_channel_min: u16,
    pub tilt_channel_trim: u16,
    pub tilt_channel_max: u16,
    pub tilt_channel_reversed: bool,

    // Mount (MNTx) parameters
    pub tilt_mnt_type: api::MountType,
    pub tilt_mnt_pitch_min: i32,
    pub tilt_mnt_pitch_max: i32,
}

impl Default for ActuatorsParameters {
    fn default() -> Self {
        Self {
            // Camera parameters
            camera_id: api::CameraID::CAM1,

            // Focus - used as Focus output, controlled by the Lua script
            focus_channel: api::ServoChannel::SERVO10,
            focus_channel_min: 870,
            focus_channel_trim: 918,
            focus_channel_max: 2130,
            focus_margin_gain: 1.0,

            // Script channel - used as the Focus input coming from the GCS, controlled by the user
            script_channel: api::ServoChannel::SERVO12,
            script_channel_min: 870,
            script_channel_trim: 1500,
            script_channel_max: 2130,
            script_function: api::ScriptFunction::SCRIPT1,
            enable_focus_and_zoom_correlation: true,

            // Zoom - used as Zoom output, controlled by the ZoomFocus.lua script
            zoom_channel: api::ServoChannel::SERVO11,
            zoom_channel_min: 935,
            zoom_channel_trim: 935,
            zoom_channel_max: 1850,

            // Tilt - controlled by the user
            tilt_channel: api::ServoChannel::SERVO16,
            tilt_channel_min: 865,
            tilt_channel_trim: 1500,
            tilt_channel_max: 2250,
            tilt_channel_reversed: false,

            // Mount
            tilt_mnt_type: api::MountType::Servo,
            tilt_mnt_pitch_min: -70,
            tilt_mnt_pitch_max: 70,
        }
    }
}

impl From<ActuatorsParametersConfig> for ActuatorsParameters {
    fn from(value: ActuatorsParametersConfig) -> Self {
        let default = Self::default();
        Self {
            camera_id: value.camera_id.unwrap_or(default.camera_id),
            focus_channel: value.focus_channel.unwrap_or(default.focus_channel),
            focus_channel_min: value.focus_channel_min.unwrap_or(default.focus_channel_min),
            focus_channel_trim: value
                .focus_channel_trim
                .unwrap_or(default.focus_channel_trim),
            focus_channel_max: value.focus_channel_max.unwrap_or(default.focus_channel_max),
            focus_margin_gain: value.focus_margin_gain.unwrap_or(default.focus_margin_gain),
            script_function: value.script_function.unwrap_or(default.script_function),
            script_channel: value.script_channel.unwrap_or(default.script_channel),
            script_channel_min: value
                .script_channel_min
                .unwrap_or(default.script_channel_min),
            script_channel_trim: value
                .script_channel_trim
                .unwrap_or(default.script_channel_trim),
            script_channel_max: value
                .script_channel_max
                .unwrap_or(default.script_channel_max),

            enable_focus_and_zoom_correlation: value
                .enable_focus_and_zoom_correlation
                .unwrap_or(default.enable_focus_and_zoom_correlation),
            zoom_channel: value.zoom_channel.unwrap_or(default.zoom_channel),
            zoom_channel_min: value.zoom_channel_min.unwrap_or(default.zoom_channel_min),
            zoom_channel_trim: value.zoom_channel_trim.unwrap_or(default.zoom_channel_trim),
            zoom_channel_max: value.zoom_channel_max.unwrap_or(default.zoom_channel_max),
            tilt_channel: value.tilt_channel.unwrap_or(default.tilt_channel),
            tilt_channel_min: value.tilt_channel_min.unwrap_or(default.tilt_channel_min),
            tilt_channel_trim: value.tilt_channel_trim.unwrap_or(default.tilt_channel_trim),
            tilt_channel_max: value.tilt_channel_max.unwrap_or(default.tilt_channel_max),
            tilt_channel_reversed: value
                .tilt_channel_reversed
                .unwrap_or(default.tilt_channel_reversed),
            tilt_mnt_type: value.tilt_mnt_type.unwrap_or(default.tilt_mnt_type),
            tilt_mnt_pitch_min: value
                .tilt_mnt_pitch_min
                .unwrap_or(default.tilt_mnt_pitch_min),
            tilt_mnt_pitch_max: value
                .tilt_mnt_pitch_max
                .unwrap_or(default.tilt_mnt_pitch_max),
        }
    }
}

#[repr(i16)]
#[derive(Debug, Clone, Copy, PartialEq, TryFromPrimitive)]
pub enum ChannelFunction {
    Disabled = 0,
    Mount1Pitch = 7,
    Mount2Pitch = 13,
    CameraFocus = 92,
    CameraZoom = 180,
    Script1 = 94,
    Script2 = 95,
    Script3 = 96,
    Script4 = 97,
    Script5 = 98,
    Script6 = 99,
    Script7 = 100,
    Script8 = 101,
    Script9 = 102,
    Script10 = 103,
    Script11 = 104,
    Script12 = 105,
    Script13 = 106,
    Script14 = 107,
    Script15 = 108,
    Script16 = 109,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CameraType {
    None = 0,
    Servo = 1,
}

pub const CLOSEST_POINTS: &[api::FocusZoomPoint] = &[
    api::FocusZoomPoint {
        zoom: 870,
        focus: 949,
    },
    api::FocusZoomPoint {
        zoom: 986,
        focus: 975,
    },
    api::FocusZoomPoint {
        zoom: 1191,
        focus: 1102,
    },
    api::FocusZoomPoint {
        zoom: 1413,
        focus: 1277,
    },
    api::FocusZoomPoint {
        zoom: 1617,
        focus: 1518,
    },
    api::FocusZoomPoint {
        zoom: 1771,
        focus: 1836,
    },
    api::FocusZoomPoint {
        zoom: 1831,
        focus: 2057,
    },
    api::FocusZoomPoint {
        zoom: 1951,
        focus: 2556,
    },
];

pub const FURTHEST_POINTS: &[api::FocusZoomPoint] = &[
    api::FocusZoomPoint {
        zoom: 870,
        focus: 834,
    },
    api::FocusZoomPoint {
        zoom: 986,
        focus: 905,
    },
    api::FocusZoomPoint {
        zoom: 1191,
        focus: 1011,
    },
    api::FocusZoomPoint {
        zoom: 1413,
        focus: 1168,
    },
    api::FocusZoomPoint {
        zoom: 1617,
        focus: 1359,
    },
    api::FocusZoomPoint {
        zoom: 1771,
        focus: 1571,
    },
    api::FocusZoomPoint {
        zoom: 1831,
        focus: 1699,
    },
    api::FocusZoomPoint {
        zoom: 1951,
        focus: 2159,
    },
];

pub(crate) fn param_display_value(value: &ParamType) -> Option<f32> {
    param_numeric_value(value).map(|value| value as f32)
}

fn param_numeric_value(value: &ParamType) -> Option<f64> {
    match value {
        ParamType::UINT8(v) => Some(f64::from(*v)),
        ParamType::INT8(v) => Some(f64::from(*v)),
        ParamType::UINT16(v) => Some(f64::from(*v)),
        ParamType::INT16(v) => Some(f64::from(*v)),
        ParamType::UINT32(v) => Some(f64::from(*v)),
        ParamType::INT32(v) => Some(f64::from(*v)),
        ParamType::UINT64(v) => Some(*v as f64),
        ParamType::INT64(v) => Some(*v as f64),
        ParamType::REAL32(v) => Some(f64::from(*v)),
        ParamType::REAL64(v) => Some(*v),
    }
}

fn param_is_integer_valued(value: &ParamType) -> bool {
    !matches!(value, ParamType::REAL32(_) | ParamType::REAL64(_))
}

pub(crate) fn param_values_match(expected: &ParamType, actual: &ParamType, tolerance: f32) -> bool {
    let (Some(expected_n), Some(actual_n)) =
        (param_numeric_value(expected), param_numeric_value(actual))
    else {
        return expected == actual;
    };

    if param_is_integer_valued(expected) || param_is_integer_valued(actual) {
        return expected_n == actual_n;
    }

    (expected_n - actual_n).abs() <= f64::from(tolerance)
}

#[cfg(test)]
mod drift_tests {
    use super::*;

    #[test]
    fn param_drift_tolerance_flags_real_mismatch_not_type_mismatch() {
        let expected = ParamType::INT16(ChannelFunction::CameraFocus as i16);

        assert!(param_values_match(
            &expected,
            &ParamType::INT16(ChannelFunction::CameraFocus as i16),
            PARAM_DRIFT_TOLERANCE,
        ));
        assert!(param_values_match(
            &expected,
            &ParamType::REAL32(ChannelFunction::CameraFocus as i16 as f32),
            PARAM_DRIFT_TOLERANCE,
        ));
        assert!(param_values_match(
            &ParamType::UINT8(1),
            &ParamType::REAL32(1.0),
            PARAM_DRIFT_TOLERANCE,
        ));

        assert!(!param_values_match(
            &expected,
            &ParamType::INT16(ChannelFunction::CameraZoom as i16),
            PARAM_DRIFT_TOLERANCE,
        ));
        assert!(!param_values_match(
            &ParamType::UINT16(1500),
            &ParamType::UINT16(1501),
            PARAM_DRIFT_TOLERANCE,
        ));

        // Floats compare within the tolerance band, not bytewise.
        assert!(param_values_match(
            &ParamType::REAL32(1.5),
            &ParamType::REAL32(1.2),
            PARAM_DRIFT_TOLERANCE,
        ));
        assert!(!param_values_match(
            &ParamType::REAL32(1.5),
            &ParamType::REAL32(2.1),
            PARAM_DRIFT_TOLERANCE,
        ));
    }

    #[test]
    fn param_drift_compares_bytewise_integers_exactly() {
        assert!(param_values_match(
            &ParamType::UINT16(1500),
            &ParamType::UINT16(1500),
            PARAM_DRIFT_TOLERANCE,
        ));
        assert!(!param_values_match(
            &ParamType::UINT16(1500),
            &ParamType::UINT16(1501),
            PARAM_DRIFT_TOLERANCE,
        ));
    }
}
