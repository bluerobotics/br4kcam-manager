use anyhow::{Context, Result};
use indexmap::IndexMap;
use tracing::*;
use uuid::Uuid;

use crate::{
    api, generate_update_channel_param_function,
    manager::{Manager, store_parameters, write_param},
    parameters::{ActuatorsParameters, ChannelFunction, ParamType},
};

impl Manager {
    /// The channel the role leaves is turned off by [`Manager::update_config`].
    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_focus_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        overwrite: bool,
    ) -> Result<bool> {
        let mut autopilot_reboot_required = overwrite;

        if let Some(channel) = parameters.focus_channel {
            let saved = Self::saved_parameters(camera_uuid).await?;
            let script_function = parameters.script_function.unwrap_or(saved.script_function);
            // The focus servo input is the script:
            let function = Self::focus_channel_function(script_function)?;
            autopilot_reboot_required |= write_param(
                &format!("SERVO{}_FUNCTION", channel as u8),
                ParamType::INT16(function as i16),
                overwrite,
            )
            .await
            .context("Failed setting the focus channel function")?;
            store_parameters(camera_uuid, |stored| {
                stored.focus_channel = channel;
                stored.script_function = script_function;
            })
            .await?;
        } else if let Some(script_function) = parameters.script_function {
            autopilot_reboot_required |=
                Self::apply_script_function(camera_uuid, script_function, overwrite).await?;
        }

        Self::update_focus_channel_parameters(camera_uuid, parameters, autopilot_reboot_required)
            .await?;

        Ok(autopilot_reboot_required)
    }

    async fn apply_script_function(
        camera_uuid: &Uuid,
        script_function: api::ScriptFunction,
        overwrite: bool,
    ) -> Result<bool> {
        let (channel, stored_script_function) = {
            let mut manager = crate::manager::MANAGER
                .get()
                .context("Not available")?
                .write()
                .await;
            let current_parameters = &mut manager
                .settings
                .actuators
                .entry(*camera_uuid)
                .or_default()
                .parameters;
            (
                current_parameters.focus_channel,
                current_parameters.script_function,
            )
        };

        if !overwrite && stored_script_function == script_function {
            return Ok(false);
        }

        let function = Self::focus_channel_function(script_function)?;
        let param_name = format!("SERVO{}_FUNCTION", channel as u8);
        let mavlink = crate::mavlink::component()?;
        let encoding = mavlink.encoding().await;
        let mut param = mavlink.get_param(&param_name, false).await?;
        let old_value = param.value;
        param
            .value
            .set_value(ParamType::INT16(function as i16), encoding)?;
        let new_value = param.value;

        if overwrite || old_value != new_value {
            mavlink.set_param(param).await.with_context(|| {
                format!("Failed setting {param_name} to script_function {script_function:?}")
            })?;
        }

        let mut manager = crate::manager::MANAGER
            .get()
            .context("Not available")?
            .write()
            .await;
        if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid) {
            actuators.parameters.script_function = script_function;
        }

        Ok(overwrite || old_value != new_value)
    }

    #[instrument(level = "debug", skip(parameters))]
    async fn update_focus_channel_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        force_apply: bool,
    ) -> Result<()> {
        Self::update_focus_channel_min(camera_uuid, parameters, force_apply).await?;
        Self::update_focus_channel_trim(camera_uuid, parameters, force_apply).await?;
        Self::update_focus_channel_max(camera_uuid, parameters, force_apply).await?;

        Ok(())
    }

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

    generate_update_channel_param_function!(
        update_focus_channel_min,
        focus_channel_min,
        "SERVO",
        "MIN",
        UINT16,
        focus_channel
    );

    generate_update_channel_param_function!(
        update_focus_channel_max,
        focus_channel_max,
        "SERVO",
        "MAX",
        UINT16,
        focus_channel
    );

    generate_update_channel_param_function!(
        update_focus_channel_trim,
        focus_channel_trim,
        "SERVO",
        "TRIM",
        UINT16,
        focus_channel
    );
}

pub(super) fn push_owned_expectations(
    parameters: &ActuatorsParameters,
    map: &mut IndexMap<String, ParamType>,
) {
    Manager::expect_owned_focus_servo_function(parameters, map);
}
