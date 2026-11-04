use anyhow::{Context, Result};
use indexmap::IndexMap;
use tracing::*;
use uuid::Uuid;

use crate::{
    api, generate_update_channel_param_function,
    manager::{Manager, clear_channel_reversed, store_parameters, write_param},
    parameters::{ActuatorsParameters, ChannelFunction, ParamType},
};

impl Manager {
    /// The channel the role leaves is turned off by [`Manager::update_config`].
    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_zoom_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        overwrite: bool,
    ) -> Result<bool> {
        let mut autopilot_reboot_required = overwrite;
        let mut channel_moved = false;

        if let Some(channel) = parameters.zoom_channel {
            let old_channel = Self::saved_parameters(camera_uuid).await?.zoom_channel;
            autopilot_reboot_required |= write_param(
                &format!("SERVO{}_FUNCTION", channel as u8),
                ParamType::INT16(Self::zoom_channel_function() as i16),
                overwrite,
            )
            .await
            .context("Failed setting the zoom channel function")?;
            channel_moved = old_channel != channel;
            if overwrite || channel_moved {
                clear_channel_reversed(channel).await?;
            }
            store_parameters(camera_uuid, |stored| stored.zoom_channel = channel).await?;
        }

        // A channel the role moves to has none of its settings yet.
        Self::update_zoom_channel_parameters(
            camera_uuid,
            parameters,
            autopilot_reboot_required || channel_moved,
        )
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
