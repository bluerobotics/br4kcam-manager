use indexmap::IndexMap;

use crate::{
    manager::{
        Manager,
        plan::{Change, Planner},
    },
    parameters::{ActuatorsParameters, ChannelFunction, ParamType},
};

impl Manager {
    fn zoom_channel_function() -> ChannelFunction {
        ChannelFunction::CameraZoom
    }

    fn expect_owned_zoom_servo_function(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let channel = parameters.zoom_channel as u8;
        map.insert(
            format!("SERVO{channel}_FUNCTION"),
            ParamType::INT16(Self::zoom_channel_function() as i16),
        );
    }
}

/// The zoom servo: `SERVO{zoom_channel}_FUNCTION` and its PWM range. Returns whether the
/// function is written.
pub(super) fn plan(planner: &mut Planner, change: &Change) -> bool {
    let Change {
        current,
        next,
        incoming,
        overwrite,
    } = *change;
    let mut written = overwrite;

    if incoming.zoom_channel.is_some() {
        written |= planner.channel_function(
            current.zoom_channel,
            next.zoom_channel,
            Manager::zoom_channel_function(),
            overwrite,
        );
    }
    planner.channel_range(
        next.zoom_channel,
        [
            current.zoom_channel_min,
            current.zoom_channel_trim,
            current.zoom_channel_max,
        ],
        [
            next.zoom_channel_min,
            next.zoom_channel_trim,
            next.zoom_channel_max,
        ],
        written,
    );

    written
}

pub(super) fn push_owned_expectations(
    parameters: &ActuatorsParameters,
    map: &mut IndexMap<String, ParamType>,
) {
    Manager::expect_owned_zoom_servo_function(parameters, map);
}
