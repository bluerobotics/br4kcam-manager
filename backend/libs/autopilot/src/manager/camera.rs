use indexmap::IndexMap;

use crate::{
    manager::{
        Manager,
        plan::{Change, Planner},
    },
    parameters::{ActuatorsParameters, CameraType, ParamType},
};

impl Manager {
    fn enabled_camera_type_value() -> ParamType {
        ParamType::UINT8(CameraType::Servo as u8)
    }
}

/// `CAM{n}_TYPE`: the camera's id gets the servo camera backend. Returns whether the type is
/// written.
pub(super) fn plan(planner: &mut Planner, change: &Change) -> bool {
    let Change {
        next,
        incoming,
        overwrite,
        ..
    } = *change;
    let mut written = overwrite;

    // The camera id the camera leaves is released by `plan_apply`.
    if incoming.camera_id.is_some() {
        written |= planner.write_if_needed(
            format!("CAM{}_TYPE", next.camera_id as u8),
            Manager::enabled_camera_type_value(),
            overwrite,
        );
    }

    written
}

pub(super) fn push_owned_expectations(
    parameters: &ActuatorsParameters,
    map: &mut IndexMap<String, ParamType>,
) {
    let camera = parameters.camera_id as u8;
    map.insert(
        format!("CAM{camera}_TYPE"),
        Manager::enabled_camera_type_value(),
    );
}
