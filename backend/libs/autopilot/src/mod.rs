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
    set_backend_version, set_rebooting, set_syncing, subscribe_health, vehicle_armed,
};
pub use manager::{clear_saved_settings, init};

use crate::{
    manager::MANAGER,
    parameters::{ActuatorsParameters, CLOSEST_POINTS, FURTHEST_POINTS, param_display_value},
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

impl CameraActuators {
    /// The default setup of a camera with `lens`.
    fn default_for(lens: api::LensType) -> Self {
        let mut actuators = Self::default();
        actuators.parameters.lens = lens;
        actuators
    }
}

/// True when `message` (e.g. `format!("{error:?}")`) carries [`ACTUATORS_NOT_CONFIGURED`].
pub fn error_indicates_actuators_not_configured(message: &str) -> bool {
    message.contains(ACTUATORS_NOT_CONFIGURED)
}

/// Whether any camera with persisted actuator settings has a motorized lens, i.e. needs
/// the Lua script and focus/zoom controls.
pub async fn any_motorized_camera() -> bool {
    match MANAGER.get() {
        Some(manager) => manager
            .read()
            .await
            .settings
            .actuators
            .values()
            .any(|actuators| actuators.parameters.has_motorized_lens()),
        None => false,
    }
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
    /// Settings that differed from the defaults, and parameters released for a fixed-focus
    /// lens, as `name: old → new`.
    pub changes: Vec<String>,
}

/// Whether a camera with `lens` uses the setting `name`. A fixed-focus lens only has tilt.
fn setting_used_by(lens: api::LensType, name: &str) -> bool {
    match lens {
        api::LensType::Motorized => true,
        api::LensType::FixedFocus => {
            name == "lens" || name == "camera_id" || name.starts_with("tilt_")
        }
    }
}

/// Parameters of `current` that differ from `defaults`, as `name: old → new`. Settings the
/// lens of `defaults` does not use are left out.
fn changed_parameters(
    current: &api::ActuatorsParametersConfig,
    defaults: &api::ActuatorsParametersConfig,
) -> Result<Vec<String>> {
    let lens = defaults.lens.unwrap_or_default();
    let current = serde_json::to_value(current)?;
    let defaults = serde_json::to_value(defaults)?;
    let (Some(current), Some(defaults)) = (current.as_object(), defaults.as_object()) else {
        return Ok(Vec::new());
    };

    Ok(defaults
        .iter()
        .filter(|(name, _)| setting_used_by(lens, name))
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

/// The saved actuators configuration of `camera_uuid`, if it has one.
async fn saved_actuators_config(camera_uuid: Uuid) -> Result<Option<api::ActuatorsConfig>> {
    Ok(MANAGER
        .get()
        .context("Not available")?
        .read()
        .await
        .settings
        .actuators
        .get(&camera_uuid)
        .map(api::ActuatorsConfig::from))
}

/// Settings that applying `next` over `current` would change. A camera with no saved setup
/// reports a single "Initial hardware setup" entry.
fn setup_changes(
    current: &Option<api::ActuatorsConfig>,
    next: &api::ActuatorsConfig,
) -> Result<Vec<String>> {
    match (current, &next.parameters) {
        (Some(current), Some(next)) => {
            changed_parameters(current.parameters.as_ref().unwrap_or(next), next)
        }
        _ => Ok(vec!["Initial hardware setup".to_string()]),
    }
}

/// What applying `next` over the saved setup `current` of `camera_uuid` would change. The
/// releases come from the same plan the apply carries out.
async fn setup_preview(
    camera_uuid: Uuid,
    current: &Option<api::ActuatorsConfig>,
    next: &api::ActuatorsConfig,
    overwrite: bool,
    start: manager::plan::Start,
) -> Result<api::SetupPreview> {
    let changes = setup_changes(current, next)?;
    let snapshot = manager::plan::Snapshot::capture().await?;
    let plan = manager::plan::plan_apply(&snapshot, camera_uuid, next, overwrite, start)?;
    let released = manager::plan::describe_releases(&plan, &snapshot);

    Ok(api::SetupPreview { changes, released })
}

/// How a default setup for `lens` applies: over the defaults, writing everything, when forced
/// or for a camera with no saved setup, else over the saved setup. Returns the start and
/// whether to overwrite.
fn default_setup_start(
    force: bool,
    current: &Option<api::ActuatorsConfig>,
    lens: api::LensType,
) -> (manager::plan::Start, bool) {
    if force || current.is_none() {
        (manager::plan::Start::Defaults(lens), true)
    } else {
        (manager::plan::Start::Saved, false)
    }
}

/// The lens a default setup is for: the `requested` one, else the saved one.
fn default_setup_lens(
    requested: Option<api::LensType>,
    current: &Option<api::ActuatorsConfig>,
) -> api::LensType {
    requested
        .or_else(|| {
            current
                .as_ref()
                .and_then(|config| config.parameters.as_ref())
                .and_then(|parameters| parameters.lens)
        })
        .unwrap_or_default()
}

/// Apply the default hardware setup for `lens` (the saved lens when `None`), reporting
/// `progress(step, total, label)`.
///
/// Without `force`, a camera whose saved setup already matches the defaults is left
/// untouched, and otherwise only the differing parameters are written. With `force`
/// (or when the camera has no saved setup yet) every default is written again.
#[instrument(level = "debug", skip(progress))]
pub async fn reset_actuators_config(
    camera_uuid: Uuid,
    force: bool,
    lens: Option<api::LensType>,
    progress: &(dyn Fn(u32, u32, &str) + Send + Sync),
) -> Result<ResetOutcome> {
    const STEPS: u32 = 4;

    progress(1, STEPS, "Comparing the current setup with the defaults");
    let current = saved_actuators_config(camera_uuid).await?;
    let lens = default_setup_lens(lens, &current);
    let default_config = api::ActuatorsConfig::from(&CameraActuators::default_for(lens));
    let (start, overwrite) = default_setup_start(force, &current, lens);
    let preview = setup_preview(camera_uuid, &current, &default_config, overwrite, start).await?;
    let changes: Vec<String> = preview
        .changes
        .into_iter()
        .chain(
            preview
                .released
                .into_iter()
                .map(|line| format!("{line} (released)")),
        )
        .collect();
    if !force
        && changes.is_empty()
        && let Some(config) = current
    {
        return Ok(ResetOutcome { config, changes });
    }

    let reapply_everything = force || current.is_none();
    manager::reboot_outside_apply_with(
        Box::pin(async {
            let label = match lens {
                api::LensType::Motorized => {
                    "Applying camera, focus, zoom, tilt and script parameters"
                }
                api::LensType::FixedFocus => "Applying tilt parameters",
            };
            progress(2, STEPS, label);
            if reapply_everything {
                manager::Manager::reset_config(&camera_uuid, lens).await
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
            let label = match lens {
                api::LensType::Motorized => "Enabling the script and saving the setup",
                api::LensType::FixedFocus => "Saving the setup",
            };
            progress(4, STEPS, label);
            manager::Manager::finalize_config_after_reboot(&camera_uuid).await
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
            let parameters = {
                let manager = MANAGER.get().context("Not available")?.read().await;
                manager
                    .settings
                    .actuators
                    .get(&camera_uuid)
                    .context(crate::ACTUATORS_NOT_CONFIGURED)?
                    .parameters
                    .clone()
            };
            manager::Manager::apply_state_setpoints(new_state, &parameters).await?;
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
                        .is_some_and(|a| {
                            a.parameters.has_motorized_lens()
                                && a.parameters.enable_focus_and_zoom_correlation
                        })
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
            let current = saved_actuators_config(actuators_control.camera_uuid).await?;
            let lens = default_setup_lens(None, &current);
            let config = api::ActuatorsConfig::from(&CameraActuators::default_for(lens));

            serde_json::to_value(config)?
        }
        Action::SetActuatorsConfig(new_config) => {
            let camera_uuid = actuators_control.camera_uuid;
            // Overwrite when the camera has no saved setup yet.
            let overwrite = saved_actuators_config(camera_uuid).await?.is_none();
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
                    manager::Manager::update_config(&camera_uuid, &new_config, overwrite).await
                }),
                Box::pin(async {
                    manager::Manager::finalize_config_after_reboot(&camera_uuid).await
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
        Action::PreviewResetActuatorsConfig(lens) => {
            let camera_uuid = actuators_control.camera_uuid;
            let current = saved_actuators_config(camera_uuid).await?;
            let lens = default_setup_lens(*lens, &current);
            let default_config = api::ActuatorsConfig::from(&CameraActuators::default_for(lens));
            let (start, overwrite) = default_setup_start(false, &current, lens);
            let preview =
                setup_preview(camera_uuid, &current, &default_config, overwrite, start).await?;

            serde_json::to_value(preview)?
        }
        Action::PreviewSetActuatorsConfig(new_config) => {
            let camera_uuid = actuators_control.camera_uuid;
            let current = saved_actuators_config(camera_uuid).await?;
            let base_config = current
                .clone()
                .unwrap_or(api::ActuatorsConfig::from(&CameraActuators::default()));
            let new_config = merge_struct::merge(&base_config, new_config)
                .context("Failing to merge structs")?;
            let overwrite = current.is_none();
            let preview = setup_preview(
                camera_uuid,
                &current,
                &new_config,
                overwrite,
                manager::plan::Start::Saved,
            )
            .await?;

            serde_json::to_value(preview)?
        }
        Action::ResetActuatorsConfig(lens) | Action::ForceResetActuatorsConfig(lens) => {
            let force = matches!(
                actuators_control.action,
                Action::ForceResetActuatorsConfig(_)
            );
            let outcome = Box::pin(reset_actuators_config(
                actuators_control.camera_uuid,
                force,
                *lens,
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
                let current = saved_actuators_config(camera_uuid).await?;
                let lens = default_setup_lens(None, &current);
                manager::reboot_outside_apply(
                    Box::pin(async { manager::Manager::reset_config(&camera_uuid, lens).await }),
                    Box::pin(async {
                        manager::Manager::finalize_config_after_reboot(&camera_uuid).await
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
        Action::AcceptParameterDrifts(names) => {
            let camera_uuid = actuators_control.camera_uuid;
            // Read the autopilot values before MANAGER: lock order is CONFIG_APPLY, MANAGER.
            let mut autopilot_values = Vec::with_capacity(names.len());
            {
                let component = crate::mavlink::component()?;
                let cache = component.inner.parameters.read().await;
                for name in names {
                    let value = cache
                        .get(name)
                        .and_then(|parameter| param_display_value(&parameter.value))
                        .with_context(|| format!("{name} has no value from the autopilot"))?;
                    autopilot_values.push((name.as_str(), value));
                }
            }

            let _apply = manager::CONFIG_APPLY.lock().await;
            {
                let mut manager = MANAGER.get().context("Not available")?.write().await;
                let actuators = manager
                    .settings
                    .actuators
                    .get_mut(&camera_uuid)
                    .context(ACTUATORS_NOT_CONFIGURED)?;
                // Adopt all values or none, so one rejected name leaves the saved setup as it was.
                let mut adopted = actuators.parameters.clone();
                for (name, value) in autopilot_values {
                    manager::owned_parameters::adopt_autopilot_value(&mut adopted, name, value)?;
                }
                actuators.parameters = adopted;
            }
            manager::Manager::save_actuators_settings().await?;
            drop(_apply);
            manager::owned_parameters::rebuild().await;
            manager::owned_parameters::reevaluate_after_apply().await;
            serde_json::Value::Null
        }
        Action::RestoreParameterDrifts(names) => {
            let camera_uuid = actuators_control.camera_uuid;
            // Mount and camera types and channel functions only take effect after a reboot,
            // as in the apply path.
            let needs_reboot = names
                .iter()
                .any(|name| name.ends_with("_TYPE") || name.ends_with("_FUNCTION"));
            manager::reboot_outside_apply(
                Box::pin(async {
                    let mavlink = crate::mavlink::component()?;
                    let encoding = mavlink.encoding().await;
                    for name in names {
                        let expected = manager::owned_parameters::expectations_for_param(name)
                            .into_iter()
                            .find(|(owner, _)| *owner == camera_uuid)
                            .with_context(|| format!("{name} is not part of the saved setup"))?
                            .1;
                        let mut parameter = mavlink.get_param(name, false).await?;
                        parameter.value.set_value(expected, encoding)?;
                        mavlink
                            .set_param(parameter)
                            .await
                            .with_context(|| format!("Failed restoring {name}"))?;
                    }
                    manager::owned_parameters::reevaluate_after_apply().await;
                    Ok(needs_reboot)
                }),
                Box::pin(async {
                    manager::owned_parameters::reevaluate_after_apply().await;
                    Ok(())
                }),
            )
            .await?;
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
        error_indicates_actuators_not_configured, setup_changes,
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
    fn setup_changes_only_reports_a_real_difference() {
        let defaults = api::ActuatorsConfig::from(&CameraActuators::default());
        assert_eq!(
            setup_changes(&None, &defaults).unwrap(),
            ["Initial hardware setup"]
        );
        assert!(
            setup_changes(&Some(defaults.clone()), &defaults)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn fixed_focus_changes_ignore_settings_it_does_not_use() {
        let motorized = api::ActuatorsConfig::from(&CameraActuators::default())
            .parameters
            .unwrap();
        let fixed =
            api::ActuatorsConfig::from(&CameraActuators::default_for(api::LensType::FixedFocus))
                .parameters
                .unwrap();

        let mut current = motorized.clone();
        current.focus_channel = Some(api::ServoChannel::SERVO1);
        current.tilt_channel = Some(api::ServoChannel::SERVO2);
        let changes = changed_parameters(&current, &fixed).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0], "lens: Motorized → FixedFocus");
        assert!(changes[1].starts_with("tilt_channel: SERVO2 → "));

        let mut current = fixed.clone();
        current.focus_channel = Some(api::ServoChannel::SERVO1);
        assert!(changed_parameters(&current, &fixed).unwrap().is_empty());
    }

    #[test]
    fn default_setup_actions_accept_a_missing_lens() {
        let parse = |json: &str| serde_json::from_str::<api::Action>(json).unwrap();
        assert!(matches!(
            parse(r#"{"action":"resetActuatorsConfig"}"#),
            api::Action::ResetActuatorsConfig(None)
        ));
        assert!(matches!(
            parse(r#"{"action":"forceResetActuatorsConfig","json":null}"#),
            api::Action::ForceResetActuatorsConfig(None)
        ));
        assert!(matches!(
            parse(r#"{"action":"previewResetActuatorsConfig","json":"FixedFocus"}"#),
            api::Action::PreviewResetActuatorsConfig(Some(api::LensType::FixedFocus))
        ));
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
