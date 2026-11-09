use anyhow::{Context, Result};
use br4kcam_api::LuaScriptStatus;
use indexmap::IndexMap;
use mavlink::ardupilotmega::SERVO_OUTPUT_RAW_DATA;
use mlua::Lua;
use serde::Serialize;
use tera::Tera;
use tracing::*;
use uuid::Uuid;

use crate::{
    CameraActuators,
    manager::{
        Manager, get_output_raw_from_channel,
        plan::{Change, PlannedWrite, Planner},
    },
    parameters::{ActuatorsParameters, ChannelFunction, ParamType, Setup},
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

// Independent of `CARGO_PKG_VERSION`. Bump only when the template or generation logic
// changes. Health compares script bodies and ignores the `--- Version:` header line.
pub const LUA_SCRIPT_VERSION: &str = "2.0.0";

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
            if !camera_actuators.parameters.has_motorized_lens() {
                anyhow::bail!("A fixed-focus lens needs no Lua script");
            }

            let motorized: Vec<&CameraActuators> = manager
                .settings
                .actuators
                .values()
                .filter(|actuators| actuators.parameters.has_motorized_lens())
                .collect();
            (
                generate_checked_lua_script(&motorized)?,
                manager.autopilot_scripts_file.clone(),
            )
        };

        if let Ok(existing_contents) = tokio::fs::read_to_string(&path).await
            && !overwrite
            && lua_scripts_equivalent(&existing_contents, &contents)
        {
            return Ok(false);
        }

        Self::write_script_file(&path, &contents).await?;
        // Settings save is deferred to update_config finalize / ExportLuaScript caller.
        Ok(true)
    }

    #[instrument(level = "debug", skip(contents))]
    pub(crate) async fn write_script_file(path: &str, contents: &str) -> Result<()> {
        let path_obj = std::path::Path::new(path);
        if let Some(parent_dir) = path_obj.parent() {
            tokio::fs::create_dir_all(parent_dir).await?;
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
        Ok(())
    }

    /// Whether the script installed on the autopilot is the one this install expects.
    ///
    /// Compares generated script bodies (ignoring the stamped `--- Version:` line).
    ///
    /// One script serves every motorized camera on every firmware.
    ///
    /// Cameras with a fixed-focus lens need no script, so with only those configured
    /// this is [`LuaScriptStatus::NotNeeded`].
    #[instrument(level = "debug")]
    pub async fn script_status() -> LuaScriptStatus {
        let Some(manager) = crate::manager::MANAGER.get() else {
            return LuaScriptStatus::Unknown;
        };

        let (path, any_configured, expected) = {
            let manager = manager.read().await;
            let motorized: Vec<&CameraActuators> = manager
                .settings
                .actuators
                .values()
                .filter(|actuators| actuators.parameters.has_motorized_lens())
                .collect();
            let expected: Vec<String> = if motorized.is_empty() {
                Vec::new()
            } else {
                generate_lua_script(&motorized).ok().into_iter().collect()
            };
            (
                manager.autopilot_scripts_file.clone(),
                !manager.settings.actuators.is_empty(),
                expected,
            )
        };

        if expected.is_empty() {
            return if any_configured {
                LuaScriptStatus::NotNeeded
            } else {
                LuaScriptStatus::Unknown
            };
        }

        remove_conflicting_owned_scripts(std::path::Path::new(&path)).await;

        let installed_normalized = tokio::fs::read_to_string(&path).await;
        match installed_normalized {
            Ok(installed)
                if expected
                    .iter()
                    .any(|candidate| lua_scripts_equivalent(&installed, candidate)) =>
            {
                LuaScriptStatus::Ok
            }
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

    fn expect_owned_script_servo_function(
        parameters: &ActuatorsParameters,
        setup: Setup,
        map: &mut IndexMap<String, ParamType>,
    ) {
        let channel = parameters.script_channel as u8;
        map.insert(
            format!("SERVO{channel}_FUNCTION"),
            ParamType::INT16(parameters.focus_input_function(setup) as i16),
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
                actuators.parameters.has_motorized_lens()
                    && actuators.parameters.enable_focus_and_zoom_correlation,
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

/// The script channel, the focus input from the GCS: `SERVO{script_channel}_FUNCTION` and
/// its PWM range. Returns whether the function is written.
pub(super) fn plan(planner: &mut Planner, change: &Change) -> bool {
    let Change {
        current,
        next,
        incoming,
        overwrite,
        setup,
        ..
    } = *change;
    let mut written = overwrite;

    let mut channel_moved = false;
    if incoming.script_channel.is_some() || change.functions_moved() {
        written |= planner.channel_function(
            next.script_channel,
            next.focus_input_function(setup),
            overwrite,
        );
        channel_moved = current.script_channel != next.script_channel;
        if overwrite || channel_moved {
            planner.clear_reversed(next.script_channel);
        }
    }
    // A channel the role moves to has none of its settings yet.
    planner.channel_range(
        next.script_channel,
        [
            current.script_channel_min,
            current.script_channel_trim,
            current.script_channel_max,
        ],
        [
            next.script_channel_min,
            next.script_channel_trim,
            next.script_channel_max,
        ],
        written || channel_moved,
    );

    written
}

/// `BR4KCAM{n}_ENABLE` and `_GAIN` of `next`, each written when it changes from `current`,
/// or both with `force`. They only exist while the script runs.
pub(crate) fn param_writes(
    current: &ActuatorsParameters,
    next: &ActuatorsParameters,
    force: bool,
) -> Vec<PlannedWrite> {
    let camera = next.camera_id as u8;
    let mut writes = Vec::new();
    if force || current.enable_focus_and_zoom_correlation != next.enable_focus_and_zoom_correlation
    {
        writes.push(PlannedWrite {
            name: format!("{PARAM_PREFIX}{camera}_ENABLE"),
            value: ParamType::UINT8(next.enable_focus_and_zoom_correlation as u8),
            force: true,
        });
    }
    if force || current.focus_margin_gain != next.focus_margin_gain {
        writes.push(PlannedWrite {
            name: format!("{PARAM_PREFIX}{camera}_GAIN"),
            value: ParamType::REAL32(next.focus_margin_gain),
            force: true,
        });
    }
    writes
}

pub(super) fn push_owned_expectations(
    parameters: &ActuatorsParameters,
    setup: Setup,
    map: &mut IndexMap<String, ParamType>,
) {
    Manager::expect_owned_script_servo_function(parameters, setup, map);
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

fn normalize_lua_script_for_compare(script: &str) -> String {
    script
        .lines()
        .filter(|line| !line.starts_with("--- Version:"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn lua_scripts_equivalent(installed: &str, expected: &str) -> bool {
    normalize_lua_script_for_compare(installed) == normalize_lua_script_for_compare(expected)
}

/// The script for `cameras`, checked to be valid Lua.
pub(super) fn generate_checked_lua_script(cameras: &[&CameraActuators]) -> Result<String> {
    let contents = generate_lua_script(cameras)?;
    validate_lua(&contents)?;
    Ok(contents)
}

/// What the template needs of one camera.
#[derive(Serialize)]
struct LuaCamera {
    id: u8,
    param_table_key: u8,
    param_prefix: String,
    margin_gain: f32,
    k_focus: i16,
    k_zoom: i16,
    k_focus_input: i16,
    k_script: i16,
    script_function_name: String,
    focus_name: String,
    zoom_name: String,
    focus_input_name: String,
    closest_points: String,
    furthest_points: String,
}

fn script_function_name(function: ChannelFunction) -> String {
    format!(
        "Script{}",
        function as i16 - ChannelFunction::Script1 as i16 + 1
    )
}

/// The script, the same on every firmware. One script serves every motorized camera, so cameras
/// whose focus servos share one function (the shared setup's default) have one writer.
/// Cameras on one camera id, which share a mount, share their functions, so only the first is
/// served. A camera on the shared setup's parameters is served as that setup needs, from
/// `CAMn_TYPE`, so the script does not change when ArduSub is updated.
fn generate_lua_script(cameras: &[&CameraActuators]) -> Result<String> {
    let mut context = tera::Context::new();

    let mut cameras: Vec<&CameraActuators> = cameras
        .iter()
        .copied()
        .filter(|camera| camera.parameters.has_motorized_lens())
        .collect();
    anyhow::ensure!(
        !cameras.is_empty(),
        "No motorized camera needs a Lua script"
    );
    cameras.sort_by_key(|camera| camera.parameters.camera_id as u8);
    cameras.dedup_by_key(|camera| camera.parameters.camera_id as u8);

    let cameras: Vec<LuaCamera> = cameras
        .into_iter()
        .map(|camera| {
            let parameters = &camera.parameters;
            let id = parameters.camera_id as u8;
            let (focus, zoom, input) = (
                parameters.focus_function(Setup::PerCamera),
                parameters.zoom_function(Setup::PerCamera),
                parameters.focus_input_function(Setup::PerCamera),
            );
            LuaCamera {
                id,
                param_table_key: PARAM_TABLE_KEY_BASE + id,
                param_prefix: format!("\"{PARAM_PREFIX}{id}_\""),
                margin_gain: parameters.focus_margin_gain,
                k_focus: focus as i16,
                k_zoom: zoom as i16,
                k_focus_input: input as i16,
                k_script: parameters.focus_function(Setup::Shared) as i16,
                script_function_name: script_function_name(
                    parameters.focus_function(Setup::Shared),
                ),
                focus_name: script_function_name(focus),
                zoom_name: script_function_name(zoom),
                focus_input_name: script_function_name(input),
                closest_points: camera.closest_points.to_lua(),
                furthest_points: camera.furthest_points.to_lua(),
            }
        })
        .collect();

    context.insert("cameras", &cameras);
    context.insert("version", LUA_SCRIPT_VERSION);

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
    use mlua::{Function, Lua};

    use super::*;
    use crate::api;

    /// The second camera: CAM2, on servos 7 to 9 (zoom 8, focus 7, script 9) and tilt 15.
    fn second_camera() -> CameraActuators {
        let mut camera = CameraActuators::default();
        let parameters = &mut camera.parameters;
        parameters.camera_id = api::CameraID::CAM2;
        parameters.focus_channel = api::ServoChannel::SERVO7;
        parameters.zoom_channel = api::ServoChannel::SERVO8;
        parameters.script_channel = api::ServoChannel::SERVO9;
        parameters.tilt_channel = api::ServoChannel::SERVO15;
        camera
    }

    fn both() -> [CameraActuators; 2] {
        [CameraActuators::default(), second_camera()]
    }

    #[test]
    fn lua_scripts_equivalent_ignores_version_header() {
        let expected = generate_lua_script(&[&CameraActuators::default()]).unwrap();
        let installed = expected.replace(
            &format!("--- Version: {LUA_SCRIPT_VERSION}"),
            "--- Version: 0.3.2",
        );
        assert!(lua_scripts_equivalent(&installed, &expected));
    }

    #[test]
    fn test_script_generation() {
        let contents = generate_lua_script(&[&CameraActuators::default()]).unwrap();

        validate_lua(&contents).unwrap();

        assert!(contents.contains(&format!("--- Version: {LUA_SCRIPT_VERSION}")));
        assert!(contents.contains("warn_missing_servo_function"));
        assert!(contents.contains(SCRIPT_OWNERSHIP_MARKER));
        assert!(contents.contains("find_servo_function(K_FOCUS, \"CameraFocus\""));
        assert!(contents.contains("find_servo_function(K_ZOOM, \"CameraZoom\""));
        assert!(contents.contains("k_script = 94,"));
        assert!(contents.contains("script_function_name = \"Script1\""));
        assert!(contents.contains("SRV_Channels:set_output_pwm(camera.k_script, script_trim)"));
        assert!(contents.contains("servo function not found"));
        // The camera's own Script functions: Script1 focus, Script2 zoom, Script3 focus input.
        assert!(contents.contains("k_focus = 94,"));
        assert!(contents.contains("k_zoom = 95,"));
        assert!(contents.contains("k_focus_input = 96,"));
    }

    #[test]
    fn generated_script_uses_configured_script_function() {
        let mut actuators = CameraActuators::default();
        actuators.parameters.script_function = crate::api::ScriptFunction::SCRIPT3;
        let contents = generate_lua_script(&[&actuators]).unwrap();
        validate_lua(&contents).unwrap();
        assert!(contents.contains("k_script = 96,"));
        assert!(contents.contains("script_function_name = \"Script3\""));
    }

    #[test]
    fn one_script_serves_every_motorized_camera_once() {
        let [first, second] = both();
        let contents = generate_lua_script(&[&second, &first]).unwrap();
        validate_lua(&contents).unwrap();

        assert_eq!(contents.matches("param_table_key =").count(), 2);
        assert!(contents.contains("param_prefix = \"BR4KCAM1_\""));
        assert!(contents.contains("param_prefix = \"BR4KCAM2_\""));
        assert!(contents.contains("k_focus = 97,"));
        assert!(contents.contains("k_zoom = 98,"));
        assert!(contents.contains("k_focus_input = 99,"));
        // The order of the cameras does not change the script, so health does not flag it.
        assert_eq!(contents, generate_lua_script(&[&first, &second]).unwrap());

        // A fixed-focus lens needs none, and a script with no camera to serve is refused.
        let mut fixed = second_camera();
        fixed.parameters.lens = api::LensType::FixedFocus;
        let contents = generate_lua_script(&[&first, &fixed]).unwrap();
        assert!(!contents.contains("BR4KCAM2_"));
        assert!(generate_lua_script(&[&fixed]).is_err());
        // Cameras on one camera id share their functions, so only the first is served.
        let contents = generate_lua_script(&[&first, &first]).unwrap();
        assert_eq!(contents.matches("param_table_key =").count(), 1);
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

    /// ArduPilot's scripting bindings, as the script uses them. `STATES[i]` is the camera_state_t
    /// of the instance `i` (camera id - 1), `NOW` the time in ms.
    const AUTOPILOT: &str = r#"
        OUT, SCALED, CHANNELS, PARAMS, STATES, TEXTS, NOW = {}, {}, {}, {}, {}, {}, 0
        -- The functions written in the current run: one written twice is the script fighting
        -- itself, which fails the run.
        WRITTEN = {}
        local function ms(value) return type(value) == "table" and value.ms or value end
        local uint32 = { __le = function(a, b) return ms(a) <= ms(b) end }
        function millis()
            return setmetatable({ ms = NOW, tofloat = function() return NOW end }, uint32)
        end
        SRV_Channels = {
            find_channel = function(_, f) return CHANNELS[f] end,
            set_output_pwm = function(_, f, v)
                assert(not WRITTEN[f], "function " .. f .. " written twice in one run")
                WRITTEN[f] = true
                OUT[f] = v
            end,
            -- Only a function on a channel has an output.
            get_output_pwm = function(_, f) if CHANNELS[f] then return OUT[f] end end,
            -- Scaled 0 to 1000 over the servo's MIN to MAX, read back as its PWM.
            set_output_scaled = function(_, f, v)
                SCALED[f] = v
                local n = CHANNELS[f] and CHANNELS[f] + 1
                if n then
                    local min, max = PARAMS["SERVO" .. n .. "_MIN"], PARAMS["SERVO" .. n .. "_MAX"]
                    OUT[f] = math.floor(min + v * (max - min) / 1000 + 0.5)
                end
            end,
        }
        param = {
            get = function(_, name) return PARAMS[name] end,
            add_table = function() return true end,
            add_param = function() return true end,
        }
        Parameter = function()
            return {
                init = function(self, name) self.name = name; return true end,
                get = function(self) return PARAMS[self.name] end,
            }
        end
        gcs = { send_text = function(_, _, text) TEXTS[#TEXTS + 1] = text end }
        camera = {
            get_state = function(_, instance)
                local state = STATES[instance]
                if state == nil then return nil end
                return {
                    zoom_type = function() return state.zoom_type end,
                    zoom_value = function() return state.zoom_value end,
                    focus_type = function() return state.focus_type end,
                    focus_value = function() return state.focus_value end,
                }
            end,
        }
        -- A camera that has not been commanded yet, as ArduPilot zero-initializes it.
        function state(instance)
            STATES[instance] = STATES[instance]
                or { zoom_type = 0, zoom_value = 0, focus_type = 0, focus_value = 0 }
            return STATES[instance]
        end
    "#;

    /// The per-camera setup's script of `cameras` running in a simulated autopilot, each camera
    /// on its own servos.
    struct Sim {
        lua: Lua,
        update: Function,
        cameras: Vec<(CameraActuators, Setup)>,
    }

    /// Puts each camera's servos on its functions in `setup`, as the autopilot holds them.
    fn put_on_channels(lua: &Lua, camera: &CameraActuators, setup: Setup) {
        let parameters = &camera.parameters;
        for (function, channel, min, trim, max) in [
            (
                parameters.focus_function(setup),
                parameters.focus_channel,
                parameters.focus_channel_min,
                parameters.focus_channel_trim,
                parameters.focus_channel_max,
            ),
            (
                parameters.zoom_function(setup),
                parameters.zoom_channel,
                parameters.zoom_channel_min,
                parameters.zoom_channel_trim,
                parameters.zoom_channel_max,
            ),
            (
                parameters.focus_input_function(setup),
                parameters.script_channel,
                parameters.script_channel_min,
                parameters.script_channel_trim,
                parameters.script_channel_max,
            ),
        ] {
            let channel_number = channel as u8;
            lua.load(format!(
                "CHANNELS[{}] = {}; PARAMS.SERVO{channel_number}_MIN = {min}; \
                 PARAMS.SERVO{channel_number}_TRIM = {trim}; PARAMS.SERVO{channel_number}_MAX = {max}",
                function as i16,
                channel_number - 1,
            ))
            .exec()
            .unwrap();
        }
        let id = parameters.camera_id as u8;
        let camera_type = match setup {
            Setup::Shared => 1,
            Setup::PerCamera => 7,
        };
        lua.load(format!(
            "PARAMS.BR4KCAM{id}_ENABLE = {}; PARAMS.BR4KCAM{id}_GAIN = {}; PARAMS.CAM{id}_TYPE = {camera_type}",
            u8::from(parameters.enable_focus_and_zoom_correlation),
            parameters.focus_margin_gain,
        ))
        .exec()
        .unwrap();
    }

    impl Sim {
        fn new(cameras: &[&CameraActuators]) -> Self {
            let cameras: Vec<(&CameraActuators, Setup)> = cameras
                .iter()
                .map(|camera| (*camera, Setup::PerCamera))
                .collect();
            Self::with(&cameras)
        }

        /// Each camera with the parameters of `setup` on the autopilot.
        fn with(cameras: &[(&CameraActuators, Setup)]) -> Self {
            let lua = Lua::new();
            lua.load(AUTOPILOT).exec().unwrap();
            for (camera, setup) in cameras {
                put_on_channels(&lua, camera, *setup);
                // Every scripting camera has a state from boot, with no command in it.
                if *setup == Setup::PerCamera {
                    lua.load(format!("state({})", camera.parameters.camera_id as u8 - 1))
                        .exec()
                        .unwrap();
                }
            }
            let served: Vec<&CameraActuators> = cameras.iter().map(|(camera, _)| *camera).collect();
            let script = generate_lua_script(&served).unwrap();
            let (update, _delay): (Function, i64) = lua.load(script).call(()).unwrap();
            let mut sim = Self {
                lua,
                update,
                cameras: cameras
                    .iter()
                    .map(|(camera, setup)| ((*camera).clone(), *setup))
                    .collect(),
            };
            // The script's start, then its first update.
            sim.tick();
            sim.tick();
            sim
        }

        /// A command to a Servo camera: ArduPilot drives its one CameraFocus and CameraZoom,
        /// on the default ranges, whichever camera it reaches. Then a run.
        fn servo_command(&mut self, focus: f64, zoom: f64) {
            self.lua
                .load(format!(
                    "OUT[92] = {}; OUT[180] = {}",
                    focus_input_pwm(focus),
                    zoom_pwm(zoom)
                ))
                .exec()
                .unwrap();
            self.tick();
        }

        /// Runs the script once, 100 ms (its period) after the last run.
        fn tick(&mut self) {
            self.lua
                .load("NOW = NOW + 100; WRITTEN = {}")
                .exec()
                .unwrap();
            let (next, _delay): (Function, i64) = self.update.call(()).unwrap();
            self.update = next;
        }

        /// What ArduPilot stores for camera `camera_id` when a command reaches it, then a run.
        fn command(&mut self, camera_id: u8, field: &str, kind: u8, value: f32) {
            self.lua
                .load(
                    format!(
                        "local s = state({}); s.{field}_type = {kind}; s.{field}_value = {value}",
                        camera_id - 1
                    )
                    .replace("NaN", "(0/0)"),
                )
                .exec()
                .unwrap();
            self.tick();
        }

        fn zoom(&mut self, value: f32, camera_id: u8) {
            self.command(camera_id, "zoom", 2, value);
        }

        fn focus(&mut self, value: f32, camera_id: u8) {
            self.command(camera_id, "focus", 2, value);
        }

        /// The outputs of the camera on `index`: zoom, focus, and the focus asked for.
        fn outputs(&self, index: usize) -> [Option<i64>; 3] {
            let (camera, setup) = &self.cameras[index];
            let parameters = &camera.parameters;
            [
                parameters.zoom_function(*setup),
                parameters.focus_function(*setup),
                parameters.focus_input_function(*setup),
            ]
            .map(|function| {
                self.lua
                    .load(format!("return OUT[{}]", function as i16))
                    .eval()
                    .unwrap()
            })
        }

        fn global<T: mlua::FromLua>(&self, expression: &str) -> T {
            self.lua
                .load(format!("return {expression}"))
                .eval()
                .unwrap()
        }
    }

    // Zoom pwm 935 to 1850 and focus input pwm 870 to 2130, as the defaults.
    fn zoom_pwm(percent: f64) -> i64 {
        (935.0 + percent * (1850.0 - 935.0) / 100.0).round() as i64
    }

    fn focus_input_pwm(percent: f64) -> i64 {
        (870.0 + percent * (2130.0 - 870.0) / 100.0).round() as i64
    }

    #[test]
    fn the_script_leaves_the_commands_to_ardupilot() {
        let [first, _] = both();
        let contents = generate_lua_script(&[&first]).unwrap();
        assert!(!contents.contains("block_command(") && !contents.contains("register_rx_msgid("));
        assert!(contents.contains("get_state(camera.id - 1)"));
    }

    #[test]
    fn the_state_of_one_camera_moves_only_that_cameras_outputs() {
        let [first, second] = both();
        let mut sim = Sim::new(&[&first, &second]);
        let (rest_first, rest_second) = (sim.outputs(0), sim.outputs(1));

        sim.zoom(50.0, 1);
        assert_eq!(sim.outputs(0)[0], Some(zoom_pwm(50.0)));
        assert_eq!(sim.outputs(1), rest_second);
        assert_ne!(sim.outputs(0), rest_first);

        let after_zoom = sim.outputs(0);
        sim.focus(75.0, 2);
        assert_eq!(sim.outputs(0), after_zoom);
        assert_ne!(sim.outputs(1), rest_second);

        sim.zoom(100.0, 2);
        assert_eq!(sim.outputs(1)[0], Some(1850));
        assert_eq!(sim.outputs(0), after_zoom);
    }

    #[test]
    fn the_script_channel_carries_the_focus_asked_for() {
        let [first, second] = both();
        let mut sim = Sim::new(&[&first, &second]);

        // The manager reads it back to check the script runs: the focus input range is the
        // script channel's, and it does not depend on the zoom or the calibration.
        sim.focus(30.0, 1);
        sim.zoom(80.0, 1);
        assert_eq!(
            sim.outputs(0)[2],
            Some(870 + (0.3_f64 * 1260.0).round() as i64)
        );
        sim.focus(75.0, 2);
        assert_eq!(
            sim.outputs(1)[2],
            Some(870 + (0.75_f64 * 1260.0).round() as i64)
        );
        assert_eq!(
            sim.outputs(0)[2],
            Some(870 + (0.3_f64 * 1260.0).round() as i64)
        );
    }

    #[test]
    fn a_camera_not_commanded_yet_keeps_the_trim() {
        let [first, second] = both();
        let mut sim = Sim::new(&[&first, &second]);
        let at_trim = |sim: &Sim, index: usize| {
            let parameters = &sim.cameras[index].0.parameters;
            let [zoom, _, input] = sim.outputs(index);
            zoom == Some(i64::from(parameters.zoom_channel_trim))
                && input == Some(i64::from(parameters.script_channel_trim))
        };

        // A boot: nothing set the outputs, however long the script runs.
        for _ in 0..20 {
            sim.tick();
        }
        assert!(at_trim(&sim, 0) && at_trim(&sim, 1));
        // What the other camera is told does not matter to it.
        sim.zoom(70.0, 2);
        assert!(at_trim(&sim, 0) && !at_trim(&sim, 1));
    }

    #[test]
    fn a_rate_zoom_integrates_over_time_and_clamps() {
        let [first, second] = both();
        let mut sim = Sim::new(&[&first, &second]);
        let start = 100.0 * (f64::from(first.parameters.zoom_channel_trim) - 935.0) / 915.0;
        let rest_second = sim.outputs(1);

        // 1 s at +1 and the default 5 %/s: 5 % further.
        sim.command(1, "zoom", 1, 1.0);
        for _ in 0..9 {
            sim.tick();
        }
        assert_eq!(sim.outputs(0)[0], Some(zoom_pwm(start + 5.0)));

        // The camera's own CAM1_ZOM_RAT_MAX, here 20 %/s, and a rate of half.
        sim.lua.load("PARAMS.CAM1_ZOM_RAT_MAX = 20").exec().unwrap();
        sim.command(1, "zoom", 1, 0.5);
        for _ in 0..9 {
            sim.tick();
        }
        assert_eq!(sim.outputs(0)[0], Some(zoom_pwm(start + 15.0)));

        // Zero holds, however long.
        sim.command(1, "zoom", 1, 0.0);
        for _ in 0..30 {
            sim.tick();
        }
        assert_eq!(sim.outputs(0)[0], Some(zoom_pwm(start + 15.0)));

        // Clamped at both ends, with a rate past 1 taken as 1.
        sim.command(1, "zoom", 1, 5.0);
        for _ in 0..100 {
            sim.tick();
        }
        assert_eq!(sim.outputs(0)[0], Some(1850));
        sim.command(1, "zoom", 1, -1.0);
        for _ in 0..100 {
            sim.tick();
        }
        assert_eq!(sim.outputs(0)[0], Some(935));
        // The other camera never moved.
        assert_eq!(sim.outputs(1), rest_second);
    }

    #[test]
    fn a_rate_focus_integrates_and_the_script_channel_follows() {
        let [first, _] = both();
        let mut sim = Sim::new(&[&first]);
        sim.focus(50.0, 1);
        sim.command(1, "focus", 1, 1.0);
        for _ in 0..99 {
            sim.tick();
        }
        // 10 s at 5 %/s from 50 %.
        assert_eq!(
            sim.outputs(0)[2],
            Some(870 + (1.0_f64 * 1260.0).round() as i64)
        );
    }

    #[test]
    fn types_the_script_does_not_follow_keep_the_position() {
        let [first, _] = both();
        let mut sim = Sim::new(&[&first]);
        sim.zoom(40.0, 1);
        sim.focus(60.0, 1);
        let held = sim.outputs(0);

        // Autofocus (4), a type it does not know, and a NaN value change nothing.
        sim.command(1, "focus", 4, 0.0);
        sim.command(1, "zoom", 9, 100.0);
        sim.command(1, "zoom", 2, f32::NAN);
        assert_eq!(sim.outputs(0), held);
    }

    #[test]
    fn a_percentage_out_of_range_is_clamped() {
        let [first, _] = both();
        let mut sim = Sim::new(&[&first]);

        sim.zoom(250.0, 1);
        assert_eq!(sim.outputs(0)[0], Some(1850));
        sim.zoom(-5.0, 1);
        assert_eq!(sim.outputs(0)[0], Some(935));
    }

    #[test]
    fn a_stalled_script_does_not_jump_when_it_runs_again() {
        let [first, _] = both();
        let mut sim = Sim::new(&[&first]);
        sim.zoom(10.0, 1);
        sim.command(1, "zoom", 1, 1.0);
        sim.lua.load("NOW = NOW + 600000").exec().unwrap();
        sim.tick();
        // Ten minutes of stall count as 1 s at most (5 %).
        assert!(sim.outputs(0)[0].unwrap() <= zoom_pwm(10.0 + 5.0 + 1.0));
    }

    #[test]
    fn the_focus_follows_the_zoom_through_the_calibration() {
        let [mut first, _] = both();
        let mut sim = Sim::new(&[&first]);
        // Focus input 1500 (50 %) at zoom 935 (0 %): between the closest and furthest lines.
        sim.focus(50.0, 1);
        sim.zoom(0.0, 1);
        let at_wide = sim.outputs(0)[1].unwrap();
        sim.zoom(100.0, 1);
        let at_tele = sim.outputs(0)[1].unwrap();
        assert!((834..=963).contains(&at_wide), "{at_wide}");
        assert_ne!(at_wide, at_tele);

        // Without the correlation the focus goes straight through.
        first.parameters.enable_focus_and_zoom_correlation = false;
        let mut sim = Sim::new(&[&first]);
        sim.focus(50.0, 1);
        sim.zoom(100.0, 1);
        assert_eq!(sim.outputs(0)[1], sim.outputs(0)[2]);
    }

    #[test]
    fn a_restart_keeps_the_position_and_a_boot_starts_at_trim() {
        let [first, _] = both();
        // A boot: the outputs were never set, the zoom starts at its trim.
        let sim = Sim::new(&[&first]);
        assert_eq!(
            sim.outputs(0)[0],
            Some(i64::from(first.parameters.zoom_channel_trim))
        );

        // A restart: the outputs keep what the last run set.
        let mut restarted = Sim::new(&[&first]);
        restarted.zoom(40.0, 1);
        let script = generate_lua_script(&[&first]).unwrap();
        let (update, _): (Function, i64) = restarted.lua.load(script).call(()).unwrap();
        restarted.update = update;
        restarted.tick();
        restarted.tick();
        assert_eq!(restarted.outputs(0)[0], Some(zoom_pwm(40.0)));
    }

    #[test]
    fn a_missing_servo_function_is_reported_once_and_does_not_stop_the_script() {
        let [first, _] = both();
        let mut sim = Sim::new(&[&first]);
        sim.lua
            .load(format!(
                "CHANNELS[{}] = nil",
                first.parameters.zoom_function(Setup::PerCamera) as i16
            ))
            .exec()
            .unwrap();
        sim.tick();
        sim.tick();

        assert_eq!(
            sim.global::<i64>("#TEXTS"),
            1,
            "{:?}",
            sim.global::<Vec<String>>("TEXTS")
        );
        assert!(
            sim.global::<String>("TEXTS[1]")
                .contains("Script2 servo function not found")
        );
    }

    #[test]
    fn the_shared_setups_script_is_flagged_when_its_output_stays_while_its_input_moves() {
        let mut running = ScriptHealthTracker::default();
        for (input, output) in [(1000, 900), (1200, 950), (1400, 1000), (1600, 1050)] {
            assert!(!running.update(input, output));
        }

        // Flagged on the third reading in a row, then counting again.
        let mut stopped = ScriptHealthTracker::default();
        assert!(!stopped.update(1000, 900));
        assert!(!stopped.update(1200, 900));
        assert!(!stopped.update(1400, 900));
        assert!(stopped.update(1600, 900));
        assert!(!stopped.update(1800, 900));
        // An input that does not move says nothing.
        let mut idle = ScriptHealthTracker::default();
        for _ in 0..5 {
            assert!(!idle.update(1500, 900));
        }
    }

    /// For CameraFocus and CameraZoom at each (focus, zoom) %, what origin/master's one-camera
    /// script (e4644e59) writes to the focus servo of a camera on the default calibration, with
    /// gain 1 and with gain 1.5.
    const SHARED_SCRIPT_FOCUS: [((f64, f64), i64, i64); 3] = [
        ((30.0, 0.0), 975, 981),
        ((70.0, 60.0), 1218, 1146),
        ((50.0, 100.0), 1953, 1862),
    ];

    #[test]
    fn servo_cameras_are_driven_as_the_shared_setups_script_drives_them() {
        // Before ArduSub 4.8, or after an update before the cameras are set up again: both are
        // Servo cameras, and ArduPilot drives the one CameraFocus and CameraZoom, so both follow
        // every command. Each focus servo on its own function follows its own calibration.
        let [first, mut second] = both();
        second.parameters.focus_margin_gain = 1.5;
        second.parameters.script_function = api::ScriptFunction::SCRIPT2;
        let mut sim = Sim::with(&[(&first, Setup::Shared), (&second, Setup::Shared)]);
        // After a boot they start at their trims: zoom 935 (its minimum), focus input 1500.
        assert_eq!(sim.global::<f64>("SCALED[180]"), 0.0);
        assert_eq!(sim.global::<f64>("SCALED[92]"), 500.0);
        for ((focus, zoom), gain_1, gain_1_5) in SHARED_SCRIPT_FOCUS {
            sim.servo_command(focus, zoom);
            assert_eq!(sim.outputs(0)[1], Some(gain_1), "{focus} {zoom}");
            assert_eq!(sim.outputs(1)[1], Some(gain_1_5), "{focus} {zoom}");
        }
        assert_eq!(sim.global::<i64>("#TEXTS"), 0);
    }

    #[test]
    fn servo_cameras_sharing_one_focus_function_drive_it_once_from_the_first() {
        // origin/master's default: both focus servos on Script1. ArduPilot gives both cameras
        // the same CameraFocus and CameraZoom, so one calibration drives the one function, the
        // first camera's, and the run never writes it twice (the simulation fails on that).
        let [first, mut second] = both();
        second.parameters.focus_margin_gain = 1.5;
        let mut sim = Sim::with(&[(&second, Setup::Shared), (&first, Setup::Shared)]);
        for ((focus, zoom), gain_1, _) in SHARED_SCRIPT_FOCUS {
            sim.servo_command(focus, zoom);
            assert_eq!(sim.outputs(0)[1], Some(gain_1), "{focus} {zoom}");
            assert_eq!(sim.outputs(1)[1], Some(gain_1), "{focus} {zoom}");
        }
    }

    #[test]
    fn a_servo_camera_and_a_scripting_one_each_follow_their_own_commands() {
        // One camera set up again after an ArduSub update, the other not yet.
        let [first, second] = both();
        let mut sim = Sim::with(&[(&first, Setup::Shared), (&second, Setup::PerCamera)]);
        let rest_second = sim.outputs(1);

        let ((focus, zoom), gain_1, _) = SHARED_SCRIPT_FOCUS[1];
        sim.servo_command(focus, zoom);
        assert_eq!(sim.outputs(0)[1], Some(gain_1));
        assert_eq!(sim.outputs(1), rest_second);

        let first_held = sim.outputs(0);
        sim.zoom(80.0, 2);
        sim.focus(20.0, 2);
        assert_eq!(sim.outputs(1)[0], Some(zoom_pwm(80.0)));
        assert_eq!(sim.outputs(1)[2], Some(focus_input_pwm(20.0)));
        assert_eq!(sim.outputs(0), first_held);
    }

    #[test]
    fn a_servo_camera_never_drives_a_function_a_scripting_camera_owns() {
        // CAM1 set up again owns Script1 to Script3, while CAM2's focus servo is still on
        // Script1 from the shared setup: CAM2's commands must not move CAM1's focus.
        let [first, second] = both();
        let mut sim = Sim::with(&[(&first, Setup::PerCamera), (&second, Setup::Shared)]);
        sim.focus(30.0, 1);
        sim.zoom(60.0, 1);
        let held = sim.outputs(0);

        sim.servo_command(90.0, 90.0);
        sim.tick();
        assert_eq!(sim.outputs(0), held);
    }

    #[test]
    fn a_camera_with_its_setup_partly_applied_holds_still() {
        let [first, second] = both();

        // The Scripting type, but still the shared functions: no Script2 or Script3 to drive.
        let mut sim = Sim::with(&[(&first, Setup::Shared), (&second, Setup::PerCamera)]);
        sim.lua.load("PARAMS.CAM1_TYPE = 7").exec().unwrap();
        let held = sim.outputs(0)[1];
        sim.servo_command(30.0, 40.0);
        sim.servo_command(80.0, 90.0);
        // Its focus servo is no longer driven (zoom and focus input are ArduPilot's own here).
        assert_eq!(sim.outputs(0)[1], held);
        assert!(
            sim.global::<String>("TEXTS[1]")
                .contains("servo function not found")
        );
        // The other camera goes on.
        sim.zoom(70.0, 2);
        assert_eq!(sim.outputs(1)[0], Some(zoom_pwm(70.0)));

        // The per-camera functions, but still the Servo type: no CameraFocus to follow.
        let mut sim = Sim::with(&[(&first, Setup::PerCamera)]);
        sim.focus(40.0, 1);
        let held = sim.outputs(0);
        sim.lua.load("PARAMS.CAM1_TYPE = 1").exec().unwrap();
        sim.servo_command(80.0, 90.0);
        assert_eq!(sim.outputs(0), held);
    }
}
