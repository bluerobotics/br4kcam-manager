use anyhow::{Context, Result};
use indexmap::IndexMap;
use tracing::*;
use uuid::Uuid;

use crate::{
    api,
    manager::{
        Manager,
        plan::{Change, Planner},
        store_parameters, write_param,
    },
    parameters::{ActuatorsParameters, CameraType, ParamType},
};

impl Manager {
    fn enabled_camera_type_value() -> ParamType {
        ParamType::UINT8(CameraType::Servo as u8)
    }

    /// The camera id the camera leaves is turned off by [`Manager::update_config`].
    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_camera_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        overwrite: bool,
    ) -> Result<bool> {
        let mut autopilot_reboot_required = overwrite;

        if let Some(camera_id) = parameters.camera_id {
            autopilot_reboot_required |= write_param(
                &format!("CAM{}_TYPE", camera_id as u8),
                Self::enabled_camera_type_value(),
                overwrite,
            )
            .await
            .context("Failed setting the camera type")?;
            store_parameters(camera_uuid, |stored| stored.camera_id = camera_id).await?;
        }

        Ok(autopilot_reboot_required)
    }
}

/// `CAM{n}_TYPE`: the camera's id gets the servo camera backend. Returns whether the type is
/// written.
pub(super) fn plan(planner: &mut Planner, change: &Change) -> bool {
    let Change {
        next,
        incoming,
        overwrite,
        ..
    } = *change;
    let mut written = overwrite;

    // The camera id the camera leaves is released by `plan_apply`.
    if incoming.camera_id.is_some() {
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
