//! Single entry point for camera and autopilot mutations, so every caller
//! (REST and WebSocket alike) produces the same UI overlay and state updates.

use autopilot::api::{Action as AutopilotAction, ActuatorsControl};
use br4kcam_api::SetupResult;
use br4kcam_commands::{Action as CameraAction, CameraControl};
use serde_json::Value;

use crate::web::{camera_state, camera_ui, connectivity, one_push_awb};

/// Wire / HTTP body when the requested camera UUID is not in the MCM list.
pub(crate) const UNKNOWN_CAMERA: &str = "unknown camera";

/// Failed control with an HTTP/WS status already chosen (avoids stringly 404 matching).
#[derive(Debug)]
pub(crate) struct ControlError {
    pub status: u16,
    pub message: String,
}

impl ControlError {
    fn unknown_camera() -> Self {
        Self {
            status: 404,
            message: UNKNOWN_CAMERA.to_string(),
        }
    }

    fn other(message: String) -> Self {
        Self {
            status: 500,
            message,
        }
    }
}

/// Run a camera control, driving the shared UI overlay and state stream.
#[tracing::instrument(level = "debug", skip_all, fields(%camera_control.camera_uuid))]
pub(crate) async fn camera_control(camera_control: CameraControl) -> Result<Value, ControlError> {
    let camera_uuid = camera_control.camera_uuid;
    // Addressability, not discovery: a camera absent from the MCM list while ONVIF
    // rediscovers still answers its HTTP API at the hostname we last saw it on.
    // `SetImageAdjustmentExAll` ignores `camera_uuid` (nil) and fans out to every camera.
    if !matches!(
        camera_control.action,
        CameraAction::SetImageAdjustmentExAll(_)
    ) && mcm_client::camera_address(&camera_uuid).await.is_none()
    {
        return Err(ControlError::unknown_camera());
    }
    let action = camera_control.action.clone();

    // Concurrent onceAWB while Running/Cooldown is a no-op success (do not restart).
    if matches!(
        &action,
        CameraAction::SetImageAdjustmentEx(params) if params.once_awb == Some(1)
    ) && one_push_awb::is_busy(camera_uuid)
    {
        return Ok(Value::Null);
    }

    camera_ui::start_camera_action(camera_uuid, &action);

    match br4kcam_commands::handle_control(camera_control).await {
        Ok(value) => {
            camera_ui::finish_camera_action(camera_uuid, &action);
            camera_state::emit_camera_control_update(camera_uuid, &action, &value);
            one_push_awb::on_control_success(camera_uuid, &action);
            Ok(value)
        }
        Err(error) => {
            let message = format!("{error:?}");
            camera_ui::fail_camera_action(camera_uuid, &action, &message);
            Err(ControlError::other(message))
        }
    }
}

/// Run an autopilot control, driving the shared UI overlay and state stream.
#[tracing::instrument(level = "debug", skip_all, fields(%actuators_control.camera_uuid))]
pub(crate) async fn autopilot_control(
    actuators_control: ActuatorsControl,
) -> Result<Value, ControlError> {
    let camera_uuid = actuators_control.camera_uuid;
    if mcm_client::camera_address(&camera_uuid).await.is_none()
        && !connectivity::is_expected(camera_uuid).await
        && !matches!(
            actuators_control.action,
            AutopilotAction::ForgetActuatorsConfig
        )
    {
        return Err(ControlError::unknown_camera());
    }
    let action = actuators_control.action.clone();
    camera_ui::start_autopilot_action(camera_uuid, &action);

    let result = match &action {
        AutopilotAction::ResetActuatorsConfig | AutopilotAction::ForceResetActuatorsConfig => {
            let forced = matches!(action, AutopilotAction::ForceResetActuatorsConfig);
            autopilot::reset_actuators_config(camera_uuid, forced, &|step, total, label| {
                camera_ui::set_setup_progress(camera_uuid, step, total, label)
            })
            .await
            .and_then(|outcome| {
                camera_ui::end_default_setup(
                    camera_uuid,
                    SetupResult {
                        changes: outcome.changes,
                        forced,
                        error: None,
                    },
                );
                Ok(serde_json::to_value(outcome.config)?)
            })
        }
        _ => autopilot::handle_control(actuators_control).await,
    };

    match result {
        Ok(value) => {
            camera_ui::finish_autopilot_action(camera_uuid, &action);
            camera_state::emit_autopilot_control_update(camera_uuid, &action, &value);
            if matches!(action, AutopilotAction::ForgetActuatorsConfig) {
                mcm_client::forget_hostname(camera_uuid);
                connectivity::forget_camera(camera_uuid);
            }
            Ok(value)
        }
        Err(error) => {
            let message = format!("{error:?}");
            camera_ui::fail_autopilot_action(camera_uuid, &action, &message);
            Err(ControlError::other(message))
        }
    }
}
