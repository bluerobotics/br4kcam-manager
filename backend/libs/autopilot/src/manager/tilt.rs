use indexmap::IndexMap;
use settings::TiltChannelFunction;

use crate::{
    api,
    manager::{
        Manager,
        plan::{Change, Planner, servo_param},
    },
    parameters::{ActuatorsParameters, ChannelFunction, ParamType},
};

pub(super) fn tilt_mount_id(camera_id: api::CameraID) -> TiltChannelFunction {
    match camera_id {
        api::CameraID::CAM1 => TiltChannelFunction::MNT1,
        api::CameraID::CAM2 => TiltChannelFunction::MNT2,
    }
}

impl Manager {
    fn tilt_channel_function(camera_id: api::CameraID) -> ChannelFunction {
        match camera_id {
            api::CameraID::CAM1 => ChannelFunction::Mount1Pitch,
            api::CameraID::CAM2 => ChannelFunction::Mount2Pitch,
        }
    }

    fn expect_owned_tilt_servo_function(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let channel = parameters.tilt_channel as u8;
        let function = Self::tilt_channel_function(parameters.camera_id);
        map.insert(
            format!("SERVO{channel}_FUNCTION"),
            ParamType::INT16(function as i16),
        );
    }

    fn expect_owned_tilt_servo_reversed(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let channel = parameters.tilt_channel as u8;
        map.insert(
            format!("SERVO{channel}_REVERSED"),
            ParamType::UINT8(parameters.tilt_channel_reversed as u8),
        );
    }

    fn expect_owned_tilt_mnt_type(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let mount_id = tilt_mount_id(parameters.camera_id);
        map.insert(
            format!("{mount_id:?}_TYPE"),
            ParamType::INT32(parameters.tilt_mnt_type as i32),
        );
    }

    fn expect_owned_tilt_mnt_pitch_min(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let mount_id = tilt_mount_id(parameters.camera_id);
        map.insert(
            format!("{mount_id:?}_PITCH_MIN"),
            ParamType::INT32(parameters.tilt_mnt_pitch_min),
        );
    }

    fn expect_owned_tilt_mnt_pitch_max(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let mount_id = tilt_mount_id(parameters.camera_id);
        map.insert(
            format!("{mount_id:?}_PITCH_MAX"),
            ParamType::INT32(parameters.tilt_mnt_pitch_max),
        );
    }
}

/// The tilt servo and its mount: `SERVO{tilt_channel}_FUNCTION`, its PWM range and direction,
/// and `MNT{n}_PITCH_MIN`, `_PITCH_MAX` and `_TYPE`. Returns whether a function or the mount
/// type is written, which only take effect after a reboot.
pub(super) fn plan(planner: &mut Planner, change: &Change) -> bool {
    let Change {
        current,
        next,
        incoming,
        overwrite,
    } = *change;
    let mut written = overwrite;

    if incoming.tilt_channel.is_some() {
        written |= planner.channel_function(
            current.tilt_channel,
            next.tilt_channel,
            Manager::tilt_channel_function(next.camera_id),
            overwrite,
        );
    }

    let force = written;
    planner.channel_range(
        next.tilt_channel,
        [
            current.tilt_channel_min,
            current.tilt_channel_trim,
            current.tilt_channel_max,
        ],
        [
            next.tilt_channel_min,
            next.tilt_channel_trim,
            next.tilt_channel_max,
        ],
        force,
    );

    let mount_id = tilt_mount_id(next.camera_id);
    planner.write_if_changed(
        format!("{mount_id:?}_PITCH_MIN"),
        current.tilt_mnt_pitch_min,
        next.tilt_mnt_pitch_min,
        ParamType::INT32(next.tilt_mnt_pitch_min),
        force,
    );
    planner.write_if_changed(
        format!("{mount_id:?}_PITCH_MAX"),
        current.tilt_mnt_pitch_max,
        next.tilt_mnt_pitch_max,
        ParamType::INT32(next.tilt_mnt_pitch_max),
        force,
    );
    planner.write_if_changed(
        servo_param(next.tilt_channel, "REVERSED"),
        current.tilt_channel_reversed,
        next.tilt_channel_reversed,
        ParamType::UINT8(next.tilt_channel_reversed as u8),
        force,
    );

    // Mount type changes require an autopilot reboot to take effect.
    if incoming.tilt_mnt_type.is_some() || force {
        written |= planner.write_if_needed(
            format!("{mount_id:?}_TYPE"),
            ParamType::INT32(next.tilt_mnt_type as i32),
            force,
        );
    }

    written
}

pub(super) fn push_owned_expectations(
    parameters: &ActuatorsParameters,
    map: &mut IndexMap<String, ParamType>,
) {
    Manager::expect_owned_tilt_servo_function(parameters, map);
    Manager::expect_owned_tilt_servo_reversed(parameters, map);
    Manager::expect_owned_tilt_mnt_pitch_min(parameters, map);
    Manager::expect_owned_tilt_mnt_pitch_max(parameters, map);
    Manager::expect_owned_tilt_mnt_type(parameters, map);
}
