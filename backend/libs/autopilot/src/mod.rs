mod actuators_watch;
pub mod api;
mod health;
mod manager;
mod mavlink;
pub mod parameters;
mod settings_translations;

use anyhow::{Context, Result};
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::*;
use uuid::Uuid;

pub use actuators_watch::{
    add_interest as add_actuators_state_interest, cache_is_fresh as actuators_cache_is_fresh,
    cached_actuators_state, interest_count as actuators_interest_count,
    remove_interest as remove_actuators_state_interest, shutdown as shutdown_actuators_stream,
    subscribe as subscribe_actuators_state,
};
pub use health::{
    ParameterDrift, diagnostics, health, lua_script_status, lua_scripting_disabled,
    needs_mavlink_endpoint_ensure, parameter_drifts, report_endpoint_setup, rpc_failed, rpc_ok,
    set_backend_version, set_rebooting, set_syncing, subscribe_health,
};
pub use manager::{clear_saved_settings, init};

use crate::{
    manager::MANAGER,
    parameters::{ActuatorsParameters, CLOSEST_POINTS, FURTHEST_POINTS},
};

/// Context message when a camera has no actuators entry yet.
pub const ACTUATORS_NOT_CONFIGURED: &str = "Camera's actuators not configured";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct CameraActuators {
    pub parameters: ActuatorsParameters,
    pub closest_points: api::FocusZoomPoints,
    pub furthest_points: api::FocusZoomPoints,
    pub state: api::ActuatorsState,
}

impl Default for CameraActuators {
    fn default() -> Self {
        Self {
            parameters: ActuatorsParameters::default(),
            closest_points: api::FocusZoomPoints(CLOSEST_POINTS.to_vec()),
            furthest_points: api::FocusZoomPoints(FURTHEST_POINTS.to_vec()),
            state: api::ActuatorsState::default(),
        }
    }
}

/// True when `message` (e.g. `format!("{error:?}")`) carries [`ACTUATORS_NOT_CONFIGURED`].
pub fn error_indicates_actuators_not_configured(message: &str) -> bool {
    message.contains(ACTUATORS_NOT_CONFIGURED)
}

/// UUIDs of every camera with persisted actuator settings, i.e. cameras this
/// install expects to find. Empty when the manager is not up yet.
pub async fn configured_cameras() -> Vec<Uuid> {
    match MANAGER.get() {
        Some(manager) => manager
            .read()
            .await
            .settings
            .actuators
            .keys()
            .copied()
            .collect(),
        None => Vec::new(),
    }
}

/// What [`reset_actuators_config`] did.
pub struct ResetOutcome {
    /// Actuators configuration after the reset.
    pub config: api::ActuatorsConfig,
    /// Settings that differed from the defaults, as `name: old → new`.
    pub changes: Vec<String>,
}

/// Parameters of `current` that differ from `defaults`, as `name: old → new`.
fn changed_parameters(
    current: &api::ActuatorsParametersConfig,
    defaults: &api::ActuatorsParametersConfig,
) -> Result<Vec<String>> {
    let current = serde_json::to_value(current)?;
    let defaults = serde_json::to_value(defaults)?;
    let (Some(current), Some(defaults)) = (current.as_object(), defaults.as_object()) else {
        return Ok(Vec::new());
    };

    Ok(defaults
        .iter()
        .filter(|(name, default)| current.get(*name) != Some(*default))
        .map(|(name, default)| {
            let old = current.get(name).unwrap_or(&serde_json::Value::Null);
            format!(
                "{name}: {} → {}",
                old.to_string().trim_matches('"'),
                default.to_string().trim_matches('"')
            )
        })
        .collect())
}

/// Apply the default hardware setup, reporting `progress(step, total, label)`.
///
/// Without `force`, a camera whose saved setup already matches the defaults is left
/// untouched, and otherwise only the differing parameters are written. With `force`
/// (or when the camera has no saved setup yet) every default is written again.
#[instrument(level = "debug", skip(progress))]
pub async fn reset_actuators_config(
    camera_uuid: Uuid,
    force: bool,
    progress: &(dyn Fn(u32, u32, &str) + Send + Sync),
) -> Result<ResetOutcome> {
    const STEPS: u32 = 4;
    let default_config = api::ActuatorsConfig::from(&CameraActuators::default());

    progress(1, STEPS, "Comparing the current setup with the defaults");
    let current = MANAGER
        .get()
        .context("Not available")?
        .read()
        .await
        .settings
        .actuators
        .get(&camera_uuid)
        .map(api::ActuatorsConfig::from);
    let changes = match (&current, &default_config.parameters) {
        (Some(current), Some(defaults)) => {
            changed_parameters(current.parameters.as_ref().unwrap_or(defaults), defaults)?
        }
        _ => vec!["Initial hardware setup".to_string()],
    };
    if !force
        && changes.is_empty()
        && let Some(config) = current
    {
        return Ok(ResetOutcome { config, changes });
    }

    let reapply_everything = force || current.is_none();
    manager::reboot_outside_apply_with(
        Box::pin(async {
            progress(
                2,
                STEPS,
                "Applying camera, focus, zoom, tilt and script parameters",
            );
            if reapply_everything {
                manager::Manager::reset_config(&camera_uuid).await
            } else {
                manager::Manager::update_config(&camera_uuid, &default_config, false).await
            }
        }),
        Box::pin(async {
            progress(
                3,
                STEPS,
                "Rebooting the autopilot, this is the longest step",
            );
            crate::mavlink::component()?.reboot_autopilot().await
        }),
        Box::pin(async {
            progress(4, STEPS, "Enabling the script and saving the setup");
            manager::Manager::finalize_config_after_reboot(
                &camera_uuid,
                default_config.parameters.as_ref(),
            )
            .await
        }),
    )
    .await?;

    let manager = MANAGER.get().context("Not available")?.read().await;
    let config = manager
        .settings
        .actuators
        .get(&camera_uuid)
        .context(crate::ACTUATORS_NOT_CONFIGURED)?
        .into();
    Ok(ResetOutcome { config, changes })
}

/// Shared entry point for REST and WebSocket autopilot control requests.
#[instrument(level = "debug")]
pub async fn handle_control(actuators_control: api::ActuatorsControl) -> Result<serde_json::Value> {
    control_inner(Json(actuators_control)).await
}

#[instrument(level = "debug")]
pub(crate) async fn control_inner(
    actuators_control: Json<api::ActuatorsControl>,
) -> Result<serde_json::Value> {
    use api::Action;

    debug!("Got control query: {actuators_control:#?}");

    let res = match &actuators_control.action {
        Action::ExportLuaScript => {
            let camera_uuid = actuators_control.camera_uuid;
            manager::reboot_outside_apply(
                Box::pin(async {
                    let reload_script = manager::Manager::export_script(&camera_uuid, true).await?;
                    manager::Manager::save_actuators_settings().await?;
                    if reload_script {
                        crate::mavlink::component()?
                            .reload_lua_scripts(true)
                            .await?;
                    }
                    crate::mavlink::component()?.enable_lua_script(false).await
                }),
                // Export already saved under apply; post-reboot finalize is a no-op.
                Box::pin(async { Ok(()) }),
            )
            .await?;

            serde_json::to_value({})?
        }
        Action::GetActuatorsState => {
            // Prefer the SERVO watcher's cache when interest is on *and* a recent
            // sample exists. Otherwise one-shot wait so subscribe/REST are not
            // served stale defaults from disk.
            if actuators_watch::interest_count() > 0
                && actuators_watch::cache_is_fresh(actuators_control.camera_uuid)
            {
                let manager = MANAGER.get().context("Not available")?.read().await;

                let actuators = manager
                    .settings
                    .actuators
                    .get(&actuators_control.camera_uuid)
                    .context(crate::ACTUATORS_NOT_CONFIGURED)?;

                serde_json::to_value(actuators.state)?
            } else {
                // Wait for SERVO under a read lock so the watcher can still write.
                {
                    let manager = MANAGER.get().context("Not available")?.read().await;
                    let _ = manager
                        .settings
                        .actuators
                        .get(&actuators_control.camera_uuid)
                        .context(crate::ACTUATORS_NOT_CONFIGURED)?;
                }
                let age_before = actuators_watch::last_servo_age(actuators_control.camera_uuid);
                let servo_output_raw = crate::mavlink::component()?
                    .request_servo_output_raw()
                    .await
                    .context("Failed waiting for SERVO_OUTPUT_RAW_DATA message")?;
                let mut manager = MANAGER.get().context("Not available")?.write().await;
                let actuators = manager
                    .settings
                    .actuators
                    .get_mut(&actuators_control.camera_uuid)
                    .context(crate::ACTUATORS_NOT_CONFIGURED)?;
                let state = manager::actuators_state_from_servo(actuators, &servo_output_raw);
                let age_after = actuators_watch::last_servo_age(actuators_control.camera_uuid);
                // Do not clobber a newer watcher sample that landed while we waited.
                if !actuators_watch::servo_mark_advanced(age_before, age_after) {
                    actuators.state = state;
                    actuators_watch::mark_servo_from_get_state(actuators_control.camera_uuid);
                    serde_json::to_value(state)?
                } else {
                    serde_json::to_value(actuators.state)?
                }
            }
        }
        Action::SetActuatorsState(new_state) => {
            let camera_uuid = actuators_control.camera_uuid;
            let focus_was_set = new_state.focus.is_some();
            // Validate entry, then send MAVLink with no Manager lock held.
            {
                let manager = MANAGER.get().context("Not available")?.read().await;
                let _ = manager
                    .settings
                    .actuators
                    .get(&camera_uuid)
                    .context(crate::ACTUATORS_NOT_CONFIGURED)?;
            }
            manager::Manager::apply_state_setpoints(new_state).await?;
            let age_before = actuators_watch::last_servo_age(camera_uuid);
            let servo_output_raw = crate::mavlink::component()?
                .request_servo_output_raw()
                .await
                .context("Failed waiting for SERVO_OUTPUT_RAW_DATA message")?;
            let state = {
                let mut manager = MANAGER.get().context("Not available")?.write().await;
                let actuators = manager
                    .settings
                    .actuators
                    .get_mut(&camera_uuid)
                    .context(crate::ACTUATORS_NOT_CONFIGURED)?;
                let measured = manager::actuators_state_from_servo(actuators, &servo_output_raw);
                let age_after = actuators_watch::last_servo_age(camera_uuid);
                if !actuators_watch::servo_mark_advanced(age_before, age_after) {
                    actuators.state = measured;
                    actuators_watch::mark_servo_from_get_state(camera_uuid);
                    measured
                } else {
                    actuators.state
                }
            };
            // Health check waits for SERVO again — never under MANAGER.write().
            if focus_was_set {
                let enabled = {
                    let manager = MANAGER.get().context("Not available")?.read().await;
                    manager
                        .settings
                        .actuators
                        .get(&camera_uuid)
                        .is_some_and(|a| a.parameters.enable_focus_and_zoom_correlation)
                };
                if enabled {
                    let health_servo = crate::mavlink::component()?
                        .request_servo_output_raw()
                        .await
                        .ok();
                    if let Some(health_servo) = health_servo {
                        let needs_reload = {
                            let mut manager = MANAGER.get().context("Not available")?.write().await;
                            manager.apply_focus_script_health_sample(&camera_uuid, &health_servo)
                        };
                        if needs_reload {
                            warn!("Attempting Lua script reload due to stale focus output");
                            crate::health::note_script_reload();
                            if let Err(error) =
                                crate::mavlink::component()?.reload_lua_scripts(true).await
                            {
                                error!("Failed to reload Lua scripts: {error:?}");
                            }
                        }
                    }
                }
            }
            serde_json::to_value(state)?
        }
        Action::GetActuatorsConfig => {
            let manager = MANAGER.get().context("Not available")?.read().await;

            let config: &api::ActuatorsConfig = &manager
                .settings
                .actuators
                .get(&actuators_control.camera_uuid)
                .context(crate::ACTUATORS_NOT_CONFIGURED)?
                .into();

            serde_json::to_value(config)?
        }
        Action::GetActuatorsDefaultConfig => {
            let config = api::ActuatorsConfig::from(&CameraActuators::default());

            serde_json::to_value(config)?
        }
        Action::SetActuatorsConfig(new_config) => {
            let camera_uuid = actuators_control.camera_uuid;
            let new_config = {
                let manager = MANAGER.get().context("Not available")?.read().await;
                let base_config = manager
                    .settings
                    .actuators
                    .get(&camera_uuid)
                    .map(api::ActuatorsConfig::from)
                    .unwrap_or(api::ActuatorsConfig::from(&CameraActuators::default()));
                merge_struct::merge(&base_config, new_config).context("Failing to merge structs")?
            };

            manager::reboot_outside_apply(
                Box::pin(async {
                    manager::Manager::update_config(&camera_uuid, &new_config, false).await
                }),
                Box::pin(async {
                    manager::Manager::finalize_config_after_reboot(
                        &camera_uuid,
                        new_config.parameters.as_ref(),
                    )
                    .await
                }),
            )
            .await?;

            let manager = MANAGER.get().context("Not available")?.read().await;
            let config: &api::ActuatorsConfig = &manager
                .settings
                .actuators
                .get(&camera_uuid)
                .context(crate::ACTUATORS_NOT_CONFIGURED)?
                .into();

            serde_json::to_value(config)?
        }
        Action::ResetActuatorsConfig | Action::ForceResetActuatorsConfig => {
            let force = matches!(actuators_control.action, Action::ForceResetActuatorsConfig);
            let outcome = Box::pin(reset_actuators_config(
                actuators_control.camera_uuid,
                force,
                &|_, _, _| {},
            ))
            .await?;

            serde_json::to_value(outcome.config)?
        }
        Action::ForgetActuatorsConfig => {
            let camera_uuid = actuators_control.camera_uuid;
            let had_entry = MANAGER
                .get()
                .context("Not available")?
                .read()
                .await
                .settings
                .actuators
                .contains_key(&camera_uuid);
            if had_entry {
                let default_params = api::ActuatorsConfig::from(&CameraActuators::default());
                manager::reboot_outside_apply(
                    Box::pin(async { manager::Manager::reset_config(&camera_uuid).await }),
                    Box::pin(async {
                        manager::Manager::finalize_config_after_reboot(
                            &camera_uuid,
                            default_params.parameters.as_ref(),
                        )
                        .await
                    }),
                )
                .await?;
            }

            let _apply = manager::CONFIG_APPLY.lock().await;
            let removed = {
                let mut manager = MANAGER.get().context("Not available")?.write().await;
                manager.settings.actuators.shift_remove(&camera_uuid)
            };
            if removed.is_some() {
                manager::Manager::save_actuators_settings().await?;
                info!(%camera_uuid, "Forgot actuators configuration for camera");

                drop(_apply);
                manager::owned_parameters::rebuild().await;
                manager::owned_parameters::reevaluate_after_apply().await;
            }
            serde_json::Value::Null
        }
    };

    Ok(res)
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::{
        ACTUATORS_NOT_CONFIGURED, CameraActuators, api, changed_parameters,
        error_indicates_actuators_not_configured,
    };

    #[test]
    fn changed_parameters_lists_only_differences() {
        let defaults = api::ActuatorsConfig::from(&CameraActuators::default())
            .parameters
            .unwrap();
        assert!(changed_parameters(&defaults, &defaults).unwrap().is_empty());

        let mut current = defaults.clone();
        current.focus_channel = Some(api::ServoChannel::SERVO1);
        let changes = changed_parameters(&current, &defaults).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].starts_with("focus_channel: SERVO1 → "));
    }

    #[test]
    fn actuators_not_configured_message_is_stable() {
        let error = anyhow!("missing entry").context(ACTUATORS_NOT_CONFIGURED);
        assert!(error_indicates_actuators_not_configured(&format!(
            "{error:?}"
        )));
        assert!(!error_indicates_actuators_not_configured(
            "Camera actuators unavailable"
        ));
    }
}
