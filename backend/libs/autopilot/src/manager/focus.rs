use anyhow::Result;
use indexmap::IndexMap;

use crate::{
    api,
    manager::{
        Manager,
        plan::{Change, Planner, servo_param},
    },
    parameters::{ActuatorsParameters, ChannelFunction, ParamType},
};

impl Manager {
    fn focus_channel_function(script_function: api::ScriptFunction) -> Result<ChannelFunction> {
        ChannelFunction::try_from(script_function as u8 as i16).map_err(|error| {
            anyhow::anyhow!(
                "Invalid script_function {script_function:?} for focus channel: {error:?}"
            )
        })
    }

    fn expect_owned_focus_servo_function(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let Ok(function) = Self::focus_channel_function(parameters.script_function) else {
            return;
        };
        let channel = parameters.focus_channel as u8;
        map.insert(
            format!("SERVO{channel}_FUNCTION"),
            ParamType::INT16(function as i16),
        );
    }
}

/// The focus servo, driven by the script: `SERVO{focus_channel}_FUNCTION` and its PWM range.
/// Returns whether the function is written.
pub(super) fn plan(planner: &mut Planner, change: &Change) -> Result<bool> {
    let Change {
        current,
        next,
        incoming,
        overwrite,
    } = *change;
    let mut written = overwrite;

    if incoming.focus_channel.is_some() {
        // The focus servo input is the script:
        let function = Manager::focus_channel_function(next.script_function)?;
        written |= planner.channel_function(
            current.focus_channel,
            next.focus_channel,
            function,
            overwrite,
        );
    } else if incoming.script_function.is_some()
        && (overwrite || current.script_function != next.script_function)
    {
        let function = Manager::focus_channel_function(next.script_function)?;
        written |= planner.write_if_needed(
            servo_param(next.focus_channel, "FUNCTION"),
            ParamType::INT16(function as i16),
            overwrite,
        );
    }
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
        written,
    );

    Ok(written)
}

pub(super) fn push_owned_expectations(
    parameters: &ActuatorsParameters,
    map: &mut IndexMap<String, ParamType>,
) {
    Manager::expect_owned_focus_servo_function(parameters, map);
}
