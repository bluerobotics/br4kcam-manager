use indexmap::IndexMap;

use crate::{
    manager::{
        Manager,
        plan::{Change, Planner, servo_param},
    },
    parameters::{ActuatorsParameters, ParamType, Setup},
};

impl Manager {
    fn expect_owned_focus_servo_function(
        parameters: &ActuatorsParameters,
        setup: Setup,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let channel = parameters.focus_channel as u8;
        map.insert(
            format!("SERVO{channel}_FUNCTION"),
            ParamType::INT16(parameters.focus_function(setup) as i16),
        );
    }
}

/// The focus servo, driven by the script: `SERVO{focus_channel}_FUNCTION` and its PWM range.
/// Returns whether the function is written.
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
    if incoming.focus_channel.is_some() || change.functions_moved() {
        // The focus servo input is the script:
        written |=
            planner.channel_function(next.focus_channel, next.focus_function(setup), overwrite);
        channel_moved = current.focus_channel != next.focus_channel;
        if overwrite || channel_moved {
            planner.clear_reversed(next.focus_channel);
        }
    } else if incoming.script_function.is_some()
        && (overwrite || current.script_function != next.script_function)
    {
        written |= planner.write_if_needed(
            servo_param(next.focus_channel, "FUNCTION"),
            ParamType::INT16(next.focus_function(setup) as i16),
            overwrite,
        );
    }
    // A channel the role moves to has none of its settings yet.
    planner.channel_range(
        next.focus_channel,
        [
            current.focus_channel_min,
            current.focus_channel_trim,
            current.focus_channel_max,
        ],
        [
            next.focus_channel_min,
            next.focus_channel_trim,
            next.focus_channel_max,
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
    Manager::expect_owned_focus_servo_function(parameters, setup, map);
}
