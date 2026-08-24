use anyhow::{Context, Result};
use br4kcam_api::LuaScriptStatus;
use indexmap::IndexMap;
use mavlink::ardupilotmega::SERVO_OUTPUT_RAW_DATA;
use mlua::Lua;
use tera::Tera;
use tracing::*;
use uuid::Uuid;

use crate::{
    CameraActuators, api, generate_update_channel_param_function,
    manager::{Manager, get_output_raw_from_channel},
    parameters::{ActuatorsParameters, ChannelFunction, ParamType},
};

// Off the pre-rebrand base: ArduPilot ties each key to a prefix CRC, so a renamed
// prefix on the same key makes param:add_table fail until that key is wiped.
const PARAM_TABLE_KEY_BASE: u8 = 90;
pub const PARAM_PREFIX: &str = "BR4KCAM";

/// Stable ownership marker stamped into every generated script (new installs).
const SCRIPT_OWNERSHIP_MARKER: &str = "BLUEROBOTICS_4K_CAM_MANAGER_SCRIPT";

/// Content snippets that identify scripts this product wrote (current or historical).
/// Filenames are unreliable across brands; ownership is detected from content only.
/// The shared header line matches every prior export without naming older brands.
const SCRIPT_CONTENT_OWNERSHIP_MARKERS: &[&str] = &[
    SCRIPT_OWNERSHIP_MARKER,
    "Focus correction script. This script was generated and exported by",
];

const SCRIPT_HEALTH_STALE_THRESHOLD: u8 = 3;

impl Manager {
    #[instrument(level = "debug")]
    pub async fn export_script(camera_uuid: &Uuid, overwrite: bool) -> Result<bool> {
        let (contents, path) = {
            let manager = crate::manager::MANAGER
                .get()
                .context("Not available")?
                .read()
                .await;
            let camera_actuators = manager
                .settings
                .actuators
                .get(camera_uuid)
                .context(crate::ACTUATORS_NOT_CONFIGURED)?;

            (
                generate_lua_script(camera_actuators)?,
                manager.autopilot_scripts_file.clone(),
            )
        };
        validate_lua(&contents)?;

        let path_obj = std::path::Path::new(&path);
        if let Some(parent_dir) = path_obj.parent() {
            tokio::fs::create_dir_all(parent_dir).await?;
        }

        if let Ok(existing_contents) = tokio::fs::read_to_string(path_obj).await
            && !overwrite
            && existing_contents == contents
        {
            return Ok(false);
        }

        trace!("Saving Lua script to {path:?}. Lua script content: {contents:#?}");

        tokio::fs::write(path_obj, contents)
            .await
            .map_err(|error| {
                error!(?error, ?path, "Failed writing autopilot lua script");
                anyhow::Error::msg(error)
            })?;

        remove_conflicting_owned_scripts(path_obj).await;
        info!("Wrote new lua script to {path:?}");
        crate::health::refresh_lua_script_status().await;
        // Settings save is deferred to update_config finalize / ExportLuaScript caller.
        Ok(true)
    }

    /// Whether the script installed on the autopilot is the one this install expects.
    ///
    /// Compares file contents rather than asking the autopilot, so it also catches a
    /// script left behind by an older manager version: the template stamps the version.
    ///
    /// ponytail: one script file backs every configured camera, so with more than one
    /// configured this passes as soon as any of them matches. Upgrade path is one file
    /// per camera, which the autopilot scripts folder already supports.
    #[instrument(level = "debug")]
    pub async fn script_status() -> LuaScriptStatus {
        let Some(manager) = crate::manager::MANAGER.get() else {
            return LuaScriptStatus::Unknown;
        };

        let (path, expected) = {
            let manager = manager.read().await;
            let expected: Vec<String> = manager
                .settings
                .actuators
                .values()
                .filter_map(|actuators| generate_lua_script(actuators).ok())
                .collect();
            (manager.autopilot_scripts_file.clone(), expected)
        };

        if expected.is_empty() {
            return LuaScriptStatus::Unknown;
        }

        remove_conflicting_owned_scripts(std::path::Path::new(&path)).await;

        match tokio::fs::read_to_string(&path).await {
            Ok(installed) if expected.contains(&installed) => LuaScriptStatus::Ok,
            Ok(_) => LuaScriptStatus::Outdated,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => LuaScriptStatus::Missing,
            Err(error) => {
                warn!(?error, ?path, "Failed reading autopilot lua script");
                LuaScriptStatus::Unknown
            }
        }
    }

    #[instrument(level = "debug")]
    pub async fn delete_script_file(path: &str) -> Result<()> {
        let path_obj = std::path::Path::new(path);

        match tokio::fs::remove_file(path_obj).await {
            Ok(()) => info!("Removed lua script at {path:?}"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                trace!("No lua script to remove at {path:?}");
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed removing lua script at {path:?}"));
            }
        }

        crate::health::refresh_lua_script_status().await;

        Ok(())
    }

    /// Disable scripting (or reload if no reboot needed). Returns `true` if reboot is required.
    /// Does not reboot and does not hold [`crate::manager::MANAGER`].
    #[instrument(level = "debug")]
    pub async fn disable_or_reload_lua() -> Result<bool> {
        let autopilot_reboot_required =
            crate::mavlink::component()?.enable_lua_script(true).await?;

        if !autopilot_reboot_required {
            crate::mavlink::component()?
                .reload_lua_scripts(true)
                .await?;
        }

        Ok(autopilot_reboot_required)
    }

    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_script_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        overwrite: bool,
    ) -> Result<bool> {
        let mut autopilot_reboot_required = overwrite;

        if let Some(channel) = &parameters.script_channel {
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
                    .script_channel
            };

            let mavlink = crate::mavlink::component()?;
            let encoding = mavlink.encoding().await;

            // Disables the old script_channel:
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
                                "script_channel (SERVO{}) changed from {old_value:?} to {new_value:?}",
                                old_channel as u8,
                            );
                            autopilot_reboot_required = true;
                        }
                        Err(error) => {
                            return Err(error).context(
                                "Failed to disable the old script channel when setting parameter",
                            );
                        }
                    }
                }
            }

            // Sets the new script_channel:
            {
                let param_name = format!("SERVO{}_FUNCTION", *channel as u8);

                // The script servo input is the values from the CameraFocus
                let function = Self::script_channel_function();

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
                                "script_channel (SERVO{}) changed from {old_value:?} to {new_value:?}",
                                *channel as u8
                            );

                            let mut manager = crate::manager::MANAGER
                                .get()
                                .context("Not available")?
                                .write()
                                .await;
                            if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid)
                            {
                                actuators.parameters.script_channel = *channel;
                            }
                            autopilot_reboot_required = true;
                        }
                        Err(error) => {
                            return Err(error).context(
                                "Failed setting new script channel parameter when setting parameter",
                            );
                        }
                    }
                }
            }
        }

        Self::update_script_channel_parameters(camera_uuid, parameters, autopilot_reboot_required)
            .await?;

        Ok(autopilot_reboot_required)
    }

    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_script_enable(
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

            let channel = current_parameters.camera_id as u8;

            let param_name = format!("{PARAM_PREFIX}{channel}_ENABLE");

            let new_value = match (parameters.enable_focus_and_zoom_correlation, force_apply) {
                (Some(value), _) => value,
                (None, true) => current_parameters.enable_focus_and_zoom_correlation,
                (None, false) => return Ok(()),
            };

            let old_value = current_parameters.enable_focus_and_zoom_correlation;
            (param_name, new_value, old_value)
        };

        if !force_apply && old_value == new_value {
            trace!("Parameter {param_name:?} skipped");
            return Ok(());
        }

        let mavlink = crate::mavlink::component()?;
        let encoding = mavlink.encoding().await;
        let mut param = mavlink.get_param(&param_name, false).await?;
        param
            .value
            .set_value(ParamType::UINT8(new_value as u8), encoding)?;

        match mavlink.set_param(param).await {
            Ok(_) => {
                if old_value != new_value {
                    info!(
                        "{} changed from {old_value:?} to {new_value:?}",
                        stringify!(enable_focus_and_zoom_correlation),
                    );
                }
                let mut manager = crate::manager::MANAGER
                    .get()
                    .context("Not available")?
                    .write()
                    .await;
                if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid) {
                    actuators.parameters.enable_focus_and_zoom_correlation = new_value;
                }
            }
            Err(error) => {
                return Err(error).context(format!("Failed setting parameter {param_name}"));
            }
        }

        Ok(())
    }

    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_script_gain(
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

            let channel = current_parameters.camera_id as u8;

            let param_name = format!("{PARAM_PREFIX}{channel}_GAIN");

            let new_value = match (parameters.focus_margin_gain, force_apply) {
                (Some(value), _) => value,
                (None, true) => current_parameters.focus_margin_gain,
                (None, false) => return Ok(()),
            };

            let old_value = current_parameters.focus_margin_gain;
            (param_name, new_value, old_value)
        };

        if !force_apply && old_value == new_value {
            trace!("Parameter {param_name:?} skipped");
            return Ok(());
        }

        let mavlink = crate::mavlink::component()?;
        let encoding = mavlink.encoding().await;
        let mut param = mavlink.get_param(&param_name, false).await?;
        param
            .value
            .set_value(ParamType::REAL32(new_value), encoding)?;

        match mavlink.set_param(param).await {
            Ok(_) => {
                if old_value != new_value {
                    info!(
                        "{} changed from {old_value:?} to {new_value:?}",
                        stringify!(focus_margin_gain),
                    );
                }
                let mut manager = crate::manager::MANAGER
                    .get()
                    .context("Not available")?
                    .write()
                    .await;
                if let Some(actuators) = manager.settings.actuators.get_mut(camera_uuid) {
                    actuators.parameters.focus_margin_gain = new_value;
                }
            }
            Err(error) => {
                return Err(error).context(format!("Failed setting parameter {param_name}"));
            }
        }

        Ok(())
    }

    #[instrument(level = "debug", skip(parameters))]
    pub async fn update_script_channel_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
        force_apply: bool,
    ) -> Result<()> {
        Self::update_script_channel_min(camera_uuid, parameters, force_apply).await?;
        Self::update_script_channel_trim(camera_uuid, parameters, force_apply).await?;
        Self::update_script_channel_max(camera_uuid, parameters, force_apply).await?;

        Ok(())
    }

    fn script_channel_function() -> ChannelFunction {
        ChannelFunction::CameraFocus
    }

    fn expect_owned_script_servo_function(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let channel = parameters.script_channel as u8;
        map.insert(
            format!("SERVO{channel}_FUNCTION"),
            ParamType::INT16(Self::script_channel_function() as i16),
        );
    }

    fn expect_owned_script_enable(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let channel = parameters.camera_id as u8;
        map.insert(
            format!("{PARAM_PREFIX}{channel}_ENABLE"),
            ParamType::UINT8(parameters.enable_focus_and_zoom_correlation as u8),
        );
    }

    fn expect_owned_script_gain(
        parameters: &ActuatorsParameters,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let channel = parameters.camera_id as u8;
        map.insert(
            format!("{PARAM_PREFIX}{channel}_GAIN"),
            ParamType::REAL32(parameters.focus_margin_gain),
        );
    }

    generate_update_channel_param_function!(
        update_script_channel_min,
        script_channel_min,
        "SERVO",
        "MIN",
        UINT16,
        script_channel
    );

    generate_update_channel_param_function!(
        update_script_channel_max,
        script_channel_max,
        "SERVO",
        "MAX",
        UINT16,
        script_channel
    );

    generate_update_channel_param_function!(
        update_script_channel_trim,
        script_channel_trim,
        "SERVO",
        "TRIM",
        UINT16,
        script_channel
    );

    /// Evaluate focus-script health from an already-fetched SERVO sample.
    ///
    /// Returns `true` when a Lua reload should be attempted. Callers must invoke
    /// `reload_lua_scripts` **without** holding `MANAGER.write()`.
    pub fn apply_focus_script_health_sample(
        &mut self,
        camera_uuid: &Uuid,
        servo_output_raw: &SERVO_OUTPUT_RAW_DATA,
    ) -> bool {
        let (script_channel, focus_channel, enabled) = {
            let Some(actuators) = self.settings.actuators.get(camera_uuid) else {
                return false;
            };
            (
                actuators.parameters.script_channel,
                actuators.parameters.focus_channel,
                actuators.parameters.enable_focus_and_zoom_correlation,
            )
        };

        if !enabled {
            return false;
        }

        // script_channel (e.g. SERVO12) = CameraFocus = input to the Lua script
        let script_input_raw = get_output_raw_from_channel(servo_output_raw, script_channel);
        // focus_channel (e.g. SERVO10) = Script1 = output from the Lua script
        let script_output_raw = get_output_raw_from_channel(servo_output_raw, focus_channel);

        if let (Some(input_raw), Some(output_raw)) = (script_input_raw, script_output_raw) {
            return self.script_health.update(input_raw, output_raw);
        }
        false
    }
}

pub(super) fn push_owned_expectations(
    parameters: &ActuatorsParameters,
    map: &mut IndexMap<String, ParamType>,
) {
    Manager::expect_owned_script_servo_function(parameters, map);
    Manager::expect_owned_script_enable(parameters, map);
    Manager::expect_owned_script_gain(parameters, map);
}

#[derive(Debug, Default)]
pub(crate) struct ScriptHealthTracker {
    last_input_raw: Option<u16>,
    last_output_raw: Option<u16>,
    stale_count: u8,
}

impl ScriptHealthTracker {
    /// Checks whether the Lua script appears stuck by comparing its input
    /// (CameraFocus on script_channel) against its output (Script1 on focus_channel).
    /// Returns `true` when the output has been frozen for several consecutive
    /// readings while the input kept changing, indicating a reload is needed.
    fn update(&mut self, input_raw: u16, output_raw: u16) -> bool {
        let prev_input = self.last_input_raw.replace(input_raw);
        let prev_output = self.last_output_raw.replace(output_raw);

        let Some((prev_input, prev_output)) = prev_input.zip(prev_output) else {
            return false;
        };

        let input_changed = input_raw.abs_diff(prev_input) > 10;
        let output_stuck = output_raw == prev_output;

        if input_changed && !output_stuck {
            // The script answered an input change, so whatever the autopilot reported
            // earlier is no longer stopping it. Without this the failure latches forever.
            crate::health::clear_lua_script_failure();
        }

        if !input_changed || !output_stuck {
            self.stale_count = 0;
            return false;
        }

        self.stale_count = self.stale_count.saturating_add(1);
        if self.stale_count < SCRIPT_HEALTH_STALE_THRESHOLD {
            return false;
        }

        warn!(
            "Lua script appears stuck: input changed ({prev_input} -> {input_raw}) \
             but output is frozen at {output_raw} for {count} consecutive readings",
            count = self.stale_count,
        );
        self.stale_count = 0;
        true
    }
}

fn generate_lua_script(config: &CameraActuators) -> Result<String> {
    let mut context = tera::Context::new();

    let channel = config.parameters.camera_id as u8;

    let param_table_key = PARAM_TABLE_KEY_BASE + channel;
    let param_prefix = format!("\"{PARAM_PREFIX}{channel}_\"");

    context.insert("param_table_key", &param_table_key);
    context.insert("param_prefix", &param_prefix);
    context.insert("margin_gain", &{ config.parameters.focus_margin_gain });
    context.insert("k_script", &(config.parameters.script_function as u8));
    let script_n = config.parameters.script_function as u8 - api::ScriptFunction::SCRIPT1 as u8 + 1;
    context.insert("script_function_name", &format!("Script{script_n}"));
    context.insert("closest_points", &config.closest_points.to_lua());
    context.insert("furthest_points", &config.furthest_points.to_lua());
    context.insert("version", env!("CARGO_PKG_VERSION"));

    let template = include_str!("br4kcam.lua.template");

    let file = Tera::one_off(template, &context, false)?;

    Ok(file)
}

fn script_has_ownership_marker(contents: &str) -> bool {
    SCRIPT_CONTENT_OWNERSHIP_MARKERS
        .iter()
        .any(|marker| contents.contains(marker))
}

/// Remove other `.lua` files in the scripts directory that look like ours by content.
///
/// After a rebrand the configured path may change; an older file left beside it would
/// still run on the autopilot. Filenames are unreliable across brands, so ownership is
/// detected from stamped content markers only.
async fn remove_conflicting_owned_scripts(keep: &std::path::Path) {
    let Some(dir) = keep.parent() else {
        return;
    };
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path == keep {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("lua") {
            continue;
        }
        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        if !script_has_ownership_marker(&contents) {
            continue;
        }
        match tokio::fs::remove_file(&path).await {
            Ok(()) => info!(
                ?path,
                "Removed conflicting 4K Cam Manager lua script (matched by content marker)"
            ),
            Err(error) => warn!(?error, ?path, "Failed removing conflicting lua script"),
        }
    }
}

fn validate_lua(script: &str) -> Result<()> {
    Lua::new()
        .load(script)
        .set_mode(mlua::ChunkMode::Text)
        .into_function()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_generation() {
        let contents = generate_lua_script(&CameraActuators::default()).unwrap();
        dbg!(&contents);

        validate_lua(&contents).unwrap();

        assert!(contents.contains("warn_missing_servo_function"));
        assert!(contents.contains(SCRIPT_OWNERSHIP_MARKER));
        assert!(contents.contains("find_servo_function(K_FOCUS, \"CameraFocus\""));
        assert!(contents.contains("find_servo_function(K_ZOOM, \"CameraZoom\""));
        assert!(contents.contains("find_servo_function(K_SCRIPT, \"Script1\""));
        assert!(contents.contains("SRV_Channels:set_output_pwm(K_SCRIPT, script_trim)"));
        assert!(contents.contains("servo function not found"));
    }

    #[test]
    fn generated_script_uses_configured_script_function() {
        let mut actuators = CameraActuators::default();
        actuators.parameters.script_function = crate::api::ScriptFunction::SCRIPT3;
        let contents = generate_lua_script(&actuators).unwrap();
        validate_lua(&contents).unwrap();
        assert!(contents.contains("local K_SCRIPT = 96"));
        assert!(contents.contains("find_servo_function(K_SCRIPT, \"Script3\""));
        assert!(!contents.contains("\"Script1\""));
    }

    #[test]
    fn ownership_markers_recognize_legacy_and_current_scripts() {
        assert!(script_has_ownership_marker(
            "-- BLUEROBOTICS_4K_CAM_MANAGER_SCRIPT\nlocal x = 1"
        ));
        assert!(script_has_ownership_marker(
            "--- Focus correction script. This script was generated and exported by an older manager."
        ));
        assert!(!script_has_ownership_marker(
            "-- some unrelated vehicle script"
        ));
    }
}
