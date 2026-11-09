//! Hardware setup planning.
//!
//! An apply is decided here from a [`Snapshot`] of the saved setups, the autopilot parameters
//! and the installed script, without touching any of them, so it can be tested and previewed
//! without an autopilot. [`super::Manager::execute`] then carries the [`ApplyPlan`] out.

use anyhow::{Context, Result};
use indexmap::IndexMap;
use uuid::Uuid;

use super::{FIRMWARE_UNKNOWN, MANAGER, camera, focus, owned_parameters, script, tilt, zoom};
use crate::{
    CameraActuators, api,
    parameters::{
        ActuatorsParameters, CameraType, ChannelFunction, ParamType, Parameter, Setup,
        param_display_value,
    },
};

/// One autopilot parameter write.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedWrite {
    pub name: String,
    pub value: ParamType,
    /// Write even when the autopilot already holds `value`.
    pub force: bool,
}

/// What happens to the Lua script file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ScriptAction {
    Keep,
    Write(String),
    Delete,
}

#[derive(Debug)]
pub(crate) struct ApplyPlan {
    /// Hardware setup writes, in order.
    pub writes: Vec<PlannedWrite>,
    /// Writes that release what the camera wrote and no longer uses, after [`Self::writes`].
    pub releases: Vec<PlannedWrite>,
    pub script: ScriptAction,
    /// Restart the autopilot's scripts, as no reboot follows.
    pub reload_scripts: bool,
    pub reboot: bool,
    /// Script parameters written once the scripts reloaded, as no reboot follows. They only
    /// exist while the script runs; after a reboot [`script::param_writes`] writes them all.
    pub after_reload: Vec<PlannedWrite>,
    /// The camera's setup once applied.
    pub next: CameraActuators,
}

/// Everything planning reads.
#[derive(Debug, Default, Clone)]
pub(crate) struct Snapshot {
    pub saved: IndexMap<Uuid, CameraActuators>,
    /// The autopilot's parameter cache.
    pub autopilot: IndexMap<String, Parameter>,
    pub script_path: String,
    /// The installed Lua script, if any.
    pub installed_script: Option<String>,
    /// A reboot an earlier apply asked for has not finished yet.
    pub reboot_pending: bool,
    /// The setup the autopilot's firmware takes, `None` until its version is read.
    pub setup: Option<Setup>,
}

impl Snapshot {
    pub(crate) async fn capture() -> Result<Self> {
        let (saved, script_path) = {
            let manager = MANAGER.get().context("Not available")?.read().await;
            (
                manager.settings.actuators.clone(),
                manager.autopilot_scripts_file.clone(),
            )
        };
        // Without the autopilot's parameters every value would look unheld, so nothing is planned.
        let autopilot = crate::mavlink::component()?
            .inner
            .parameters
            .read()
            .await
            .clone();
        let installed_script = tokio::fs::read_to_string(&script_path).await.ok();

        Ok(Self {
            saved,
            autopilot,
            script_path,
            installed_script,
            reboot_pending: super::reboot_pending(),
            setup: crate::health::setup(),
        })
    }
}

/// What one section of the setup changes.
#[derive(Debug, Clone, Copy)]
pub(super) struct Change<'a> {
    pub current: &'a ActuatorsParameters,
    pub next: &'a ActuatorsParameters,
    /// The values the apply was asked for; `None` keeps the current one.
    pub incoming: &'a api::ActuatorsParametersConfig,
    /// Write the section's values even when the autopilot already holds them.
    pub overwrite: bool,
    /// The camera id changes, which renumbers the mount and script parameters.
    pub camera_moved: bool,
    pub setup: Setup,
}

impl Change<'_> {
    /// The per-camera setup's functions follow the camera id, so a camera id move moves them.
    pub fn functions_moved(&self) -> bool {
        self.camera_moved && self.setup == Setup::PerCamera
    }
}

pub(super) struct Planner<'a> {
    autopilot: &'a IndexMap<String, Parameter>,
    writes: Vec<PlannedWrite>,
    releases: Vec<PlannedWrite>,
}

impl<'a> Planner<'a> {
    fn new(autopilot: &'a IndexMap<String, Parameter>) -> Self {
        Self {
            autopilot,
            writes: Vec::new(),
            releases: Vec::new(),
        }
    }

    /// Whether the autopilot is known to hold `value` for `name`.
    pub(super) fn autopilot_holds(&self, name: &str, value: &ParamType) -> bool {
        self.autopilot
            .get(name)
            .and_then(|parameter| param_display_value(&parameter.value))
            .is_some_and(|held| Some(held) == param_display_value(value))
    }

    /// Writes `value` unless the autopilot already holds it, or anyway with `force`. Returns
    /// whether it is written.
    pub(super) fn write_if_needed(&mut self, name: String, value: ParamType, force: bool) -> bool {
        let needed = force || !self.autopilot_holds(&name, &value);
        if needed {
            self.writes.push(PlannedWrite { name, value, force });
        }
        needed
    }

    /// Writes `value` when the saved setting changes from `current` to `next`, or anyway
    /// with `force`.
    pub(super) fn write_if_changed<T: PartialEq>(
        &mut self,
        name: String,
        current: T,
        next: T,
        value: ParamType,
        force: bool,
    ) {
        if force || current != next {
            self.writes.push(PlannedWrite {
                name,
                value,
                force: true,
            });
        }
    }

    /// Gives `channel` a role's `function`. The channel the role leaves is released by
    /// [`plan_apply`]. Returns whether the function is written.
    pub(super) fn channel_function(
        &mut self,
        channel: api::ServoChannel,
        function: ChannelFunction,
        overwrite: bool,
    ) -> bool {
        self.write_if_needed(
            servo_param(channel, "FUNCTION"),
            ParamType::INT16(function as i16),
            overwrite,
        )
    }

    /// Clears `SERVOn_REVERSED` on a channel a role with no reversed setting takes, so the
    /// channel does not stay reversed from the role that had it before.
    pub(super) fn clear_reversed(&mut self, channel: api::ServoChannel) {
        self.write_if_needed(servo_param(channel, "REVERSED"), ParamType::UINT8(0), false);
    }

    /// `SERVO{channel}_MIN`, `_TRIM` and `_MAX`, each written when it changes, or all of them
    /// with `force`.
    pub(super) fn channel_range(
        &mut self,
        channel: api::ServoChannel,
        current: [u16; 3],
        next: [u16; 3],
        force: bool,
    ) {
        for ((suffix, current), next) in ["MIN", "TRIM", "MAX"].into_iter().zip(current).zip(next) {
            self.write_if_changed(
                servo_param(channel, suffix),
                current,
                next,
                ParamType::UINT16(next),
                force,
            );
        }
    }
}

pub(super) fn servo_param(channel: api::ServoChannel, suffix: &str) -> String {
    format!("SERVO{}_{suffix}", channel as u8)
}

/// The value that turns off a channel function or camera type, `None` for any other parameter.
fn released_value(name: &str) -> Option<ParamType> {
    if name.starts_with("SERVO") && name.ends_with("_FUNCTION") {
        Some(ParamType::INT16(ChannelFunction::Disabled as i16))
    } else if name.starts_with("CAM") && name.ends_with("_TYPE") {
        Some(ParamType::UINT8(CameraType::None as u8))
    } else {
        None
    }
}

/// The parameters each other camera's saved setup owns, with their values.
fn other_cameras(
    snapshot: &Snapshot,
    camera_uuid: Uuid,
    setup: Setup,
) -> Vec<(Uuid, IndexMap<String, ParamType>)> {
    snapshot
        .saved
        .iter()
        .filter(|(uuid, _)| **uuid != camera_uuid)
        .map(|(uuid, actuators)| {
            let mut owned = IndexMap::new();
            owned_parameters::push_all_expectations(&actuators.parameters, setup, &mut owned);
            (*uuid, owned)
        })
        .collect()
}

/// Refuses a setup that puts two of its roles on one channel, as only one function can drive
/// it. A fixed-focus lens only uses tilt, so its other channels may hold anything.
fn refuse_shared_channels(next: &ActuatorsParameters) -> Result<()> {
    let mut channels = vec![("tilt", next.tilt_channel)];
    if next.has_motorized_lens() {
        channels.extend([
            ("focus", next.focus_channel),
            ("script", next.script_channel),
            ("zoom", next.zoom_channel),
        ]);
    }
    for (index, (role, channel)) in channels.iter().enumerate() {
        if let Some((other, _)) = channels[..index].iter().find(|(_, used)| used == channel) {
            anyhow::bail!(
                "SERVO{} is set for both {other} and {role}, pick another channel",
                *channel as u8
            );
        }
    }
    Ok(())
}

/// Refuses a setup that gives a channel function or camera type a value other than the one
/// another camera's setup holds it with, so an apply or a reset never takes over a channel
/// another camera uses.
fn refuse_taking_other_cameras(
    snapshot: &Snapshot,
    camera_uuid: Uuid,
    next: &ActuatorsParameters,
    setup: Setup,
) -> Result<()> {
    let mut ours = IndexMap::new();
    owned_parameters::push_all_expectations(next, setup, &mut ours);
    for (uuid, theirs) in other_cameras(snapshot, camera_uuid, setup) {
        for (name, value) in &ours {
            if released_value(name).is_some()
                && theirs
                    .get(name)
                    .is_some_and(|their_value| their_value != value)
            {
                anyhow::bail!("{name} is used by camera {uuid}, pick another channel or camera id");
            }
        }
    }
    Ok(())
}

/// Turns off the channel functions and camera types the camera wrote under `written` and no
/// longer uses under `next`. One still held with this camera's value goes back to the value
/// another camera's setup gives it, or else is turned off. One the autopilot holds with any
/// other value is not this camera's anymore, so it is left alone.
///
/// Returns whether anything is written, which takes an autopilot reboot.
fn release(
    planner: &mut Planner,
    snapshot: &Snapshot,
    camera_uuid: Uuid,
    written: &ActuatorsParameters,
    next: Option<&ActuatorsParameters>,
    setup: Setup,
) -> bool {
    let mut others = IndexMap::new();
    for (_, owned) in other_cameras(snapshot, camera_uuid, setup) {
        others.extend(owned);
    }
    let mut ours = IndexMap::new();
    owned_parameters::push_all_expectations(written, setup, &mut ours);
    let mut used = IndexMap::new();
    if let Some(next) = next {
        owned_parameters::push_all_expectations(next, setup, &mut used);
    }

    let mut wrote = false;
    for (name, value) in ours {
        if used.contains_key(&name) || !planner.autopilot_holds(&name, &value) {
            continue;
        }
        let Some(release_value) = others.get(&name).copied().or_else(|| released_value(&name))
        else {
            continue;
        };
        if !planner.autopilot_holds(&name, &release_value) {
            planner.releases.push(PlannedWrite {
                name,
                value: release_value,
                force: false,
            });
            wrote = true;
        }
    }
    wrote
}

/// The motorized cameras the one script serves once `camera_uuid` is set up as `next`, or
/// forgotten with `None`, in saved order.
fn script_cameras<'a>(
    snapshot: &'a Snapshot,
    camera_uuid: Uuid,
    next: Option<&'a CameraActuators>,
) -> Vec<&'a CameraActuators> {
    let mut cameras: Vec<&CameraActuators> = snapshot
        .saved
        .iter()
        .filter_map(|(uuid, actuators)| {
            if *uuid == camera_uuid {
                next
            } else {
                Some(actuators)
            }
        })
        .collect();
    if let Some(next) = next
        && !snapshot.saved.contains_key(&camera_uuid)
    {
        cameras.push(next);
    }
    cameras.retain(|camera| camera.parameters.has_motorized_lens());
    cameras
}

/// What happens to the script once `camera_uuid` stops using it: it is rewritten for the other
/// motorized cameras, or goes away with the last one.
fn script_without(snapshot: &Snapshot, camera_uuid: Uuid) -> Result<ScriptAction> {
    let others = script_cameras(snapshot, camera_uuid, None);
    if others.is_empty() {
        return Ok(if snapshot.installed_script.is_some() {
            ScriptAction::Delete
        } else {
            ScriptAction::Keep
        });
    }
    let contents = script::generate_checked_lua_script(&others)?;
    Ok(if script_installed(snapshot, &contents) {
        ScriptAction::Keep
    } else {
        ScriptAction::Write(contents)
    })
}

/// Whether the installed script is `contents`.
fn script_installed(snapshot: &Snapshot, contents: &str) -> bool {
    snapshot
        .installed_script
        .as_deref()
        .is_some_and(|installed| script::lua_scripts_equivalent(installed, contents))
}

/// The releases of `plan` as `name: old → new`, and what happens to the Lua script when the
/// camera stops using it.
pub(crate) fn describe_releases(plan: &ApplyPlan, snapshot: &Snapshot) -> Vec<String> {
    let display = |value: Option<f32>| value.map_or_else(|| "?".to_string(), |v| v.to_string());
    let mut lines: Vec<String> = plan
        .releases
        .iter()
        .map(|release| {
            let current = snapshot
                .autopilot
                .get(&release.name)
                .and_then(|parameter| param_display_value(&parameter.value));
            format!(
                "{}: {} → {}",
                release.name,
                display(current),
                display(param_display_value(&release.value))
            )
        })
        .collect();

    let file_name = std::path::Path::new(&snapshot.script_path)
        .file_name()
        .map_or_else(
            || snapshot.script_path.clone(),
            |name| name.to_string_lossy().into_owned(),
        );
    match &plan.script {
        ScriptAction::Delete => lines.push(format!("Lua script {file_name}: removed")),
        ScriptAction::Write(_) if !plan.next.parameters.has_motorized_lens() => lines.push(
            format!("Lua script {file_name}: rewritten for the other motorized cameras"),
        ),
        _ => {}
    }
    lines
}

/// Plans forgetting `camera_uuid`: the writes that release what it holds and what happens to
/// the script it used, or `None` when it has no saved setup.
pub(crate) fn plan_forget(
    snapshot: &Snapshot,
    camera_uuid: Uuid,
) -> Result<Option<(Vec<PlannedWrite>, ScriptAction)>> {
    let Some(saved) = snapshot.saved.get(&camera_uuid) else {
        return Ok(None);
    };
    let setup = snapshot.setup.context(FIRMWARE_UNKNOWN)?;
    let mut planner = Planner::new(&snapshot.autopilot);
    release(
        &mut planner,
        snapshot,
        camera_uuid,
        &saved.parameters,
        None,
        setup,
    );
    let script = if saved.parameters.has_motorized_lens() {
        script_without(snapshot, camera_uuid)?
    } else {
        ScriptAction::Keep
    };
    Ok(Some((planner.releases, script)))
}

/// Plans applying `config` to `camera_uuid`. With `overwrite`, or for a camera with no saved
/// setup, every value is written even when the autopilot already holds it.
pub(crate) fn plan_apply(
    snapshot: &Snapshot,
    camera_uuid: Uuid,
    config: &api::ActuatorsConfig,
    overwrite: bool,
) -> Result<ApplyPlan> {
    // Each setup only works on its own firmware, so none is planned before the version is read.
    let setup = snapshot.setup.context(FIRMWARE_UNKNOWN)?;
    let saved = snapshot.saved.get(&camera_uuid);
    let overwrite = overwrite || saved.is_none();
    let base = saved.cloned().unwrap_or_default();
    // shortcut: requested calibration points are not applied, the saved ones are kept; apply
    // them here once the script can take new points.
    if config
        .closest_points
        .as_ref()
        .is_some_and(|points| *points != base.closest_points)
        || config
            .furthest_points
            .as_ref()
            .is_some_and(|points| *points != base.furthest_points)
    {
        tracing::warn!("Calibration points are not applied yet, the saved ones are kept");
    }

    let mut incoming = config.parameters.clone().unwrap_or_default();
    let current = &base.parameters;
    let mut next = ActuatorsParameters::from(
        merge_struct::merge(&api::ActuatorsParametersConfig::from(current), &incoming)
            .context("Failing to merge structs")?,
    );
    // The camera id is not picked by the user, and it numbers the mount, so two cameras with
    // their own tilt servos under one id would both tilt on every tilt command. Such a camera
    // moves to the id no other camera holds. Cameras on the same tilt servo share the mount.
    if config.parameters.is_some() {
        let others: Vec<&ActuatorsParameters> = snapshot
            .saved
            .iter()
            .filter(|(uuid, _)| **uuid != camera_uuid)
            .map(|(_, actuators)| &actuators.parameters)
            .collect();
        let taken: Vec<api::CameraID> = others.iter().map(|other| other.camera_id).collect();
        if others.iter().any(|other| {
            other.camera_id == next.camera_id && other.tilt_channel != next.tilt_channel
        }) {
            next.camera_id = [api::CameraID::CAM1, api::CameraID::CAM2]
                .into_iter()
                .find(|id| !taken.contains(id))
                .context("Every camera id is used by another camera")?;
            incoming.camera_id = Some(next.camera_id);
        }
    }
    // What a section moves away from is what the camera wrote: its saved setup, even when the
    // apply starts from the defaults. A camera with no saved setup wrote nothing, so it has no
    // old channel or camera type to turn off.
    let written = saved.map_or(&next, |actuators| &actuators.parameters);
    // Mount and script parameters are numbered by the camera id.
    let camera_moved = written.camera_id != next.camera_id;
    let motorized = next.has_motorized_lens();
    // A camera turning motorized never wrote its focus, zoom and script setup, or had it
    // released, so all of it is written again from what it asks for now.
    let turning_motorized = motorized && !written.has_motorized_lens();
    // What a fixed-focus lens stops using is released below, like any channel a setup leaves.
    let releasing = !motorized && written.has_motorized_lens();

    let mut planner = Planner::new(&snapshot.autopilot);
    // A reboot an earlier apply asked for may not have happened, and the changes it was for
    // are already on the autopilot, so nothing else would ask for it again.
    let mut reboot = overwrite || snapshot.reboot_pending;

    if config.parameters.is_some() {
        refuse_shared_channels(&next)?;
        refuse_taking_other_cameras(snapshot, camera_uuid, &next, setup)?;
        let change = Change {
            current: written,
            next: &next,
            incoming: &incoming,
            overwrite,
            camera_moved,
            setup,
        };
        if motorized {
            let incoming_motorized = api::ActuatorsParametersConfig::from(&next);
            let change = if turning_motorized {
                Change {
                    current: &next,
                    incoming: &incoming_motorized,
                    overwrite: true,
                    ..change
                }
            } else {
                change
            };
            reboot |= camera::plan(&mut planner, &change);
            reboot |= script::plan(&mut planner, &change);
            reboot |= focus::plan(&mut planner, &change);
            reboot |= zoom::plan(&mut planner, &change);
        }
        reboot |= tilt::plan(&mut planner, &change);
        if let Some(saved) = saved {
            reboot |= release(
                &mut planner,
                snapshot,
                camera_uuid,
                &saved.parameters,
                Some(&next),
                setup,
            );
        }
    }

    let next_parameters = next;
    let next = CameraActuators {
        parameters: next_parameters.clone(),
        ..base
    };

    let mut script = ScriptAction::Keep;
    let mut reload_scripts = false;
    let mut after_reload = Vec::new();
    if motorized {
        let overwrite = overwrite || turning_motorized;
        let contents = script::generate_checked_lua_script(&script_cameras(
            snapshot,
            camera_uuid,
            Some(&next),
        ))?;
        if overwrite || !script_installed(snapshot, &contents) {
            script = ScriptAction::Write(contents);
        }
        reload_scripts = overwrite || script != ScriptAction::Keep;
        reboot |=
            planner.write_if_needed("SCR_ENABLE".to_string(), ParamType::REAL32(1.0), overwrite);
        if !reboot {
            // The script parameters of a new camera id were never written.
            after_reload =
                script::param_writes(current, &next_parameters, reload_scripts || camera_moved);
        }
    } else if releasing {
        script = script_without(snapshot, camera_uuid)?;
        // The running script only changes once the autopilot reboots.
        reboot |= script != ScriptAction::Keep;
    }

    Ok(ApplyPlan {
        writes: planner.writes,
        releases: planner.releases,
        script,
        reload_scripts: reload_scripts && !reboot,
        reboot,
        after_reload,
        next,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAMERA: Uuid = Uuid::from_u128(0x091a_0001);
    const OTHER: Uuid = Uuid::from_u128(0x091a_0002);

    /// No camera set up yet, on an ArduSub 4.8 or newer.
    fn empty() -> Snapshot {
        Snapshot {
            script_path: "/scripts/br4kcam.lua".to_string(),
            setup: Some(Setup::PerCamera),
            ..Default::default()
        }
    }

    fn config_for(actuators: &CameraActuators) -> api::ActuatorsConfig {
        api::ActuatorsConfig::from(actuators)
    }

    fn names(writes: &[PlannedWrite]) -> Vec<&str> {
        writes.iter().map(|write| write.name.as_str()).collect()
    }

    /// The value the autopilot holds for `name` in `snapshot`.
    fn held(snapshot: &Snapshot, name: &str) -> Option<f32> {
        snapshot
            .autopilot
            .get(name)
            .and_then(|parameter| param_display_value(&parameter.value))
    }

    /// The snapshot after the autopilot took every write of `plan` and the setup was saved.
    fn applied(mut snapshot: Snapshot, camera_uuid: Uuid, plan: &ApplyPlan) -> Snapshot {
        let after_reboot = script::param_writes(&plan.next.parameters, &plan.next.parameters, true);
        for write in plan
            .writes
            .iter()
            .chain(&plan.releases)
            .chain(&plan.after_reload)
            .chain(&after_reboot)
        {
            snapshot.autopilot.insert(
                write.name.clone(),
                Parameter {
                    name: write.name.clone(),
                    value: write.value,
                },
            );
        }
        if let ScriptAction::Write(contents) = &plan.script {
            snapshot.installed_script = Some(contents.clone());
        }
        snapshot.saved.insert(camera_uuid, plan.next.clone());
        snapshot
    }

    /// A snapshot where `camera_uuid` has the default setup applied.
    fn set_up(snapshot: Snapshot, camera_uuid: Uuid) -> Snapshot {
        set_up_lens(snapshot, camera_uuid, api::LensType::Motorized)
    }

    /// A snapshot where `camera_uuid` has the default setup for `lens` applied.
    fn set_up_lens(snapshot: Snapshot, camera_uuid: Uuid, lens: api::LensType) -> Snapshot {
        let defaults = CameraActuators::default_for(lens);
        let plan = plan_apply(&snapshot, camera_uuid, &config_for(&defaults), true).unwrap();
        applied(snapshot, camera_uuid, &plan)
    }

    fn fixed() -> api::LensType {
        api::LensType::FixedFocus
    }

    fn lens_config(lens: api::LensType) -> api::ActuatorsConfig {
        parameters(api::ActuatorsParametersConfig {
            lens: Some(lens),
            ..Default::default()
        })
    }

    fn sorted(mut names: Vec<&str>) -> Vec<&str> {
        names.sort_unstable();
        names
    }

    /// `config` over the saved setup of `camera_uuid`, applied.
    fn apply(snapshot: Snapshot, camera_uuid: Uuid, config: &api::ActuatorsConfig) -> Snapshot {
        let plan = plan_apply(&snapshot, camera_uuid, config, false).unwrap();
        applied(snapshot, camera_uuid, &plan)
    }

    fn parameters(parameters: api::ActuatorsParametersConfig) -> api::ActuatorsConfig {
        api::ActuatorsConfig {
            parameters: Some(parameters),
            ..Default::default()
        }
    }

    /// The custom channels and camera id the tests move a camera to.
    fn custom_channels() -> api::ActuatorsParametersConfig {
        api::ActuatorsParametersConfig {
            camera_id: Some(api::CameraID::CAM2),
            focus_channel: Some(api::ServoChannel::SERVO13),
            script_channel: Some(api::ServoChannel::SERVO14),
            zoom_channel: Some(api::ServoChannel::SERVO15),
            tilt_channel: Some(api::ServoChannel::SERVO9),
            ..Default::default()
        }
    }

    /// The parameters `parameters` owns, with their values.
    fn owned(parameters: &ActuatorsParameters) -> IndexMap<String, ParamType> {
        let mut owned = IndexMap::new();
        owned_parameters::push_all_expectations(parameters, Setup::PerCamera, &mut owned);
        owned
    }

    /// The second camera's default setup on servos 7, 8 and 9 and tilt 15.
    fn second_camera_config() -> api::ActuatorsConfig {
        parameters(api::ActuatorsParametersConfig {
            focus_channel: Some(api::ServoChannel::SERVO7),
            zoom_channel: Some(api::ServoChannel::SERVO8),
            script_channel: Some(api::ServoChannel::SERVO9),
            tilt_channel: Some(api::ServoChannel::SERVO15),
            ..api::ActuatorsParametersConfig::from(&CameraActuators::default().parameters)
        })
    }

    /// The two default cameras on their own channels: CAM1 on 10/11/12 and tilt 16, CAM2 on
    /// 7/8/9 and tilt 15.
    fn two_cameras() -> Snapshot {
        let snapshot = set_up(empty(), CAMERA);
        let plan = plan_apply(&snapshot, OTHER, &second_camera_config(), false).unwrap();
        assert_eq!(plan.next.parameters.camera_id, api::CameraID::CAM2);
        applied(snapshot, OTHER, &plan)
    }

    #[test]
    fn two_cameras_share_no_zoom_or_focus_function() {
        let snapshot = two_cameras();
        let mut functions = Vec::new();
        for uuid in [CAMERA, OTHER] {
            let parameters = &snapshot.saved[&uuid].parameters;
            for channel in [
                parameters.focus_channel,
                parameters.zoom_channel,
                parameters.script_channel,
            ] {
                let name = servo_param(channel, "FUNCTION");
                functions.push(held(&snapshot, &name).unwrap() as i16);
            }
        }

        // ArduPilot's one global CameraFocus (92) and CameraZoom (180) would move both cameras.
        assert!(
            !functions.contains(&92) && !functions.contains(&180),
            "{functions:?}"
        );
        assert!(
            functions
                .iter()
                .all(|function| (94..=109).contains(function))
        );
        let mut distinct = functions.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), functions.len(), "{functions:?}");
    }

    #[test]
    fn each_motorized_camera_gets_the_scripting_camera_type() {
        // Scripting backend (7): ArduPilot routes a command to the instance in param3 and keeps
        // its zoom and focus for the script. The servo backend would drive the global functions.
        let snapshot = two_cameras();
        assert_eq!(held(&snapshot, "CAM1_TYPE"), Some(7.0));
        assert_eq!(held(&snapshot, "CAM2_TYPE"), Some(7.0));
    }

    #[test]
    fn forgetting_one_camera_leaves_the_other_cameras_setup_and_script() {
        let snapshot = two_cameras();
        let (writes, script) = plan_forget(&snapshot, CAMERA).unwrap().unwrap();

        // Only CAM1's channels, camera type and tilt are released.
        let mut released = names(&writes);
        released.sort_unstable();
        assert_eq!(
            released,
            [
                "CAM1_TYPE",
                "SERVO10_FUNCTION",
                "SERVO11_FUNCTION",
                "SERVO12_FUNCTION",
                "SERVO16_FUNCTION"
            ]
        );
        // The script is rewritten for the camera that stays.
        let ScriptAction::Write(contents) = script else {
            panic!("the script must follow the camera that stays");
        };
        assert!(contents.contains("BR4KCAM2_"));
        assert!(!contents.contains("BR4KCAM1_"));
    }

    #[test]
    fn first_setup_writes_camera_focus_zoom_script_and_tilt() {
        let plan = plan_apply(
            &empty(),
            CAMERA,
            &config_for(&CameraActuators::default()),
            false,
        )
        .unwrap();

        let names = names(&plan.writes);
        for name in [
            "CAM1_TYPE",
            "SERVO10_FUNCTION",
            "SERVO11_FUNCTION",
            "SERVO12_FUNCTION",
            "SERVO16_FUNCTION",
            "MNT1_TYPE",
            "SCR_ENABLE",
        ] {
            assert!(names.contains(&name), "{name} is not written");
        }
        assert!(matches!(plan.script, ScriptAction::Write(_)));
        assert!(plan.reboot);
    }

    #[test]
    fn applied_setup_plans_nothing_more() {
        let snapshot = set_up(empty(), CAMERA);
        let saved = snapshot.saved[&CAMERA].clone();
        let plan = plan_apply(&snapshot, CAMERA, &config_for(&saved), false).unwrap();

        assert!(plan.writes.is_empty(), "{:?}", names(&plan.writes));
        assert_eq!(plan.script, ScriptAction::Keep);
        assert!(!plan.reboot);
        assert!(!plan.reload_scripts);
        assert!(plan.after_reload.is_empty());
    }

    #[test]
    fn a_partial_change_writes_only_that_parameter() {
        let snapshot = set_up(empty(), CAMERA);
        let config = parameters(api::ActuatorsParametersConfig {
            tilt_mnt_type: Some(api::MountType::Servo),
            ..Default::default()
        });
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();

        assert_eq!(names(&plan.writes), ["MNT1_TYPE"]);
        assert!(plan.reboot);
        assert_eq!(plan.next.parameters.tilt_mnt_type, api::MountType::Servo);
    }

    #[test]
    fn first_setup_on_other_channels_leaves_the_default_ones_alone() {
        let config = parameters(custom_channels());
        let plan = plan_apply(&empty(), CAMERA, &config, true).unwrap();

        // The camera never wrote the default channels or CAM1.
        for name in [
            "SERVO10_FUNCTION",
            "SERVO11_FUNCTION",
            "SERVO12_FUNCTION",
            "SERVO16_FUNCTION",
            "CAM1_TYPE",
        ] {
            assert!(!names(&plan.writes).contains(&name), "{name} is written");
        }
        for name in [
            "SERVO9_FUNCTION",
            "SERVO13_FUNCTION",
            "SERVO14_FUNCTION",
            "SERVO15_FUNCTION",
            "CAM2_TYPE",
        ] {
            assert!(names(&plan.writes).contains(&name), "{name} is not written");
        }
    }

    #[test]
    fn moving_a_channel_off_another_cameras_channel_keeps_it() {
        let snapshot = set_up(set_up(empty(), OTHER), CAMERA);
        let config = parameters(api::ActuatorsParametersConfig {
            focus_channel: Some(api::ServoChannel::SERVO13),
            ..Default::default()
        });
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();

        assert!(!names(&plan.writes).contains(&"SERVO10_FUNCTION"));
        assert!(names(&plan.writes).contains(&"SERVO13_FUNCTION"));
    }

    #[test]
    fn swapping_two_channels_sets_both() {
        let snapshot = set_up(empty(), CAMERA);
        let config = parameters(api::ActuatorsParametersConfig {
            focus_channel: Some(api::ServoChannel::SERVO11),
            zoom_channel: Some(api::ServoChannel::SERVO10),
            ..Default::default()
        });
        let snapshot = apply(snapshot, CAMERA, &config);

        let expected = owned(&snapshot.saved[&CAMERA].parameters);
        for name in ["SERVO10_FUNCTION", "SERVO11_FUNCTION"] {
            assert_eq!(
                held(&snapshot, name),
                param_display_value(&expected[name]),
                "{name}"
            );
        }
    }

    #[test]
    fn a_role_moves_onto_a_channel_already_holding_it() {
        let mut snapshot = set_up(empty(), CAMERA);
        let function = owned(&snapshot.saved[&CAMERA].parameters)["SERVO10_FUNCTION"];
        snapshot.autopilot.insert(
            "SERVO13_FUNCTION".to_string(),
            Parameter {
                name: "SERVO13_FUNCTION".to_string(),
                value: function,
            },
        );
        let config = parameters(api::ActuatorsParametersConfig {
            focus_channel: Some(api::ServoChannel::SERVO13),
            ..Default::default()
        });
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();

        assert_eq!(
            plan.next.parameters.focus_channel,
            api::ServoChannel::SERVO13
        );
        assert!(names(&plan.releases).contains(&"SERVO10_FUNCTION"));
        let names = names(&plan.writes);
        assert!(names.contains(&"SERVO13_MIN"));
    }

    #[test]
    fn default_setup_turns_off_the_custom_channels_it_moves_away_from() {
        let snapshot = apply(
            set_up(empty(), CAMERA),
            CAMERA,
            &parameters(custom_channels()),
        );

        let defaults = CameraActuators::default();
        for overwrite in [false, true] {
            let plan = plan_apply(&snapshot, CAMERA, &config_for(&defaults), overwrite).unwrap();
            let snapshot = applied(snapshot.clone(), CAMERA, &plan);

            for name in [
                "SERVO9_FUNCTION",
                "SERVO13_FUNCTION",
                "SERVO14_FUNCTION",
                "SERVO15_FUNCTION",
                "CAM2_TYPE",
            ] {
                assert_eq!(held(&snapshot, name), Some(0.0), "{name} {overwrite}");
            }
            for (name, value) in &owned(&defaults.parameters) {
                assert_eq!(
                    held(&snapshot, name),
                    param_display_value(value),
                    "{name} {overwrite}"
                );
            }
        }
    }

    /// `snapshot` with the autopilot holding `value` for `name`.
    fn holding(mut snapshot: Snapshot, name: &str, value: ParamType) -> Snapshot {
        snapshot.autopilot.insert(
            name.to_string(),
            Parameter {
                name: name.to_string(),
                value,
            },
        );
        snapshot
    }

    /// The value `writes` gives `name`, if any.
    fn written_value(writes: &[PlannedWrite], name: &str) -> Option<f32> {
        writes
            .iter()
            .rev()
            .find(|write| write.name == name)
            .and_then(|write| param_display_value(&write.value))
    }

    #[test]
    fn a_channel_another_camera_uses_is_refused() {
        let snapshot = apply(
            set_up(empty(), OTHER),
            OTHER,
            &parameters(custom_channels()),
        );
        let snapshot = set_up(snapshot, CAMERA);
        let config = parameters(api::ActuatorsParametersConfig {
            zoom_channel: Some(api::ServoChannel::SERVO13),
            ..Default::default()
        });

        let error = plan_apply(&snapshot, CAMERA, &config, false).unwrap_err();
        assert!(error.to_string().contains("SERVO13_FUNCTION"), "{error}");
    }

    #[test]
    fn a_channel_the_camera_lost_is_not_released() {
        let snapshot = holding(
            set_up(empty(), CAMERA),
            "SERVO11_FUNCTION",
            ParamType::INT16(33),
        );
        let config = parameters(api::ActuatorsParametersConfig {
            zoom_channel: Some(api::ServoChannel::SERVO13),
            ..Default::default()
        });
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();

        assert!(!names(&plan.writes).contains(&"SERVO11_FUNCTION"));
    }

    #[test]
    fn a_role_moving_to_a_channel_clears_its_reversal() {
        let snapshot = holding(
            set_up(empty(), CAMERA),
            "SERVO13_REVERSED",
            ParamType::UINT8(1),
        );
        let config = parameters(api::ActuatorsParametersConfig {
            zoom_channel: Some(api::ServoChannel::SERVO13),
            ..Default::default()
        });
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();

        assert_eq!(written_value(&plan.writes, "SERVO13_REVERSED"), Some(0.0));
        for name in ["SERVO13_MIN", "SERVO13_TRIM", "SERVO13_MAX"] {
            assert!(names(&plan.writes).contains(&name), "{name} is not written");
        }
        assert_eq!(written_value(&plan.releases, "SERVO11_FUNCTION"), Some(0.0));
    }

    #[test]
    fn a_camera_id_move_writes_the_new_mount_and_releases_the_old_camera_type() {
        let snapshot = set_up(empty(), CAMERA);
        let config = parameters(api::ActuatorsParametersConfig {
            camera_id: Some(api::CameraID::CAM2),
            ..Default::default()
        });
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();

        let names = names(&plan.writes);
        for name in ["CAM2_TYPE", "MNT2_TYPE", "MNT2_PITCH_MIN", "MNT2_PITCH_MAX"] {
            assert!(names.contains(&name), "{name} is not written");
        }
        assert_eq!(written_value(&plan.releases, "CAM1_TYPE"), Some(0.0));
        // The Script functions follow the camera id: CAM2 owns Script4 to Script6.
        for (name, function) in [
            ("SERVO10_FUNCTION", 97.0),
            ("SERVO11_FUNCTION", 98.0),
            ("SERVO12_FUNCTION", 99.0),
        ] {
            assert_eq!(written_value(&plan.writes, name), Some(function), "{name}");
        }
        assert!(plan.reboot);
    }

    #[test]
    fn a_second_camera_on_its_own_tilt_servo_gets_its_own_mount() {
        // Both default to CAM1; the second one tilts on another servo.
        let snapshot = set_up(empty(), OTHER);
        let config = parameters(api::ActuatorsParametersConfig {
            focus_channel: Some(api::ServoChannel::SERVO7),
            zoom_channel: Some(api::ServoChannel::SERVO8),
            script_channel: Some(api::ServoChannel::SERVO9),
            tilt_channel: Some(api::ServoChannel::SERVO15),
            ..api::ActuatorsParametersConfig::from(&CameraActuators::default().parameters)
        });
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();
        assert_eq!(plan.next.parameters.camera_id, api::CameraID::CAM2);
        assert_eq!(written_value(&plan.writes, "SERVO15_FUNCTION"), Some(13.0));
        assert!(names(&plan.writes).contains(&"MNT2_TYPE"));

        // A camera saved on the other camera's id before ids were assigned moves on its next apply.
        let mut collided = applied(snapshot, CAMERA, &plan);
        collided
            .saved
            .get_mut(&CAMERA)
            .unwrap()
            .parameters
            .camera_id = api::CameraID::CAM1;
        collided
            .autopilot
            .get_mut("SERVO15_FUNCTION")
            .unwrap()
            .value = ParamType::INT16(7);
        let plan = plan_apply(&collided, CAMERA, &config, false).unwrap();
        assert_eq!(plan.next.parameters.camera_id, api::CameraID::CAM2);
        assert_eq!(written_value(&plan.writes, "SERVO15_FUNCTION"), Some(13.0));
    }

    #[test]
    fn rewriting_the_mount_type_the_autopilot_holds_does_not_reboot() {
        let snapshot = set_up(empty(), CAMERA);
        let parameters = &snapshot.saved[&CAMERA].parameters;
        let mut planner = Planner::new(&snapshot.autopilot);
        let reboot = tilt::plan(
            &mut planner,
            &Change {
                current: parameters,
                next: parameters,
                incoming: &api::ActuatorsParametersConfig::default(),
                overwrite: false,
                camera_moved: true,
                setup: Setup::PerCamera,
            },
        );

        assert!(names(&planner.writes).contains(&"MNT1_TYPE"));
        assert!(!reboot);
    }

    #[test]
    fn a_pending_reboot_is_planned_again() {
        let mut snapshot = set_up(empty(), CAMERA);
        snapshot.reboot_pending = true;
        let saved = snapshot.saved[&CAMERA].clone();
        let plan = plan_apply(&snapshot, CAMERA, &config_for(&saved), false).unwrap();

        assert!(plan.writes.is_empty(), "{:?}", names(&plan.writes));
        assert!(plan.reboot);
    }

    #[test]
    fn forgetting_a_camera_releases_only_what_it_still_holds() {
        let snapshot = holding(
            set_up(empty(), CAMERA),
            "SERVO11_FUNCTION",
            ParamType::INT16(33),
        );
        let (writes, _) = plan_forget(&snapshot, CAMERA).unwrap().unwrap();

        let mut names = names(&writes);
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "CAM1_TYPE",
                "SERVO10_FUNCTION",
                "SERVO12_FUNCTION",
                "SERVO16_FUNCTION"
            ]
        );
        assert!(plan_forget(&snapshot, OTHER).unwrap().is_none());
    }

    #[test]
    fn forgetting_a_camera_leaves_what_another_camera_uses() {
        let snapshot = set_up(set_up(empty(), OTHER), CAMERA);

        let (writes, script) = plan_forget(&snapshot, CAMERA).unwrap().unwrap();
        assert!(writes.is_empty());
        // The installed script already serves the other camera.
        assert_eq!(script, ScriptAction::Keep);
    }

    #[test]
    fn a_forced_reset_keeps_the_calibration_points() {
        let mut snapshot = set_up(empty(), CAMERA);
        let saved = snapshot.saved.get_mut(&CAMERA).unwrap();
        saved.closest_points.0.reverse();
        let points = saved.closest_points.clone();
        let plan = plan_apply(
            &snapshot,
            CAMERA,
            &config_for(&CameraActuators::default()),
            true,
        )
        .unwrap();

        assert_eq!(plan.next.closest_points, points);
    }

    #[test]
    fn fixed_focus_first_setup_writes_only_tilt() {
        let defaults = CameraActuators::default_for(fixed());
        let plan = plan_apply(&empty(), CAMERA, &config_for(&defaults), true).unwrap();

        assert!(plan.releases.is_empty());
        assert!(!plan.writes.is_empty());
        for name in names(&plan.writes) {
            assert!(
                name.starts_with("SERVO16_") || name.starts_with("MNT1_"),
                "{name} is not a tilt parameter"
            );
        }
        assert_eq!(plan.script, ScriptAction::Keep);
        assert!(plan.reboot);
        assert!(plan.after_reload.is_empty());
        assert_eq!(plan.next.parameters.lens, fixed());
    }

    #[test]
    fn switching_to_fixed_focus_releases_once() {
        let snapshot = set_up(empty(), CAMERA);
        let plan = plan_apply(&snapshot, CAMERA, &lens_config(fixed()), false).unwrap();

        assert_eq!(
            sorted(names(&plan.releases)),
            [
                "CAM1_TYPE",
                "SERVO10_FUNCTION",
                "SERVO11_FUNCTION",
                "SERVO12_FUNCTION"
            ]
        );
        assert!(plan.writes.is_empty(), "{:?}", names(&plan.writes));
        assert_eq!(plan.script, ScriptAction::Delete);
        assert!(plan.reboot);
        let mut description = describe_releases(&plan, &snapshot);
        assert_eq!(
            description.pop().unwrap(),
            "Lua script br4kcam.lua: removed"
        );
        description.sort_unstable();
        assert_eq!(
            description,
            [
                "CAM1_TYPE: 7 → 0",
                "SERVO10_FUNCTION: 94 → 0",
                "SERVO11_FUNCTION: 95 → 0",
                "SERVO12_FUNCTION: 96 → 0",
            ]
        );

        let snapshot = applied(snapshot, CAMERA, &plan);
        let plan = plan_apply(&snapshot, CAMERA, &lens_config(fixed()), false).unwrap();
        assert!(plan.releases.is_empty());
        assert!(plan.writes.is_empty());
        assert_eq!(plan.script, ScriptAction::Keep);
        assert!(!plan.reboot);
    }

    #[test]
    fn release_keeps_what_another_motorized_camera_uses() {
        let snapshot = set_up(empty(), OTHER);
        let snapshot = set_up(snapshot, CAMERA);
        let plan = plan_apply(&snapshot, CAMERA, &lens_config(fixed()), false).unwrap();

        // Both cameras use the default channels and CAM1, so nothing is released and the
        // installed script already serves the other camera.
        assert!(plan.releases.is_empty(), "{:?}", names(&plan.releases));
        assert_eq!(plan.script, ScriptAction::Keep);
        assert!(!plan.reboot);
    }

    #[test]
    fn release_keeps_a_channel_tilt_moves_onto() {
        let snapshot = set_up(empty(), CAMERA);
        let config = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                lens: Some(fixed()),
                tilt_channel: Some(api::ServoChannel::SERVO10),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();

        assert!(!names(&plan.releases).contains(&"SERVO10_FUNCTION"));
        assert!(names(&plan.writes).contains(&"SERVO10_FUNCTION"));
        assert!(names(&plan.releases).contains(&"SERVO16_FUNCTION"));
    }

    #[test]
    fn switching_to_motorized_writes_the_whole_focus_setup_again() {
        let snapshot = set_up_lens(empty(), CAMERA, fixed());
        let plan = plan_apply(
            &snapshot,
            CAMERA,
            &lens_config(api::LensType::Motorized),
            false,
        )
        .unwrap();

        for name in [
            "CAM1_TYPE",
            "SERVO10_FUNCTION",
            "SERVO11_FUNCTION",
            "SERVO12_FUNCTION",
            "SERVO10_MIN",
            "SCR_ENABLE",
        ] {
            let write = plan
                .writes
                .iter()
                .find(|write| write.name == name)
                .unwrap_or_else(|| panic!("{name} is not written"));
            assert!(write.force, "{name} must be forced");
        }
        // Tilt did not change, so it is left alone.
        assert!(
            !names(&plan.writes)
                .iter()
                .any(|name| name.starts_with("SERVO16_"))
        );
        assert!(matches!(plan.script, ScriptAction::Write(_)));
        assert!(plan.reboot);
    }

    #[test]
    fn switching_to_motorized_on_other_channels_leaves_unwritten_ones_alone() {
        let alone = set_up_lens(empty(), CAMERA, fixed());
        let shared = set_up_lens(set_up(empty(), OTHER), CAMERA, fixed());
        let config = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                lens: Some(api::LensType::Motorized),
                camera_id: Some(api::CameraID::CAM2),
                focus_channel: Some(api::ServoChannel::SERVO13),
                script_channel: Some(api::ServoChannel::SERVO14),
                zoom_channel: Some(api::ServoChannel::SERVO15),
                // The other camera's mount pitch holds the default tilt channel.
                tilt_channel: Some(api::ServoChannel::SERVO9),
                ..Default::default()
            }),
            ..Default::default()
        };
        // The fixed-focus camera never wrote the default channels or CAM1, and the other
        // camera uses them.
        for snapshot in [alone, shared] {
            let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();
            for name in [
                "SERVO10_FUNCTION",
                "SERVO11_FUNCTION",
                "SERVO12_FUNCTION",
                "CAM1_TYPE",
            ] {
                assert!(!names(&plan.writes).contains(&name), "{name} is written");
            }
            for name in [
                "SERVO13_FUNCTION",
                "SERVO14_FUNCTION",
                "SERVO15_FUNCTION",
                "CAM2_TYPE",
            ] {
                assert!(names(&plan.writes).contains(&name), "{name} is not written");
            }
        }
    }

    #[test]
    fn default_setup_keeps_focus_on_the_channel_tilt_leaves() {
        // A fixed-focus camera with tilt on the default focus channel.
        let snapshot = set_up_lens(empty(), CAMERA, fixed());
        let config = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                tilt_channel: Some(api::ServoChannel::SERVO10),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();
        let snapshot = applied(snapshot, CAMERA, &plan);
        assert_eq!(held(&snapshot, "SERVO10_FUNCTION"), Some(7.0));

        let defaults = CameraActuators::default();
        for overwrite in [false, true] {
            let plan = plan_apply(&snapshot, CAMERA, &config_for(&defaults), overwrite).unwrap();
            let snapshot = applied(snapshot.clone(), CAMERA, &plan);

            let mut expected = IndexMap::new();
            owned_parameters::push_all_expectations(
                &defaults.parameters,
                Setup::PerCamera,
                &mut expected,
            );
            let focus = param_display_value(&expected["SERVO10_FUNCTION"]);
            assert_eq!(held(&snapshot, "SERVO10_FUNCTION"), focus, "{overwrite}");
            assert_eq!(
                held(&snapshot, "SERVO16_FUNCTION"),
                Some(7.0),
                "{overwrite}"
            );
        }
    }

    #[test]
    fn release_leaves_a_channel_repurposed_since() {
        let mut snapshot = set_up(empty(), CAMERA);
        snapshot.autopilot.insert(
            "SERVO11_FUNCTION".to_string(),
            Parameter {
                name: "SERVO11_FUNCTION".to_string(),
                value: ParamType::INT16(1),
            },
        );
        let plan = plan_apply(&snapshot, CAMERA, &lens_config(fixed()), false).unwrap();

        assert_eq!(
            sorted(names(&plan.releases)),
            ["CAM1_TYPE", "SERVO10_FUNCTION", "SERVO12_FUNCTION"]
        );
    }

    #[test]
    fn release_describes_the_script_handed_to_another_camera() {
        let snapshot = set_up(empty(), OTHER);
        let mut snapshot = set_up(snapshot, CAMERA);
        snapshot
            .saved
            .get_mut(&OTHER)
            .unwrap()
            .parameters
            .focus_margin_gain = 2.0;
        let plan = plan_apply(&snapshot, CAMERA, &lens_config(fixed()), false).unwrap();

        assert!(matches!(plan.script, ScriptAction::Write(_)));
        assert!(plan.reboot);
        // The script parameter goes back to the value the other camera gives it.
        assert_eq!(
            describe_releases(&plan, &snapshot),
            [
                "BR4KCAM1_GAIN: 1 → 2",
                "Lua script br4kcam.lua: rewritten for the other motorized cameras"
            ]
        );
    }

    #[test]
    fn a_camera_turning_fixed_focus_rewrites_the_script_for_the_other_one() {
        let snapshot = two_cameras();
        let plan = plan_apply(&snapshot, OTHER, &lens_config(fixed()), false).unwrap();

        let ScriptAction::Write(contents) = &plan.script else {
            panic!("the script must stop serving the camera that turned fixed-focus");
        };
        assert!(contents.contains("BR4KCAM1_") && !contents.contains("BR4KCAM2_"));
        // The rewritten script only runs after the autopilot rebooted.
        assert!(plan.reboot);
    }

    #[test]
    fn two_roles_on_one_channel_are_refused_unless_the_lens_does_not_use_one() {
        let snapshot = set_up(empty(), CAMERA);
        let onto_focus = |lens| {
            parameters(api::ActuatorsParametersConfig {
                lens: Some(lens),
                tilt_channel: Some(api::ServoChannel::SERVO10),
                ..Default::default()
            })
        };

        let error = plan_apply(
            &snapshot,
            CAMERA,
            &onto_focus(api::LensType::Motorized),
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("SERVO10"), "{error}");
        assert!(plan_apply(&snapshot, CAMERA, &onto_focus(fixed()), false).is_ok());
    }

    #[test]
    fn a_motorized_channel_move_previews_only_its_release() {
        let snapshot = set_up(empty(), CAMERA);
        let config = parameters(api::ActuatorsParametersConfig {
            zoom_channel: Some(api::ServoChannel::SERVO13),
            ..Default::default()
        });
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();

        assert_eq!(
            describe_releases(&plan, &snapshot),
            ["SERVO11_FUNCTION: 95 → 0"]
        );
    }

    #[test]
    fn a_forced_reset_to_fixed_focus_releases_and_removes_the_script() {
        let snapshot = set_up(empty(), CAMERA);
        let defaults = CameraActuators::default_for(fixed());
        let plan = plan_apply(&snapshot, CAMERA, &config_for(&defaults), true).unwrap();

        assert_eq!(
            sorted(names(&plan.releases)),
            [
                "CAM1_TYPE",
                "SERVO10_FUNCTION",
                "SERVO11_FUNCTION",
                "SERVO12_FUNCTION"
            ]
        );
        assert_eq!(plan.script, ScriptAction::Delete);
        assert!(!names(&plan.writes).contains(&"SCR_ENABLE"));
        assert!(plan.reboot);
    }

    #[test]
    fn forgetting_the_last_motorized_camera_removes_the_script() {
        let snapshot = set_up_lens(set_up(empty(), CAMERA), OTHER, fixed());
        let (_, script) = plan_forget(&snapshot, CAMERA).unwrap().unwrap();

        assert_eq!(script, ScriptAction::Delete);
        let (_, script) = plan_forget(&snapshot, OTHER).unwrap().unwrap();
        assert_eq!(script, ScriptAction::Keep);
    }

    #[test]
    fn forgetting_a_camera_hands_the_script_to_another_motorized_one() {
        let mut snapshot = set_up(set_up(empty(), OTHER), CAMERA);
        snapshot
            .saved
            .get_mut(&OTHER)
            .unwrap()
            .parameters
            .focus_margin_gain = 2.0;
        let (_, script) = plan_forget(&snapshot, CAMERA).unwrap().unwrap();

        let expected = script::generate_checked_lua_script(&[&snapshot.saved[&OTHER]]).unwrap();
        assert_eq!(script, ScriptAction::Write(expected));
    }

    #[test]
    fn turning_motorized_adds_the_camera_to_the_one_script() {
        let snapshot = two_cameras();
        let snapshot = apply(snapshot, OTHER, &lens_config(fixed()));
        let plan = plan_apply(
            &snapshot,
            OTHER,
            &lens_config(api::LensType::Motorized),
            false,
        )
        .unwrap();

        // One script file serves every motorized camera, so the camera turning motorized joins it.
        let expected =
            script::generate_checked_lua_script(&[&snapshot.saved[&CAMERA], &plan.next]).unwrap();
        assert!(expected.contains("BR4KCAM1_") && expected.contains("BR4KCAM2_"));
        assert_eq!(plan.script, ScriptAction::Write(expected));
        assert!(plan.reboot);
    }

    #[test]
    fn script_parameters_follow_the_reload_when_no_reboot_is_needed() {
        let snapshot = set_up(empty(), CAMERA);
        let config = parameters(api::ActuatorsParametersConfig {
            focus_margin_gain: Some(2.0),
            ..Default::default()
        });
        let plan = plan_apply(&snapshot, CAMERA, &config, false).unwrap();

        assert!(!plan.reboot);
        // The gain is baked into the script, so it is rewritten and reloaded.
        assert!(matches!(plan.script, ScriptAction::Write(_)));
        assert!(plan.reload_scripts);
        assert_eq!(
            names(&plan.after_reload),
            ["BR4KCAM1_ENABLE", "BR4KCAM1_GAIN"]
        );
    }
}
