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
    parameters::{ActuatorsParameters, ChannelFunction, ParamType, Parameter, param_display_value},
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
}

#[derive(Debug)]
pub(crate) struct ApplyPlan {
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
    /// The defaults, replacing the saved setup.
    Defaults,
}

/// Everything planning reads.
#[derive(Debug, Default, Clone)]
pub(crate) struct Snapshot {
    pub saved: IndexMap<Uuid, CameraActuators>,
    /// The autopilot's parameter cache.
    pub autopilot: IndexMap<String, Parameter>,
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
    writes: Vec<PlannedWrite>,
}

impl<'a> Planner<'a> {
    fn new(autopilot: &'a IndexMap<String, Parameter>, kept: HashSet<String>) -> Self {
        Self {
            autopilot,
            kept,
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
        Start::Defaults => CameraActuators::default(),
    };

    let incoming = config.parameters.clone().unwrap_or_default();
    let current = &base.parameters;
    let next = ActuatorsParameters::from(
        merge_struct::merge(&api::ActuatorsParametersConfig::from(current), &incoming)
            .context("Failing to merge structs")?,
    );
    // What a section moves away from is what the camera wrote: its saved setup, even when the
    // apply starts from the defaults. A camera with no saved setup wrote nothing, so it has no
    // old channel or camera type to turn off.
    let written = saved.map_or(&next, |actuators| &actuators.parameters);

    let mut kept = IndexMap::new();
    owned_parameters::push_all_expectations(&next, &mut kept);
    for (uuid, actuators) in &snapshot.saved {
        if *uuid != camera_uuid {
            owned_parameters::push_all_expectations(&actuators.parameters, &mut kept);
        }
    }
    let mut planner = Planner::new(&snapshot.autopilot, kept.into_keys().collect());
    let mut reboot = overwrite;

    if config.parameters.is_some() {
        let change = Change {
            current: written,
            next: &next,
            incoming: &incoming,
            overwrite,
        };
        reboot |= camera::plan(&mut planner, &change);
        reboot |= script::plan(&mut planner, &change);
        reboot |= focus::plan(&mut planner, &change)?;
        reboot |= zoom::plan(&mut planner, &change);
        reboot |= tilt::plan(&mut planner, &change);
    }

    let next_parameters = next;
    let next = CameraActuators {
        parameters: next_parameters.clone(),
        ..base
    };

    let contents = script::generate_checked_lua_script(&next)?;
    let is_installed = snapshot
        .installed_script
        .as_deref()
        .is_some_and(|installed| script::lua_scripts_equivalent(installed, &contents));
    let script = if overwrite || !is_installed {
        ScriptAction::Write(contents)
    } else {
        ScriptAction::Keep
    };
    let reload_scripts = overwrite || script != ScriptAction::Keep;
    reboot |= planner.write_if_needed("SCR_ENABLE".to_string(), ParamType::REAL32(1.0), overwrite);
    let after_reload = if reboot {
        Vec::new()
    } else {
        script::param_writes(current, &next_parameters, reload_scripts)
    };

    Ok(ApplyPlan {
        writes: planner.writes,
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

    fn empty() -> Snapshot {
        Snapshot::default()
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
        let defaults = CameraActuators::default();
        let plan = plan_apply(
            &snapshot,
            camera_uuid,
            &config_for(&defaults),
            true,
            Start::Defaults,
        )
        .unwrap();
        applied(snapshot, camera_uuid, &plan)
    }

    /// `config` over the saved setup of `camera_uuid`, applied.
    fn apply(snapshot: Snapshot, camera_uuid: Uuid, config: &api::ActuatorsConfig) -> Snapshot {
        let plan = plan_apply(&snapshot, camera_uuid, config, false, Start::Saved).unwrap();
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
        owned_parameters::push_all_expectations(parameters, &mut owned);
        owned
    }

    #[test]
    fn first_setup_writes_camera_focus_zoom_script_and_tilt() {
        let plan = plan_apply(
            &empty(),
            CAMERA,
            &config_for(&CameraActuators::default()),
            true,
            Start::Defaults,
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
        let plan = plan_apply(&snapshot, CAMERA, &config_for(&saved), false, Start::Saved).unwrap();

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
        let plan = plan_apply(&snapshot, CAMERA, &config, false, Start::Saved).unwrap();

        assert_eq!(names(&plan.writes), ["MNT1_TYPE"]);
        assert!(plan.reboot);
        assert_eq!(plan.next.parameters.tilt_mnt_type, api::MountType::Servo);
    }

    #[test]
    fn first_setup_on_other_channels_leaves_the_default_ones_alone() {
        let config = parameters(custom_channels());
        let plan = plan_apply(&empty(), CAMERA, &config, true, Start::Saved).unwrap();

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
        let plan = plan_apply(&snapshot, CAMERA, &config, false, Start::Saved).unwrap();

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
        let plan = plan_apply(&snapshot, CAMERA, &config, false, Start::Saved).unwrap();

        assert_eq!(
            plan.next.parameters.focus_channel,
            api::ServoChannel::SERVO13
        );
        let names = names(&plan.writes);
        assert!(names.contains(&"SERVO10_FUNCTION"));
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
        for (overwrite, start) in [(false, Start::Saved), (true, Start::Defaults)] {
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
            for (name, value) in &owned(&defaults.parameters) {
                assert_eq!(
                    held(&snapshot, name),
                    param_display_value(value),
                    "{name} {start:?}"
                );
            }
        }
    }

    #[test]
    fn script_parameters_follow_the_reload_when_no_reboot_is_needed() {
        let snapshot = set_up(empty(), CAMERA);
        let config = parameters(api::ActuatorsParametersConfig {
            focus_margin_gain: Some(2.0),
            ..Default::default()
        });
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
