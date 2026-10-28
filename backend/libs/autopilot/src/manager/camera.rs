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

/// `CAM{n}_TYPE`: the camera's id gets the servo camera backend, and an id it moves away
/// from is turned off. Returns whether a type is written.
pub(super) fn plan(planner: &mut Planner, change: &Change) -> bool {
    let Change {
        current,
        next,
        incoming,
        overwrite,
    } = *change;
    let mut written = overwrite;

    if incoming.camera_id.is_some() {
        if current.camera_id != next.camera_id {
            written |= planner.turn_off(
                format!("CAM{}_TYPE", current.camera_id as u8),
                ParamType::UINT8(CameraType::None as u8),
            );
        }
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
