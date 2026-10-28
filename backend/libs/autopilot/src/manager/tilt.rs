use std::collections::HashSet;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use settings::TiltChannelFunction;
use tracing::*;
use uuid::Uuid;

use crate::{
    api, generate_update_channel_param_function, generate_update_mount_param_function,
    manager::Manager,
    parameters::{ActuatorsParameters, ChannelFunction, ParamType},
};

pub(super) fn tilt_mount_id(camera_id: api::CameraID) -> TiltChannelFunction {
    match camera_id {
        api::CameraID::CAM1 => TiltChannelFunction::MNT1,
        api::CameraID::CAM2 => TiltChannelFunction::MNT2,
    }
}

impl Manager {
    /// The old channel is only turned off when nothing in `kept` uses it.
    #[instrument(level = "debug", skip(parameters, kept))]
    pub async fn update_tilt_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        overwrite: bool,
        kept: &HashSet<String>,
    ) -> Result<bool> {
        let mut autopilot_reboot_required = overwrite;

        if let Some(channel) = &parameters.tilt_channel {
            // Snapshot under a short write with no await inside, so the MAVLink I/O
            // below never runs while MANAGER is locked.
            let (old_channel, camera_id) = {
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
                    current_parameters.tilt_channel,
                    current_parameters.camera_id,
                )
            };

            let mavlink = crate::mavlink::component()?;
            let encoding = mavlink.encoding().await;

            // Disables the old tilt_channel:
            if &old_channel != channel
                && !kept.contains(&format!("SERVO{}_FUNCTION", old_channel as u8))
            {
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
                                "tilt_channel (SERVO{}) changed from {:?} to {new_value:?}",
                                old_channel as u8, old_value
                            );
                            autopilot_reboot_required = true;
                        }
                        Err(error) => {
                            return Err(error).context(
                                "Failed to disable the old tilt channel when setting parameter",
                            );
                        }
                    }
                }
            }

            // Sets the new tilt_channel:
            {
                let param_name = format!("SERVO{}_FUNCTION", *channel as u8);
                let function = Self::tilt_channel_function(camera_id);

                let mut param = mavlink.get_param(&param_name, false).await?;
                let old_value = param.value;
                param
                    .value
                    .set_value(ParamType::INT16(function as i16), encoding)?;
                let new_value = param.value;

                if overwrite || old_value != new_value {
                    match mavlink.set_param(param).await {
                        Ok(_) => {
                            info!(
                                "tilt_channel (SERVO{}) changed from {:?} to {new_value:?}",
                                *channel as u8, old_value
                            );

                            let mut manager = crate::manager::MANAGER
                                .get()
                                .context("Not available")?
                                .write()
                                .await;
                            if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid)
                            {
                                actuators.parameters.tilt_channel = *channel;
                            }
                            autopilot_reboot_required = true;
                        }
                        Err(error) => {
                            return Err(error).context(
                                "Failed setting new tilt channel parameter when setting parameter",
                            );
                        }
                    }
                } else {
                    // Already held, the role still moves to this channel.
                    let mut manager = crate::manager::MANAGER
                        .get()
                        .context("Not available")?
                        .write()
                        .await;
                    if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid) {
                        actuators.parameters.tilt_channel = *channel;
                    }
                }
            }
        }

        Self::update_tilt_channel_parameters(camera_uuid, parameters, autopilot_reboot_required)
            .await
            .map(|reboot| autopilot_reboot_required | reboot)
    }
    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_tilt_channel_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        force_apply: bool,
    ) -> Result<bool> {
        Self::update_tilt_channel_min(camera_uuid, parameters, force_apply).await?;
        Self::update_tilt_channel_trim(camera_uuid, parameters, force_apply).await?;
        Self::update_tilt_channel_max(camera_uuid, parameters, force_apply).await?;

        Self::update_tilt_mnt_pitch_min(camera_uuid, parameters, force_apply).await?;
        Self::update_tilt_mnt_pitch_max(camera_uuid, parameters, force_apply).await?;
        Self::update_tilt_channel_reversed(camera_uuid, parameters, force_apply).await?;
        Self::update_tilt_mnt_type(camera_uuid, parameters, force_apply).await
    }

    #[instrument(level = "debug", skip(parameters))]
    async fn update_tilt_channel_reversed(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        force_apply: bool,
    ) -> Result<()> {
        let (param_name, new_value, old_value) = {
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
            let channel = current_parameters.tilt_channel as u8;
            let param_name = format!("SERVO{channel}_REVERSED");
            let new_value = match (parameters.tilt_channel_reversed, force_apply) {
                (Some(value), _) => value,
                (None, true) => current_parameters.tilt_channel_reversed,
                (None, false) => return Ok(()),
            };
            (
                param_name,
                new_value,
                current_parameters.tilt_channel_reversed,
            )
        };

        if old_value == new_value && !force_apply {
            trace!("Parameter {param_name:?} skipped");
            return Ok(());
        }

        let mavlink = crate::mavlink::component()?;
        let encoding = mavlink.encoding().await;
        let mut param = mavlink.get_param(&param_name, false).await?;
        param
            .value
            .set_value(ParamType::UINT8(new_value as u8), encoding)?;
        mavlink
            .set_param(param)
            .await
            .context("Failed setting tilt channel reversed parameter")?;

        if old_value != new_value {
            info!("tilt_channel_reversed changed from {old_value} to {new_value}");
        }
        let mut manager = crate::manager::MANAGER
            .get()
            .context("Not available")?
            .write()
            .await;
        if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid) {
            actuators.parameters.tilt_channel_reversed = new_value;
        }
        Ok(())
    }

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

    generate_update_channel_param_function!(
        update_tilt_channel_min,
        tilt_channel_min,
        "SERVO",
        "MIN",
        UINT16,
        tilt_channel
    );

    generate_update_channel_param_function!(
        update_tilt_channel_max,
        tilt_channel_max,
        "SERVO",
        "MAX",
        UINT16,
        tilt_channel
    );

    generate_update_channel_param_function!(
        update_tilt_channel_trim,
        tilt_channel_trim,
        "SERVO",
        "TRIM",
        UINT16,
        tilt_channel
    );

    generate_update_mount_param_function!(
        update_tilt_mnt_pitch_min,
        expect_owned_tilt_mnt_pitch_min,
        tilt_mnt_pitch_min,
        "PITCH_MIN",
        INT32
    );

    generate_update_mount_param_function!(
        update_tilt_mnt_pitch_max,
        expect_owned_tilt_mnt_pitch_max,
        tilt_mnt_pitch_max,
        "PITCH_MAX",
        INT32
    );

    #[instrument(level = "debug")]
    pub async fn update_tilt_mnt_type(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        force_apply: bool,
    ) -> Result<bool> {
        let (param_name, new_value, old_value) = {
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

            let mount_id = tilt_mount_id(current_parameters.camera_id);
            let param_name = format!("{mount_id:?}_TYPE");

            let new_value = match (parameters.tilt_mnt_type, force_apply) {
                (Some(value), _) => value,
                (None, true) => current_parameters.tilt_mnt_type,
                (None, false) => return Ok(false),
            };

            (param_name, new_value, current_parameters.tilt_mnt_type)
        };

        let mavlink = crate::mavlink::component()?;
        let encoding = mavlink.encoding().await;
        let mut param = mavlink.get_param(&param_name, false).await?;
        let old_value_encoded = param.param_value(encoding)?;
        param
            .value
            .set_value(ParamType::INT32(new_value as i32), encoding)?;
        let new_value_encoded = param.param_value(encoding)?;
        if (old_value_encoded != new_value_encoded) || force_apply {
            match mavlink.set_param(param).await {
                Ok(_) => {
                    if old_value_encoded != new_value_encoded {
                        info!(
                            "{} changed from {:?} to {:?}",
                            stringify!(tilt_mnt_type),
                            old_value,
                            new_value
                        );
                    }
                    let mut manager = crate::manager::MANAGER
                        .get()
                        .context("Not available")?
                        .write()
                        .await;
                    if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid) {
                        actuators.parameters.tilt_mnt_type = new_value;
                    }
                    // Mount type changes require an autopilot reboot to take effect.
                    return Ok(old_value_encoded != new_value_encoded || force_apply);
                }
                Err(error) => {
                    return Err(error).context(format!("Failed setting parameter {param_name}"));
                }
            }
        }

        Ok(false)
    }
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
