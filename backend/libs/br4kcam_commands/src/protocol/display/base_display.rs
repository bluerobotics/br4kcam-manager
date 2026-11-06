use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use tsync::tsync;

use super::*;

#[skip_serializing_none]
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct BaseParameterSetting {
    /// Tonal. Range: [0..=255]
    pub hue: Option<u8>,
    /// Brightness. Range: [0..=255]
    pub brightness: Option<u8>,
    /// Sharpness. Range: [0..=255]
    pub sharpness: Option<u8>,
    /// Contrast. Range: [0..=255]
    pub contrast: Option<u8>,
    /// Saturation. Range: [0..=255]
    pub saturation: Option<u8>,
    /// Gamma. Range: [0..=255]
    pub gamma: Option<u8>,
    /// Backlight Compensation. Range: [0..=255]
    pub blc_level: Option<u8>,
    /// Exposure Time.
    pub max_exposure: Option<BaseMaxExposureValue>,
    /// Restores all BaseParameterSetting parameters when `1`
    pub set_default: Option<u8>,
    /// Dehaze
    #[serde(rename = "antiFog")]
    #[serde(alias = "anti_fog")]
    pub anti_fog: Option<BaseAntiFogValue>,
    /// Frame Turbo.
    #[serde(rename = "frameTurbo_pro")]
    #[serde(alias = "frameTurbo")]
    #[serde(alias = "frame_turbo")]
    pub frame_turbo_pro: Option<BaseFrameTurboValue>,
    /// Scene Mode. Note: this is not working, use the `AdvanceParameterSetting::sene_mode` instead
    #[serde(rename = "sceneMode")]
    #[serde(alias = "scene_mode")]
    pub scene_mode: Option<BaseSceneModeValue>,
    /// Exposure Strategy Mode.
    #[serde(rename = "AE_strategy_mode")]
    #[serde(alias = "auto_exposure_strategy_mode")]
    pub auto_exposure_strategy_mode: Option<BaseAutoExposureStrategyModeValue>,
    /// Exposure Mode.
    #[serde(rename = "auto_exposureEx")]
    #[serde(alias = "auto_exposure_ex")]
    pub auto_exposure_ex: Option<BaseExposureModeValue>,
    /// Manual Exposure Time. This is the 'x' value in T = 1/x.
    pub exposure_time: Option<BaseExposureTimeValue>,
    /// White Balance Mode.
    pub auto_awb: Option<BaseAutoWhiteBalanceModeValue>,
    /// Manual White Balance Red. Range: [0..=255]
    pub awb_red: Option<u8>,
    /// Manual White Balance Green. Range: [0..=255]
    pub awb_green: Option<u8>,
    /// Manual White Balance Blue. Range: [0..=255]
    pub awb_blue: Option<u8>,
    /// White Balance Scene.
    pub awb_auto_mode: Option<BaseAutoWhiteBalanceSceneValue>,
    /// Manual White Balance Style Red. Range: [0..=255]
    pub awb_style_red: Option<u8>,
    /// Manual White Balance Style Green. Range: [0..=255]
    pub awb_style_green: Option<u8>,
    /// Manual White Balance Style Blue. Range: [0..=255]
    pub awb_style_blue: Option<u8>,
    /// Automatic Gain Mode.
    pub auto_gain_mode: Option<BaseAutoGainModeValue>,
    /// Automatic Maximum D Gain. Range: [0..=255]
    #[serde(rename = "auto_DGain_max")]
    #[serde(alias = "auto_d_gain_max")]
    pub auto_d_gain_max: Option<u8>,
    /// Automatic Maximum A Gain. Range: [0..=255]
    #[serde(rename = "auto_AGain_max")]
    #[serde(alias = "auto_a_gain_max")]
    pub auto_a_gain_max: Option<u8>,
    /// Max Sys Gain. Range: [0..=255]
    pub max_sys_gain: Option<u8>,
    /// Enable Manual A Gain.
    #[serde(rename = "manual_AGain_enable")]
    #[serde(alias = "manual_a_gain_enable")]
    pub manual_a_gain_enable: Option<BaseManualAGainEnableValue>,
    /// Manual A Gain. Range: [0..=255]
    #[serde(rename = "manual_AGain")]
    #[serde(alias = "manual_a_gain")]
    pub manual_a_gain: Option<u8>,
    /// Enable Manual D Gain.
    #[serde(rename = "manual_DGain_enable")]
    #[serde(alias = "manual_d_gain_enable")]
    pub manual_d_gain_enable: Option<BaseManualDGainEnableValue>,
    /// Manual D Gain. Range: [0..=255]
    #[serde(rename = "manual_DGain")]
    #[serde(alias = "manual_d_gain")]
    pub manual_d_gain: Option<u8>,
    /// Image Rotation
    pub rotate: Option<BaseRotateValue>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u32)]
pub enum BaseMaxExposureValue {
    _12 = 12,
    _25 = 25,
    #[default]
    _30 = 30,
    _50 = 50,
    _60 = 60,
    _100 = 100,
    _200 = 200,
    _400 = 400,
    _800 = 800,
    _1000 = 1000,
    _2000 = 2000,
    _4000 = 4000,
    _8000 = 8000,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseAntiFogValue {
    #[default]
    Close = 0,
    Open = 1,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseFrameTurboValue {
    #[default]
    Close = 0,
    HighFrameRates = 1,
    UltraHighFrameRates = 2,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseSceneModeValue {
    /// Camera firmware reports 0 when no capture scene is selected.
    #[default]
    Off = 0,
    FaceCapture = 1,
    LicensePlateCapture = 2,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseAutoExposureStrategyModeValue {
    #[default]
    HighLightPriority = 0,
    LowLightPriority = 1,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseExposureModeValue {
    #[default]
    Auto = 0,
    Manual = 1,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u32)]
pub enum BaseExposureTimeValue {
    _12 = 12,
    _25 = 25,
    _30 = 30,
    _50 = 50,
    #[default]
    _60 = 60,
    _100 = 100,
    _200 = 200,
    _400 = 400,
    _800 = 800,
    _1000 = 1000,
    _2000 = 2000,
    _4000 = 4000,
    _8000 = 8000,
    _10000 = 10000,
    /// 1/100000 s. The camera takes it as 34464: 100000 truncated to 16 bits.
    _100000 = 34464,
}

impl BaseExposureTimeValue {
    /// Every exposure the camera takes, longest first.
    pub const ALL: [Self; 15] = [
        Self::_12,
        Self::_25,
        Self::_30,
        Self::_50,
        Self::_60,
        Self::_100,
        Self::_200,
        Self::_400,
        Self::_800,
        Self::_1000,
        Self::_2000,
        Self::_4000,
        Self::_8000,
        Self::_10000,
        Self::_100000,
    ];

    /// The `x` of the 1/x s exposure, which is not always the number sent to the camera.
    pub fn denominator(&self) -> u32 {
        match self {
            Self::_100000 => 100_000,
            other => other.clone() as u32,
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseAutoWhiteBalanceModeValue {
    #[default]
    Auto = 0,
    Manual = 1,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseAutoWhiteBalanceSceneValue {
    #[default]
    Scene1 = 0,
    Scene2 = 1,
    // Scene3 = 2,
    // Scene4 = 3,
    // Scene5 = 4,
    // Scene6 = 5,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseAutoGainModeValue {
    #[default]
    Auto = 0,
    Manual = 1,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseManualAGainEnableValue {
    #[default]
    Close = 0,
    Open = 1,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseManualDGainEnableValue {
    #[default]
    Close = 0,
    Open = 1,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize_repr, Deserialize_repr, TS)]
#[tsync]
#[repr(u8)]
pub enum BaseRotateValue {
    #[default]
    _0 = 0,
    _90 = 1,
    _180 = 2,
    _270 = 3,
}

#[cfg(test)]
mod tests {

    use serde_json::json;

    use utils::deserialize;

    use super::*;

    #[test]
    fn exposure_times_are_all_listed_with_their_real_duration() {
        let wire: Vec<BaseExposureTimeValue> = (0..=u32::from(u16::MAX))
            .filter_map(|value| deserialize(&value.to_string()).ok())
            .collect();
        assert_eq!(wire, BaseExposureTimeValue::ALL.to_vec());
        assert_eq!(BaseExposureTimeValue::_100000.denominator(), 100_000);
        assert_eq!(BaseExposureTimeValue::_1000.denominator(), 1000);
    }

    #[test]
    fn setting_some_parameters() {
        let json = json!({
            "hue": 128,
            "brightness": 128,
            "sharpness": 128,
            "contrast": 128,
            "saturation": 128,
            "gamma": 137,
        })
        .to_string();

        let params = deserialize::<BaseParameterSetting>(&json).expect("Failed deserializing");

        let expected_params = BaseParameterSetting {
            hue: Some(128),
            brightness: Some(128),
            sharpness: Some(128),
            contrast: Some(128),
            saturation: Some(128),
            gamma: Some(137),
            ..Default::default()
        };

        assert_eq!(expected_params, params);
    }
}
