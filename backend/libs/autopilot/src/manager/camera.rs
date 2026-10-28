use anyhow::{Context, Result};
use indexmap::IndexMap;
use tracing::*;
use uuid::Uuid;

use crate::{
    api,
    manager::Manager,
    parameters::{ActuatorsParameters, CameraType, ParamType},
};

impl Manager {
    fn enabled_camera_type_value() -> ParamType {
        ParamType::UINT8(CameraType::Servo as u8)
    }

    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_camera_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        overwrite: bool,
    ) -> Result<bool> {
        let mut autopilot_reboot_required = overwrite;

        if let Some(camera_id) = &parameters.camera_id {
            // Snapshot under a short write with no await inside, so the MAVLink I/O
            // below never runs while MANAGER is locked.
            let old_camera_id = {
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
                    .camera_id
            };

            let mavlink = crate::mavlink::component()?;
            let encoding = mavlink.encoding().await;

            // Disables the old camera_id:
            if &old_camera_id != camera_id {
                let param_name = format!("CAM{}_TYPE", old_camera_id as u8);

                let mut param = mavlink.get_param(&param_name, false).await?;
                let old_value = param.value;
                param
                    .value
                    .set_value(ParamType::UINT8(CameraType::None as u8), encoding)?;
                let new_value = param.value;

                if old_value != new_value {
                    match mavlink.set_param(param).await {
                        Ok(_) => {
                            info!(
                                "camera_id (CAM{}) changed from {:?} to {new_value:?}",
                                old_camera_id as u8, old_value
                            );
                            autopilot_reboot_required = true;
                        }
                        Err(error) => {
                            return Err(error).context(
                                "Failed to disable the old camera camera_id when setting parameter",
                            );
                        }
                    }
                }
            }

            // Sets the new camera_id:
            {
                let param_name = format!("CAM{}_TYPE", *camera_id as u8);

                let mut param = mavlink.get_param(&param_name, false).await?;
                let old_value = param.value;
                param
                    .value
                    .set_value(Self::enabled_camera_type_value(), encoding)?;
                let new_value = param.value;

                if overwrite || old_value != new_value {
                    match mavlink.set_param(param).await {
                        Ok(_) => {
                            info!(
                                "camera_id (CAM{}) changed from {:?} to {new_value:?}",
                                *camera_id as u8, old_value
                            );

                            let mut manager = crate::manager::MANAGER
                                .get()
                                .context("Not available")?
                                .write()
                                .await;
                            if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid)
                            {
                                actuators.parameters.camera_id = *camera_id;
                            }
                            autopilot_reboot_required = true;
                        }
                        Err(error) => {
                            return Err(error).context(
                                "Failed setting new camera camera_id parameter when setting parameter",
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
                        actuators.parameters.camera_id = *camera_id;
                    }
                }
            }
        }

        Ok(autopilot_reboot_required)
    }
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
