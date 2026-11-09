use indexmap::IndexMap;

use crate::{
    manager::{
        Manager,
        plan::{Change, Planner},
    },
    parameters::{ActuatorsParameters, ParamType, Setup},
};

impl Manager {
    fn expect_owned_zoom_servo_function(
        parameters: &ActuatorsParameters,
        setup: Setup,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let channel = parameters.zoom_channel as u8;
        map.insert(
            format!("SERVO{channel}_FUNCTION"),
            ParamType::INT16(parameters.zoom_function(setup) as i16),
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
        setup,
        ..
    } = *change;
    let mut written = overwrite;

    let mut channel_moved = false;
    if incoming.zoom_channel.is_some() || change.functions_moved() {
        written |=
            planner.channel_function(next.zoom_channel, next.zoom_function(setup), overwrite);
        channel_moved = current.zoom_channel != next.zoom_channel;
        if overwrite || channel_moved {
            planner.clear_reversed(next.zoom_channel);
        }
    }
    // A channel the role moves to has none of its settings yet.
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
        written || channel_moved,
    );

    written
}

pub(super) fn push_owned_expectations(
    parameters: &ActuatorsParameters,
    setup: Setup,
    map: &mut IndexMap<String, ParamType>,
) {
    Manager::expect_owned_zoom_servo_function(parameters, setup, map);
}
