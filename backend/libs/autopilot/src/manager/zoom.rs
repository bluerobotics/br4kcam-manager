use anyhow::{Context, Result};
use indexmap::IndexMap;
use tracing::*;
use uuid::Uuid;

use crate::{
    api, generate_update_channel_param_function,
    manager::Manager,
    parameters::{ActuatorsParameters, ChannelFunction, ParamType},
};

impl Manager {
    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_zoom_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        overwrite: bool,
    ) -> Result<bool> {
        let mut autopilot_reboot_required = overwrite;

        if let Some(channel) = &parameters.zoom_channel {
            // Snapshot under a short write with no await inside, so the MAVLink I/O
            // below never runs while MANAGER is locked.
            let old_channel = {
                let mut manager = crate::manager::MANAGER
                    .get()
                    .context("Not available")?
                    .write()
                    .await;
                manager
                    .settings
                    .actuators
                    .entry(*camera_uuid)
                    .or_default()
                    .parameters
                    .zoom_channel
            };

            let mavlink = crate::mavlink::component()?;
            let encoding = mavlink.encoding().await;

            // Disables the old zoom_channel:
            if &old_channel != channel {
                let param_name = format!("SERVO{}_FUNCTION", old_channel as u8);

                let mut param = mavlink.get_param(&param_name, false).await?;
                let old_value = param.value;
                param
                    .value
                    .set_value(ParamType::INT16(ChannelFunction::Disabled as i16), encoding)?;
                let new_value = param.value;

                if old_value != new_value {
                    match mavlink.set_param(param).await {
                        Ok(_) => {
                            info!(
                                "zoom_channel (SERVO{}) changed from {:?} to {new_value:?}",
                                old_channel as u8, old_value
                            );
                            autopilot_reboot_required = true;
                        }
                        Err(error) => {
                            return Err(error).context(
                                "Failed to disable the old zoom channel when setting parameter",
                            );
                        }
                    }
                }
            }

            // Sets the new zoom_channel:
            {
                let param_name = format!("SERVO{}_FUNCTION", *channel as u8);

                let mut param = mavlink.get_param(&param_name, false).await?;
                let old_value = param.value;
                param.value.set_value(
                    ParamType::INT16(Self::zoom_channel_function() as i16),
                    encoding,
                )?;
                let new_value = param.value;

                if overwrite || old_value != new_value {
                    match mavlink.set_param(param).await {
                        Ok(_) => {
                            info!(
                                "zoom_channel (SERVO{}) changed from {:?} to {new_value:?}",
                                *channel as u8, old_value
                            );

                            let mut manager = crate::manager::MANAGER
                                .get()
                                .context("Not available")?
                                .write()
                                .await;
                            if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid)
                            {
                                actuators.parameters.zoom_channel = *channel;
                            }
                            autopilot_reboot_required = true;
                        }
                        Err(error) => {
                            return Err(error).context(
                                "Failed setting new zoom channel parameter when setting parameter",
                            );
                        }
                    }
                }
            }
        }

        Self::update_zoom_channel_parameters(camera_uuid, parameters, autopilot_reboot_required)
            .await?;

        Ok(autopilot_reboot_required)
    }

    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_zoom_channel_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        force_apply: bool,
    ) -> Result<()> {
        Self::update_zoom_channel_min(camera_uuid, parameters, force_apply).await?;
        Self::update_zoom_channel_trim(camera_uuid, parameters, force_apply).await?;
        Self::update_zoom_channel_max(camera_uuid, parameters, force_apply).await?;

        Ok(())
    }

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

    generate_update_channel_param_function!(
        update_zoom_channel_min,
        zoom_channel_min,
        "SERVO",
        "MIN",
        UINT16,
        zoom_channel
    );

    generate_update_channel_param_function!(
        update_zoom_channel_max,
        zoom_channel_max,
        "SERVO",
        "MAX",
        UINT16,
        zoom_channel
    );

    generate_update_channel_param_function!(
        update_zoom_channel_trim,
        zoom_channel_trim,
        "SERVO",
        "TRIM",
        UINT16,
        zoom_channel
    );
}

pub(super) fn push_owned_expectations(
    parameters: &ActuatorsParameters,
    map: &mut IndexMap<String, ParamType>,
) {
    Manager::expect_owned_zoom_servo_function(parameters, map);
}
