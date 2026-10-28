//! Hardware setup planning.
//!
//! An apply is decided here from a [`Snapshot`] of the saved setups, the autopilot parameters
//! and the installed script, without touching any of them, so it can be tested and previewed
//! without an autopilot. [`super::Manager::execute`] then carries the [`ApplyPlan`] out.

use std::collections::HashSet;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use uuid::Uuid;

use super::{MANAGER, camera, focus, owned_parameters, script, tilt, zoom};
use crate::{
    CameraActuators, api,
    parameters::{
        ActuatorsParameters, CameraType, ChannelFunction, ParamType, Parameter, param_display_value,
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
    /// Writes that turn off what a motorized setup used, now that the lens is fixed-focus.
    pub releases: Vec<PlannedWrite>,
    /// Hardware setup writes, in order.
    pub writes: Vec<PlannedWrite>,
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

/// What an apply starts from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Start {
    /// The saved setup, or the defaults for a camera that has none.
    Saved,
    /// The defaults for a lens, replacing the saved setup.
    Defaults(api::LensType),
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
        let autopilot = match crate::mavlink::component() {
            Ok(component) => component.inner.parameters.read().await.clone(),
            Err(_) => IndexMap::new(),
        };
        let installed_script = tokio::fs::read_to_string(&script_path).await.ok();

        Ok(Self {
            saved,
            autopilot,
            script_path,
            installed_script,
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
}

pub(super) struct Planner<'a> {
    autopilot: &'a IndexMap<String, Parameter>,
    /// Parameters the next setup or another camera owns, which are never turned off.
    kept: HashSet<String>,
    releases: Vec<PlannedWrite>,
    writes: Vec<PlannedWrite>,
}

impl<'a> Planner<'a> {
    fn new(autopilot: &'a IndexMap<String, Parameter>, kept: HashSet<String>) -> Self {
        Self {
            autopilot,
            kept,
            releases: Vec::new(),
            writes: Vec::new(),
        }
    }

    /// Whether the autopilot is known to hold `value` for `name`.
    fn autopilot_holds(&self, name: &str, value: &ParamType) -> bool {
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

    /// Turns `name` off to `value`, unless the next setup or another camera owns it. Returns
    /// whether it is written.
    pub(super) fn turn_off(&mut self, name: String, value: ParamType) -> bool {
        !self.kept.contains(&name) && self.write_if_needed(name, value, false)
    }

    /// Moves a channel role from `old` to `new`: the old channel's function is disabled and
    /// `new` gets `function`. Returns whether a function is written.
    pub(super) fn channel_function(
        &mut self,
        old: api::ServoChannel,
        new: api::ServoChannel,
        function: ChannelFunction,
        overwrite: bool,
    ) -> bool {
        let mut written = false;
        if old != new {
            written |= self.turn_off(
                servo_param(old, "FUNCTION"),
                ParamType::INT16(ChannelFunction::Disabled as i16),
            );
        }
        written |= self.write_if_needed(
            servo_param(new, "FUNCTION"),
            ParamType::INT16(function as i16),
            overwrite,
        );
        written
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

    /// Releases what the `previous` motorized setup wrote that a fixed-focus lens does not
    /// use: its focus, script and zoom channel functions and its camera type. Returns whether
    /// anything is released.
    ///
    /// Parameters the next setup or another camera owns (e.g. a tilt channel moved onto the
    /// old focus channel), and parameters that no longer hold what the setup wrote (e.g. a
    /// channel repurposed since), are left alone. Fails when the autopilot value of one to
    /// release is not known, as the release only happens once.
    fn release_motorized(&mut self, previous: &ActuatorsParameters) -> Result<bool> {
        let mut written = IndexMap::new();
        owned_parameters::push_all_expectations(previous, &mut written);

        let disabled = ParamType::INT16(ChannelFunction::Disabled as i16);
        let candidates = [
            (servo_param(previous.focus_channel, "FUNCTION"), disabled),
            (servo_param(previous.script_channel, "FUNCTION"), disabled),
            (servo_param(previous.zoom_channel, "FUNCTION"), disabled),
            (
                format!("CAM{}_TYPE", previous.camera_id as u8),
                ParamType::UINT8(CameraType::None as u8),
            ),
        ];

        let candidates: Vec<_> = candidates
            .into_iter()
            .filter(|(name, _)| !self.kept.contains(name))
            .collect();
        let unknown: Vec<&str> = candidates
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| !self.autopilot.contains_key(*name))
            .collect();
        if !unknown.is_empty() {
            anyhow::bail!(
                "The autopilot values of {} are not loaded yet, so the parameters the motorized setup used cannot be released. Try again once the autopilot parameters are loaded.",
                unknown.join(", ")
            );
        }

        let mut released = false;
        for (name, value) in candidates {
            let still_written = written
                .get(&name)
                .is_some_and(|expected| self.autopilot_holds(&name, expected));
            let pending = self.releases.iter().any(|release| release.name == name);
            if still_written && !pending {
                self.releases.push(PlannedWrite {
                    name,
                    value,
                    force: false,
                });
                released = true;
            }
        }
        Ok(released)
    }
}

pub(super) fn servo_param(channel: api::ServoChannel, suffix: &str) -> String {
    format!("SERVO{}_{suffix}", channel as u8)
}

/// Plans applying `config` to `camera_uuid`. With `overwrite` every value is written even
/// when the autopilot already holds it.
pub(crate) fn plan_apply(
    snapshot: &Snapshot,
    camera_uuid: Uuid,
    config: &api::ActuatorsConfig,
    overwrite: bool,
    start: Start,
) -> Result<ApplyPlan> {
    let saved = snapshot.saved.get(&camera_uuid);
    let base = match start {
        Start::Saved => saved.cloned().unwrap_or_default(),
        Start::Defaults(lens) => CameraActuators::default_for(lens),
    };
    let previous = saved.map(|actuators| &actuators.parameters);
    let others: Vec<&CameraActuators> = snapshot
        .saved
        .iter()
        .filter(|(uuid, _)| **uuid != camera_uuid)
        .map(|(_, actuators)| actuators)
        .collect();

    let incoming = config.parameters.clone().unwrap_or_default();
    let current = &base.parameters;
    let next = ActuatorsParameters::from(
        merge_struct::merge(&api::ActuatorsParametersConfig::from(current), &incoming)
            .context("Failing to merge structs")?,
    );

    let motorized = next.has_motorized_lens();
    let was_motorized = previous.map(ActuatorsParameters::has_motorized_lens);
    let releasing = was_motorized == Some(true) && !motorized;
    // A camera turning motorized never had its focus, zoom and script setup written, or had
    // it released, so all of it is written again.
    let overwrite_motorized = overwrite || was_motorized == Some(false);
    // What a section moves away from is what the camera wrote: its saved setup, even when the
    // apply starts from the defaults. A camera with no saved setup wrote nothing, and a
    // fixed-focus one wrote no focus, zoom or script setup, so they have no old channel or
    // camera type to turn off.
    let written_motorized = match previous {
        Some(previous) if previous.has_motorized_lens() => previous,
        _ => &next,
    };
    let written_tilt = previous.unwrap_or(&next);

    let mut kept = IndexMap::new();
    owned_parameters::push_all_expectations(&next, &mut kept);
    for other in &others {
        owned_parameters::push_all_expectations(&other.parameters, &mut kept);
    }
    let mut planner = Planner::new(&snapshot.autopilot, kept.into_keys().collect());
    let mut reboot = overwrite;

    if let (Some(previous), true) = (previous, releasing) {
        reboot |= planner.release_motorized(previous)?;
    }

    if config.parameters.is_some() {
        if motorized {
            // Written again in full, the focus setup is asked for in full.
            let incoming_motorized = if overwrite_motorized {
                api::ActuatorsParametersConfig::from(&next)
            } else {
                incoming.clone()
            };
            let change = Change {
                current: written_motorized,
                next: &next,
                incoming: &incoming_motorized,
                overwrite: overwrite_motorized,
            };
            reboot |= camera::plan(&mut planner, &change);
            reboot |= script::plan(&mut planner, &change);
            reboot |= focus::plan(&mut planner, &change)?;
            reboot |= zoom::plan(&mut planner, &change);
        }
        let change = Change {
            current: written_tilt,
            next: &next,
            incoming: &incoming,
            overwrite,
        };
        reboot |= tilt::plan(&mut planner, &change);
    }

    let installed = snapshot.installed_script.as_deref();
    let is_installed = |contents: &str| {
        installed.is_some_and(|installed| script::lua_scripts_equivalent(installed, contents))
    };
    let mut script = ScriptAction::Keep;
    let mut reload_scripts = false;
    let mut after_reload = Vec::new();
    let next_parameters = next;
    let next = CameraActuators {
        parameters: next_parameters.clone(),
        ..base
    };

    if motorized {
        let contents = script::generate_checked_lua_script(&next)?;
        if overwrite_motorized || !is_installed(&contents) {
            script = ScriptAction::Write(contents);
        }
        reload_scripts = overwrite_motorized || script != ScriptAction::Keep;
        reboot |= planner.write_if_needed(
            "SCR_ENABLE".to_string(),
            ParamType::REAL32(1.0),
            overwrite_motorized,
        );
        if !reboot {
            after_reload = script::param_writes(current, &next_parameters, reload_scripts);
        }
    } else if releasing {
        // The script follows another motorized camera, preferably one it already serves, or
        // goes away with the last one.
        let motorized_others: Vec<&CameraActuators> = others
            .iter()
            .copied()
            .filter(|actuators| actuators.parameters.has_motorized_lens())
            .collect();
        let mut serves_another = false;
        for other in &motorized_others {
            if script_installed_for(snapshot, other)? {
                serves_another = true;
                break;
            }
        }
        script = if serves_another {
            ScriptAction::Keep
        } else if let Some(first) = motorized_others.first() {
            ScriptAction::Write(script::generate_checked_lua_script(first)?)
        } else if installed.is_some() {
            ScriptAction::Delete
        } else {
            ScriptAction::Keep
        };
        // The running script only changes once the autopilot reboots.
        reboot |= script != ScriptAction::Keep;
    }

    Ok(ApplyPlan {
        releases: planner.releases,
        writes: planner.writes,
        script,
        reload_scripts: reload_scripts && !reboot,
        reboot,
        after_reload,
        next,
    })
}

/// Whether the installed script is the one generated for `camera`.
fn script_installed_for(snapshot: &Snapshot, camera: &CameraActuators) -> Result<bool> {
    let Some(installed) = snapshot.installed_script.as_deref() else {
        return Ok(false);
    };
    let contents = script::generate_checked_lua_script(camera)?;
    Ok(script::lua_scripts_equivalent(installed, &contents))
}

/// The motorized camera the installed script serves, `preferred` first: the one whose
/// script parameters are written once the script runs.
pub(crate) fn script_camera(snapshot: &Snapshot, preferred: Uuid) -> Result<Option<Uuid>> {
    let preferred_first = snapshot.saved.get_key_value(&preferred).into_iter().chain(
        snapshot
            .saved
            .iter()
            .filter(|(uuid, _)| **uuid != preferred),
    );
    for (uuid, camera) in preferred_first {
        if camera.parameters.has_motorized_lens() && script_installed_for(snapshot, camera)? {
            return Ok(Some(*uuid));
        }
    }
    Ok(None)
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

    let file_name = || {
        std::path::Path::new(&snapshot.script_path)
            .file_name()
            .map_or_else(
                || snapshot.script_path.clone(),
                |name| name.to_string_lossy().into_owned(),
            )
    };
    match &plan.script {
        ScriptAction::Delete => lines.push(format!("Lua script {}: removed", file_name())),
        ScriptAction::Write(_) if !plan.next.parameters.has_motorized_lens() => {
            lines.push(format!(
                "Lua script {}: rewritten for the other motorized cameras",
                file_name()
            ))
        }
        _ => {}
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAMERA: Uuid = Uuid::from_u128(0x091a_0001);
    const OTHER: Uuid = Uuid::from_u128(0x091a_0002);

    fn empty() -> Snapshot {
        Snapshot {
            script_path: "/scripts/br4kcam.lua".to_string(),
            ..Default::default()
        }
    }

    fn fixed() -> api::LensType {
        api::LensType::FixedFocus
    }

    fn config_for(actuators: &CameraActuators) -> api::ActuatorsConfig {
        api::ActuatorsConfig::from(actuators)
    }

    fn lens_config(lens: api::LensType) -> api::ActuatorsConfig {
        api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                lens: Some(lens),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn names(writes: &[PlannedWrite]) -> Vec<&str> {
        writes.iter().map(|write| write.name.as_str()).collect()
    }

    /// The snapshot after the autopilot took every write of `plan` and the setup was saved.
    fn applied(mut snapshot: Snapshot, camera_uuid: Uuid, plan: &ApplyPlan) -> Snapshot {
        let after_reboot = script::param_writes(&plan.next.parameters, &plan.next.parameters, true);
        for write in plan
            .releases
            .iter()
            .chain(&plan.writes)
            .chain(&plan.after_reload)
            .chain(
                after_reboot
                    .iter()
                    .filter(|_| plan.next.parameters.has_motorized_lens()),
            )
        {
            snapshot.autopilot.insert(
                write.name.clone(),
                Parameter {
                    name: write.name.clone(),
                    value: write.value,
                },
            );
        }
        match &plan.script {
            ScriptAction::Keep => {}
            ScriptAction::Write(contents) => snapshot.installed_script = Some(contents.clone()),
            ScriptAction::Delete => snapshot.installed_script = None,
        }
        snapshot.saved.insert(camera_uuid, plan.next.clone());
        snapshot
    }

    /// A snapshot where `camera_uuid` has the default setup for `lens` applied.
    fn set_up(snapshot: Snapshot, camera_uuid: Uuid, lens: api::LensType) -> Snapshot {
        let defaults = CameraActuators::default_for(lens);
        let plan = plan_apply(
            &snapshot,
            camera_uuid,
            &config_for(&defaults),
            true,
            Start::Defaults(lens),
        )
        .unwrap();
        applied(snapshot, camera_uuid, &plan)
    }

    #[test]
    fn fixed_focus_first_setup_writes_only_tilt() {
        let defaults = CameraActuators::default_for(fixed());
        let plan = plan_apply(
            &empty(),
            CAMERA,
            &config_for(&defaults),
            true,
            Start::Defaults(fixed()),
        )
        .unwrap();

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
    fn motorized_first_setup_writes_focus_zoom_script_and_tilt() {
        let plan = plan_apply(
            &empty(),
            CAMERA,
            &config_for(&CameraActuators::default()),
            true,
            Start::Defaults(api::LensType::Motorized),
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
        for lens in [api::LensType::Motorized, fixed()] {
            let snapshot = set_up(empty(), CAMERA, lens);
            let saved = snapshot.saved[&CAMERA].clone();
            let plan =
                plan_apply(&snapshot, CAMERA, &config_for(&saved), false, Start::Saved).unwrap();

            assert!(plan.releases.is_empty(), "{lens:?}");
            assert!(
                plan.writes.is_empty(),
                "{lens:?}: {:?}",
                names(&plan.writes)
            );
            assert_eq!(plan.script, ScriptAction::Keep, "{lens:?}");
            assert!(!plan.reboot, "{lens:?}");
            assert!(!plan.reload_scripts, "{lens:?}");
            assert!(plan.after_reload.is_empty(), "{lens:?}");
        }
    }

    #[test]
    fn a_partial_change_writes_only_that_parameter() {
        let snapshot = set_up(empty(), CAMERA, api::LensType::Motorized);
        let config = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                tilt_mnt_type: Some(api::MountType::Servo),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = plan_apply(&snapshot, CAMERA, &config, false, Start::Saved).unwrap();

        assert_eq!(names(&plan.writes), ["MNT1_TYPE"]);
        assert!(plan.reboot);
        assert_eq!(plan.next.parameters.tilt_mnt_type, api::MountType::Servo);
        assert_eq!(plan.next.parameters.lens, api::LensType::Motorized);
    }

    #[test]
    fn switching_to_fixed_focus_releases_once() {
        let snapshot = set_up(empty(), CAMERA, api::LensType::Motorized);
        let plan = plan_apply(
            &snapshot,
            CAMERA,
            &lens_config(fixed()),
            false,
            Start::Saved,
        )
        .unwrap();

        assert_eq!(
            names(&plan.releases),
            [
                "SERVO10_FUNCTION",
                "SERVO12_FUNCTION",
                "SERVO11_FUNCTION",
                "CAM1_TYPE"
            ]
        );
        assert!(plan.writes.is_empty(), "{:?}", names(&plan.writes));
        assert_eq!(plan.script, ScriptAction::Delete);
        assert!(plan.reboot);
        assert_eq!(
            describe_releases(&plan, &snapshot),
            [
                "SERVO10_FUNCTION: 94 → 0",
                "SERVO12_FUNCTION: 92 → 0",
                "SERVO11_FUNCTION: 180 → 0",
                "CAM1_TYPE: 1 → 0",
                "Lua script br4kcam.lua: removed",
            ]
        );

        let snapshot = applied(snapshot, CAMERA, &plan);
        let plan = plan_apply(
            &snapshot,
            CAMERA,
            &lens_config(fixed()),
            false,
            Start::Saved,
        )
        .unwrap();
        assert!(plan.releases.is_empty());
        assert!(plan.writes.is_empty());
        assert_eq!(plan.script, ScriptAction::Keep);
        assert!(!plan.reboot);
    }

    #[test]
    fn release_keeps_what_another_motorized_camera_uses() {
        let snapshot = set_up(empty(), OTHER, api::LensType::Motorized);
        let snapshot = set_up(snapshot, CAMERA, api::LensType::Motorized);
        let plan = plan_apply(
            &snapshot,
            CAMERA,
            &lens_config(fixed()),
            false,
            Start::Saved,
        )
        .unwrap();

        // Both cameras use the default channels and CAM1, so nothing is released and the
        // installed script already serves the other camera.
        assert!(plan.releases.is_empty(), "{:?}", names(&plan.releases));
        assert_eq!(plan.script, ScriptAction::Keep);
        assert!(!plan.reboot);
    }

    #[test]
    fn release_keeps_a_channel_tilt_moves_onto() {
        let snapshot = set_up(empty(), CAMERA, api::LensType::Motorized);
        let config = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                lens: Some(fixed()),
                tilt_channel: Some(api::ServoChannel::SERVO10),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = plan_apply(&snapshot, CAMERA, &config, false, Start::Saved).unwrap();

        assert!(!names(&plan.releases).contains(&"SERVO10_FUNCTION"));
        assert!(names(&plan.writes).contains(&"SERVO10_FUNCTION"));
        assert!(names(&plan.writes).contains(&"SERVO16_FUNCTION"));
    }

    #[test]
    fn switching_to_motorized_writes_the_whole_focus_setup_again() {
        let snapshot = set_up(empty(), CAMERA, fixed());
        let plan = plan_apply(
            &snapshot,
            CAMERA,
            &lens_config(api::LensType::Motorized),
            false,
            Start::Saved,
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

    /// The value the autopilot holds for `name` in `snapshot`.
    fn held(snapshot: &Snapshot, name: &str) -> Option<f32> {
        snapshot
            .autopilot
            .get(name)
            .and_then(|parameter| param_display_value(&parameter.value))
    }

    #[test]
    fn switching_to_motorized_on_other_channels_leaves_unwritten_ones_alone() {
        let alone = set_up(empty(), CAMERA, fixed());
        let shared = set_up(
            set_up(empty(), OTHER, api::LensType::Motorized),
            CAMERA,
            fixed(),
        );
        let config = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                lens: Some(api::LensType::Motorized),
                camera_id: Some(api::CameraID::CAM2),
                focus_channel: Some(api::ServoChannel::SERVO13),
                script_channel: Some(api::ServoChannel::SERVO14),
                zoom_channel: Some(api::ServoChannel::SERVO15),
                ..Default::default()
            }),
            ..Default::default()
        };
        // The fixed-focus camera never wrote the default channels or CAM1, and the other
        // camera uses them.
        for snapshot in [alone, shared] {
            let plan = plan_apply(&snapshot, CAMERA, &config, false, Start::Saved).unwrap();
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
    fn moving_a_channel_off_another_cameras_channel_keeps_it() {
        let snapshot = set_up(empty(), OTHER, api::LensType::Motorized);
        let snapshot = set_up(snapshot, CAMERA, api::LensType::Motorized);
        let config = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                focus_channel: Some(api::ServoChannel::SERVO13),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = plan_apply(&snapshot, CAMERA, &config, false, Start::Saved).unwrap();

        assert!(!names(&plan.writes).contains(&"SERVO10_FUNCTION"));
        assert!(names(&plan.writes).contains(&"SERVO13_FUNCTION"));
    }

    #[test]
    fn default_setup_keeps_focus_on_the_channel_tilt_leaves() {
        // A fixed-focus camera with tilt on the default focus channel.
        let snapshot = set_up(empty(), CAMERA, fixed());
        let config = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                tilt_channel: Some(api::ServoChannel::SERVO10),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = plan_apply(&snapshot, CAMERA, &config, false, Start::Saved).unwrap();
        let snapshot = applied(snapshot, CAMERA, &plan);
        assert_eq!(held(&snapshot, "SERVO10_FUNCTION"), Some(7.0));

        let defaults = CameraActuators::default();
        for (overwrite, start) in [
            (false, Start::Saved),
            (true, Start::Defaults(api::LensType::Motorized)),
        ] {
            let plan =
                plan_apply(&snapshot, CAMERA, &config_for(&defaults), overwrite, start).unwrap();
            let snapshot = applied(snapshot.clone(), CAMERA, &plan);

            let mut expected = IndexMap::new();
            owned_parameters::push_all_expectations(&defaults.parameters, &mut expected);
            let focus = param_display_value(&expected["SERVO10_FUNCTION"]);
            assert_eq!(held(&snapshot, "SERVO10_FUNCTION"), focus, "{start:?}");
            assert_eq!(held(&snapshot, "SERVO16_FUNCTION"), Some(7.0), "{start:?}");
        }
    }

    #[test]
    fn default_setup_turns_off_the_custom_channels_it_moves_away_from() {
        let snapshot = set_up(empty(), CAMERA, api::LensType::Motorized);
        let custom = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                camera_id: Some(api::CameraID::CAM2),
                focus_channel: Some(api::ServoChannel::SERVO13),
                script_channel: Some(api::ServoChannel::SERVO14),
                zoom_channel: Some(api::ServoChannel::SERVO15),
                tilt_channel: Some(api::ServoChannel::SERVO9),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = plan_apply(&snapshot, CAMERA, &custom, false, Start::Saved).unwrap();
        let snapshot = applied(snapshot, CAMERA, &plan);

        let defaults = CameraActuators::default();
        for (overwrite, start) in [
            (false, Start::Saved),
            (true, Start::Defaults(api::LensType::Motorized)),
        ] {
            let plan =
                plan_apply(&snapshot, CAMERA, &config_for(&defaults), overwrite, start).unwrap();
            let snapshot = applied(snapshot.clone(), CAMERA, &plan);

            for name in [
                "SERVO9_FUNCTION",
                "SERVO13_FUNCTION",
                "SERVO14_FUNCTION",
                "SERVO15_FUNCTION",
                "CAM2_TYPE",
            ] {
                assert_eq!(held(&snapshot, name), Some(0.0), "{name} {start:?}");
            }
            let mut expected = IndexMap::new();
            owned_parameters::push_all_expectations(&defaults.parameters, &mut expected);
            for (name, value) in &expected {
                assert_eq!(
                    held(&snapshot, name),
                    param_display_value(value),
                    "{name} {start:?}"
                );
            }
        }
    }

    #[test]
    fn release_leaves_a_channel_repurposed_since() {
        let mut snapshot = set_up(empty(), CAMERA, api::LensType::Motorized);
        snapshot.autopilot.insert(
            "SERVO11_FUNCTION".to_string(),
            Parameter {
                name: "SERVO11_FUNCTION".to_string(),
                value: ParamType::INT16(1),
            },
        );
        let plan = plan_apply(
            &snapshot,
            CAMERA,
            &lens_config(fixed()),
            false,
            Start::Saved,
        )
        .unwrap();

        assert_eq!(
            names(&plan.releases),
            ["SERVO10_FUNCTION", "SERVO12_FUNCTION", "CAM1_TYPE"]
        );
    }

    #[test]
    fn release_describes_the_script_handed_to_another_camera() {
        let snapshot = set_up(empty(), OTHER, api::LensType::Motorized);
        let mut snapshot = set_up(snapshot, CAMERA, api::LensType::Motorized);
        snapshot
            .saved
            .get_mut(&OTHER)
            .unwrap()
            .parameters
            .focus_margin_gain = 2.0;
        let plan = plan_apply(
            &snapshot,
            CAMERA,
            &lens_config(fixed()),
            false,
            Start::Saved,
        )
        .unwrap();

        assert!(matches!(plan.script, ScriptAction::Write(_)));
        assert!(plan.reboot);
        assert_eq!(
            describe_releases(&plan, &snapshot),
            ["Lua script br4kcam.lua: rewritten for the other motorized cameras"]
        );

        // Its script parameters are the other camera's once the autopilot rebooted.
        let snapshot = applied(snapshot, CAMERA, &plan);
        assert_eq!(script_camera(&snapshot, CAMERA).unwrap(), Some(OTHER));
    }

    #[test]
    fn release_keeps_the_script_for_the_camera_it_already_serves() {
        const THIRD: Uuid = Uuid::from_u128(0x091a_0003);
        let mut snapshot = set_up(empty(), THIRD, api::LensType::Motorized);
        snapshot
            .saved
            .get_mut(&THIRD)
            .unwrap()
            .parameters
            .focus_margin_gain = 3.0;
        let snapshot = set_up(snapshot, OTHER, api::LensType::Motorized);
        let snapshot = set_up(snapshot, CAMERA, api::LensType::Motorized);
        let plan = plan_apply(
            &snapshot,
            CAMERA,
            &lens_config(fixed()),
            false,
            Start::Saved,
        )
        .unwrap();

        // The installed script is the one OTHER, with the same setup, needs.
        assert_eq!(plan.script, ScriptAction::Keep);
        assert!(!plan.reboot);
        let snapshot = applied(snapshot, CAMERA, &plan);
        assert_eq!(script_camera(&snapshot, CAMERA).unwrap(), Some(OTHER));
    }

    #[test]
    fn release_refuses_when_a_value_to_release_is_unknown() {
        let mut snapshot = set_up(empty(), CAMERA, api::LensType::Motorized);
        snapshot.autopilot.shift_remove("SERVO11_FUNCTION");
        let error = plan_apply(
            &snapshot,
            CAMERA,
            &lens_config(fixed()),
            false,
            Start::Saved,
        )
        .unwrap_err();

        assert!(error.to_string().contains("SERVO11_FUNCTION"), "{error}");
    }

    #[test]
    fn script_parameters_follow_the_reload_when_no_reboot_is_needed() {
        let snapshot = set_up(empty(), CAMERA, api::LensType::Motorized);
        let config = api::ActuatorsConfig {
            parameters: Some(api::ActuatorsParametersConfig {
                focus_margin_gain: Some(2.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = plan_apply(&snapshot, CAMERA, &config, false, Start::Saved).unwrap();

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
