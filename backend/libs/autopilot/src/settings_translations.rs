use crate::{CameraActuators, api, parameters};

impl From<&settings::CameraActuatorsSettings> for CameraActuators {
    fn from(value: &settings::CameraActuatorsSettings) -> Self {
        Self {
            parameters: (&value.parameters).into(),
            closest_points: (&value.calibration.closest_points).into(),
            furthest_points: (&value.calibration.furthest_points).into(),
            state: (&value.state).into(),
        }
    }
}
impl From<&CameraActuators> for settings::CameraActuatorsSettings {
    fn from(value: &CameraActuators) -> Self {
        settings::CameraActuatorsSettings {
            parameters: (&value.parameters).into(),
            calibration: settings::Calibration {
                closest_points: (&value.closest_points).into(),
                furthest_points: (&value.furthest_points).into(),
            },
            state: (&value.state).into(),
        }
    }
}

impl From<&settings::ActuatorsParameters> for parameters::ActuatorsParameters {
    fn from(value: &settings::ActuatorsParameters) -> Self {
        Self {
            lens: (&value.lens).into(),
            camera_id: (&value.camera_id).into(),
            focus_channel: (&value.focus_channel).into(),
            focus_channel_min: value.focus_channel_min,
            focus_channel_trim: value.focus_channel_trim,
            focus_channel_max: value.focus_channel_max,
            focus_margin_gain: value.focus_margin_gain,
            script_function: (&value.script_function).into(),
            script_channel: (&value.script_channel).into(),
            script_channel_min: value.script_channel_min,
            script_channel_trim: value.script_channel_trim,
            script_channel_max: value.script_channel_max,
            enable_focus_and_zoom_correlation: value.enable_focus_and_zoom_correlation,
            zoom_channel: (&value.zoom_channel).into(),
            zoom_channel_min: value.zoom_channel_min,
            zoom_channel_trim: value.zoom_channel_trim,
            zoom_channel_max: value.zoom_channel_max,
            tilt_channel: (&value.tilt_channel).into(),
            tilt_channel_min: value.tilt_channel_min,
            tilt_channel_trim: value.tilt_channel_trim,
            tilt_channel_max: value.tilt_channel_max,
            tilt_channel_reversed: value.tilt_channel_reversed,
            tilt_mnt_type: (&value.tilt_mnt_type).into(),
            tilt_mnt_pitch_min: value.tilt_mnt_pitch_min,
            tilt_mnt_pitch_max: value.tilt_mnt_pitch_max,
        }
    }
}
impl From<&parameters::ActuatorsParameters> for settings::ActuatorsParameters {
    fn from(value: &parameters::ActuatorsParameters) -> Self {
        settings::ActuatorsParameters {
            lens: (&value.lens).into(),
            camera_id: (&value.camera_id).into(),
            focus_channel: (&value.focus_channel).into(),
            focus_channel_min: value.focus_channel_min,
            focus_channel_trim: value.focus_channel_trim,
            focus_channel_max: value.focus_channel_max,
            focus_margin_gain: value.focus_margin_gain,
            script_function: (&value.script_function).into(),
            script_channel: (&value.script_channel).into(),
            script_channel_min: value.script_channel_min,
            script_channel_trim: value.script_channel_trim,
            script_channel_max: value.script_channel_max,
            enable_focus_and_zoom_correlation: value.enable_focus_and_zoom_correlation,
            zoom_channel: (&value.zoom_channel).into(),
            zoom_channel_min: value.zoom_channel_min,
            zoom_channel_trim: value.zoom_channel_trim,
            zoom_channel_max: value.zoom_channel_max,
            tilt_channel: (&value.tilt_channel).into(),
            tilt_channel_min: value.tilt_channel_min,
            tilt_channel_trim: value.tilt_channel_trim,
            tilt_channel_max: value.tilt_channel_max,
            tilt_channel_reversed: value.tilt_channel_reversed,
            tilt_mnt_type: (&value.tilt_mnt_type).into(),
            tilt_mnt_pitch_min: value.tilt_mnt_pitch_min,
            tilt_mnt_pitch_max: value.tilt_mnt_pitch_max,
        }
    }
}

impl From<&settings::ServoChannel> for api::ServoChannel {
    fn from(value: &settings::ServoChannel) -> Self {
        match value {
            settings::ServoChannel::SERVO1 => Self::SERVO1,
            settings::ServoChannel::SERVO2 => Self::SERVO2,
            settings::ServoChannel::SERVO3 => Self::SERVO3,
            settings::ServoChannel::SERVO4 => Self::SERVO4,
            settings::ServoChannel::SERVO5 => Self::SERVO5,
            settings::ServoChannel::SERVO6 => Self::SERVO6,
            settings::ServoChannel::SERVO7 => Self::SERVO7,
            settings::ServoChannel::SERVO8 => Self::SERVO8,
            settings::ServoChannel::SERVO9 => Self::SERVO9,
            settings::ServoChannel::SERVO10 => Self::SERVO10,
            settings::ServoChannel::SERVO11 => Self::SERVO11,
            settings::ServoChannel::SERVO12 => Self::SERVO12,
            settings::ServoChannel::SERVO13 => Self::SERVO13,
            settings::ServoChannel::SERVO14 => Self::SERVO14,
            settings::ServoChannel::SERVO15 => Self::SERVO15,
            settings::ServoChannel::SERVO16 => Self::SERVO16,
            settings::ServoChannel::SERVO17 => Self::SERVO17,
            settings::ServoChannel::SERVO18 => Self::SERVO18,
            settings::ServoChannel::SERVO19 => Self::SERVO19,
            settings::ServoChannel::SERVO20 => Self::SERVO20,
            settings::ServoChannel::SERVO21 => Self::SERVO21,
            settings::ServoChannel::SERVO22 => Self::SERVO22,
            settings::ServoChannel::SERVO23 => Self::SERVO23,
            settings::ServoChannel::SERVO24 => Self::SERVO24,
            settings::ServoChannel::SERVO25 => Self::SERVO25,
            settings::ServoChannel::SERVO26 => Self::SERVO26,
            settings::ServoChannel::SERVO27 => Self::SERVO27,
            settings::ServoChannel::SERVO28 => Self::SERVO28,
            settings::ServoChannel::SERVO29 => Self::SERVO29,
            settings::ServoChannel::SERVO30 => Self::SERVO30,
            settings::ServoChannel::SERVO31 => Self::SERVO31,
            settings::ServoChannel::SERVO32 => Self::SERVO32,
        }
    }
}
impl From<&api::ServoChannel> for settings::ServoChannel {
    fn from(value: &api::ServoChannel) -> Self {
        match value {
            api::ServoChannel::SERVO1 => Self::SERVO1,
            api::ServoChannel::SERVO2 => Self::SERVO2,
            api::ServoChannel::SERVO3 => Self::SERVO3,
            api::ServoChannel::SERVO4 => Self::SERVO4,
            api::ServoChannel::SERVO5 => Self::SERVO5,
            api::ServoChannel::SERVO6 => Self::SERVO6,
            api::ServoChannel::SERVO7 => Self::SERVO7,
            api::ServoChannel::SERVO8 => Self::SERVO8,
            api::ServoChannel::SERVO9 => Self::SERVO9,
            api::ServoChannel::SERVO10 => Self::SERVO10,
            api::ServoChannel::SERVO11 => Self::SERVO11,
            api::ServoChannel::SERVO12 => Self::SERVO12,
            api::ServoChannel::SERVO13 => Self::SERVO13,
            api::ServoChannel::SERVO14 => Self::SERVO14,
            api::ServoChannel::SERVO15 => Self::SERVO15,
            api::ServoChannel::SERVO16 => Self::SERVO16,
            api::ServoChannel::SERVO17 => Self::SERVO17,
            api::ServoChannel::SERVO18 => Self::SERVO18,
            api::ServoChannel::SERVO19 => Self::SERVO19,
            api::ServoChannel::SERVO20 => Self::SERVO20,
            api::ServoChannel::SERVO21 => Self::SERVO21,
            api::ServoChannel::SERVO22 => Self::SERVO22,
            api::ServoChannel::SERVO23 => Self::SERVO23,
            api::ServoChannel::SERVO24 => Self::SERVO24,
            api::ServoChannel::SERVO25 => Self::SERVO25,
            api::ServoChannel::SERVO26 => Self::SERVO26,
            api::ServoChannel::SERVO27 => Self::SERVO27,
            api::ServoChannel::SERVO28 => Self::SERVO28,
            api::ServoChannel::SERVO29 => Self::SERVO29,
            api::ServoChannel::SERVO30 => Self::SERVO30,
            api::ServoChannel::SERVO31 => Self::SERVO31,
            api::ServoChannel::SERVO32 => Self::SERVO32,
        }
    }
}

impl From<&settings::ScriptFunction> for api::ScriptFunction {
    fn from(value: &settings::ScriptFunction) -> Self {
        match value {
            settings::ScriptFunction::SCRIPT1 => Self::SCRIPT1,
            settings::ScriptFunction::SCRIPT2 => Self::SCRIPT2,
            settings::ScriptFunction::SCRIPT3 => Self::SCRIPT3,
            settings::ScriptFunction::SCRIPT4 => Self::SCRIPT4,
            settings::ScriptFunction::SCRIPT5 => Self::SCRIPT5,
            settings::ScriptFunction::SCRIPT6 => Self::SCRIPT6,
            settings::ScriptFunction::SCRIPT7 => Self::SCRIPT7,
            settings::ScriptFunction::SCRIPT8 => Self::SCRIPT8,
            settings::ScriptFunction::SCRIPT9 => Self::SCRIPT9,
            settings::ScriptFunction::SCRIPT10 => Self::SCRIPT10,
            settings::ScriptFunction::SCRIPT11 => Self::SCRIPT11,
            settings::ScriptFunction::SCRIPT12 => Self::SCRIPT12,
            settings::ScriptFunction::SCRIPT13 => Self::SCRIPT13,
            settings::ScriptFunction::SCRIPT14 => Self::SCRIPT14,
            settings::ScriptFunction::SCRIPT15 => Self::SCRIPT15,
            settings::ScriptFunction::SCRIPT16 => Self::SCRIPT16,
        }
    }
}
impl From<&api::ScriptFunction> for settings::ScriptFunction {
    fn from(value: &api::ScriptFunction) -> Self {
        match value {
            api::ScriptFunction::SCRIPT1 => Self::SCRIPT1,
            api::ScriptFunction::SCRIPT2 => Self::SCRIPT2,
            api::ScriptFunction::SCRIPT3 => Self::SCRIPT3,
            api::ScriptFunction::SCRIPT4 => Self::SCRIPT4,
            api::ScriptFunction::SCRIPT5 => Self::SCRIPT5,
            api::ScriptFunction::SCRIPT6 => Self::SCRIPT6,
            api::ScriptFunction::SCRIPT7 => Self::SCRIPT7,
            api::ScriptFunction::SCRIPT8 => Self::SCRIPT8,
            api::ScriptFunction::SCRIPT9 => Self::SCRIPT9,
            api::ScriptFunction::SCRIPT10 => Self::SCRIPT10,
            api::ScriptFunction::SCRIPT11 => Self::SCRIPT11,
            api::ScriptFunction::SCRIPT12 => Self::SCRIPT12,
            api::ScriptFunction::SCRIPT13 => Self::SCRIPT13,
            api::ScriptFunction::SCRIPT14 => Self::SCRIPT14,
            api::ScriptFunction::SCRIPT15 => Self::SCRIPT15,
            api::ScriptFunction::SCRIPT16 => Self::SCRIPT16,
        }
    }
}

impl From<&settings::LensType> for api::LensType {
    fn from(value: &settings::LensType) -> Self {
        match value {
            settings::LensType::Motorized => Self::Motorized,
            settings::LensType::FixedFocus => Self::FixedFocus,
        }
    }
}
impl From<&api::LensType> for settings::LensType {
    fn from(value: &api::LensType) -> Self {
        match value {
            api::LensType::Motorized => Self::Motorized,
            api::LensType::FixedFocus => Self::FixedFocus,
        }
    }
}

impl From<&settings::CameraID> for api::CameraID {
    fn from(value: &settings::CameraID) -> Self {
        match value {
            settings::CameraID::CAM1 => Self::CAM1,
            settings::CameraID::CAM2 => Self::CAM2,
        }
    }
}
impl From<&api::CameraID> for settings::CameraID {
    fn from(value: &api::CameraID) -> Self {
        match value {
            api::CameraID::CAM1 => Self::CAM1,
            api::CameraID::CAM2 => Self::CAM2,
        }
    }
}

impl From<&settings::MountType> for api::MountType {
    fn from(value: &settings::MountType) -> Self {
        match value {
            settings::MountType::Servo => Self::Servo,
            settings::MountType::BrushlessPWM => Self::BrushlessPWM,
        }
    }
}
impl From<&api::MountType> for settings::MountType {
    fn from(value: &api::MountType) -> Self {
        match value {
            api::MountType::Servo => Self::Servo,
            api::MountType::BrushlessPWM => Self::BrushlessPWM,
        }
    }
}

impl From<&settings::FocusZoomPoints> for api::FocusZoomPoints {
    fn from(value: &settings::FocusZoomPoints) -> Self {
        Self(value.iter().map(|v| v.into()).collect())
    }
}
impl From<&api::FocusZoomPoints> for settings::FocusZoomPoints {
    fn from(value: &api::FocusZoomPoints) -> Self {
        value.0.iter().map(settings::FocusZoomPoint::from).collect()
    }
}

impl From<&settings::FocusZoomPoint> for api::FocusZoomPoint {
    fn from(value: &settings::FocusZoomPoint) -> Self {
        Self {
            zoom: value.zoom,
            focus: value.focus,
        }
    }
}
impl From<&api::FocusZoomPoint> for settings::FocusZoomPoint {
    fn from(value: &api::FocusZoomPoint) -> Self {
        Self {
            focus: value.focus,
            zoom: value.zoom,
        }
    }
}

impl From<&settings::ActuatorsState> for api::ActuatorsState {
    fn from(value: &settings::ActuatorsState) -> Self {
        Self {
            focus: value.focus,
            zoom: value.zoom,
            tilt: value.tilt,
        }
    }
}
impl From<&api::ActuatorsState> for settings::ActuatorsState {
    fn from(value: &api::ActuatorsState) -> Self {
        Self {
            focus: value.focus,
            zoom: value.zoom,
            tilt: value.tilt,
        }
    }
}
