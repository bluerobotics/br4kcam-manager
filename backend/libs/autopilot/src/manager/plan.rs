//! Hardware setup planning.
//!
//! An apply is decided here from a [`Snapshot`] of the saved setups, the autopilot parameters
//! and the installed script, without touching any of them, so it can be tested and previewed
//! without an autopilot. [`super::Manager::execute`] then carries the [`ApplyPlan`] out.

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

/// Everything planning reads.
#[derive(Debug, Default, Clone)]
pub(crate) struct Snapshot {
    pub saved: IndexMap<Uuid, CameraActuators>,
    /// The autopilot's parameter cache.
    pub autopilot: IndexMap<String, Parameter>,
    /// The installed Lua script, if any.
    pub installed_script: Option<String>,
    /// A reboot an earlier apply asked for has not finished yet.
    pub reboot_pending: bool,
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
            installed_script,
            reboot_pending: super::reboot_pending(),
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
}

pub(super) struct Planner<'a> {
    autopilot: &'a IndexMap<String, Parameter>,
    writes: Vec<PlannedWrite>,
}

impl<'a> Planner<'a> {
    fn new(autopilot: &'a IndexMap<String, Parameter>) -> Self {
        Self {
            autopilot,
            writes: Vec::new(),
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
) -> Vec<(Uuid, IndexMap<String, ParamType>)> {
    snapshot
        .saved
        .iter()
        .filter(|(uuid, _)| **uuid != camera_uuid)
        .map(|(uuid, actuators)| {
            let mut owned = IndexMap::new();
            owned_parameters::push_all_expectations(&actuators.parameters, &mut owned);
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
) -> Result<()> {
    let mut ours = IndexMap::new();
    owned_parameters::push_all_expectations(next, &mut ours);
    for (uuid, theirs) in other_cameras(snapshot, camera_uuid) {
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
) -> bool {
    let mut others = IndexMap::new();
    for (_, owned) in other_cameras(snapshot, camera_uuid) {
        others.extend(owned);
    }
    let mut ours = IndexMap::new();
    owned_parameters::push_all_expectations(written, &mut ours);
    let mut used = IndexMap::new();
    if let Some(next) = next {
        owned_parameters::push_all_expectations(next, &mut used);
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
        wrote |= planner.write_if_needed(name, release_value, false);
    }
    wrote
}

/// Plans forgetting `camera_uuid`: the writes that release what it holds, or `None` when it
/// has no saved setup.
pub(crate) fn plan_forget(snapshot: &Snapshot, camera_uuid: Uuid) -> Option<Vec<PlannedWrite>> {
    let saved = snapshot.saved.get(&camera_uuid)?;
    let mut planner = Planner::new(&snapshot.autopilot);
    release(&mut planner, snapshot, camera_uuid, &saved.parameters, None);
    Some(planner.writes)
}

/// Plans applying `config` to `camera_uuid`. With `overwrite`, or for a camera with no saved
/// setup, every value is written even when the autopilot already holds it.
pub(crate) fn plan_apply(
    snapshot: &Snapshot,
    camera_uuid: Uuid,
    config: &api::ActuatorsConfig,
    overwrite: bool,
) -> Result<ApplyPlan> {
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
    // Mount and script parameters are numbered by the camera id.
    let camera_moved = written.camera_id != next.camera_id;

    let mut planner = Planner::new(&snapshot.autopilot);
    // A reboot an earlier apply asked for may not have happened, and the changes it was for
    // are already on the autopilot, so nothing else would ask for it again.
    let mut reboot = overwrite || snapshot.reboot_pending;

    if config.parameters.is_some() {
        refuse_shared_channels(&next)?;
        refuse_taking_other_cameras(snapshot, camera_uuid, &next)?;
        let change = Change {
            current: written,
            next: &next,
            incoming: &incoming,
            overwrite,
            camera_moved,
        };
        reboot |= camera::plan(&mut planner, &change);
        reboot |= script::plan(&mut planner, &change);
        reboot |= focus::plan(&mut planner, &change)?;
        reboot |= zoom::plan(&mut planner, &change);
        reboot |= tilt::plan(&mut planner, &change);
        if let Some(saved) = saved {
            reboot |= release(
                &mut planner,
                snapshot,
                camera_uuid,
                &saved.parameters,
                Some(&next),
            );
        }
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
        // The script parameters of a new camera id were never written.
        script::param_writes(current, &next_parameters, reload_scripts || camera_moved)
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
        let plan = plan_apply(&snapshot, camera_uuid, &config_for(&defaults), true).unwrap();
        applied(snapshot, camera_uuid, &plan)
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
        owned_parameters::push_all_expectations(parameters, &mut owned);
        owned
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
        assert_eq!(written_value(&plan.writes, "SERVO11_FUNCTION"), Some(0.0));
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
        assert_eq!(written_value(&plan.writes, "CAM1_TYPE"), Some(0.0));
        assert!(plan.reboot);
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
        let writes = plan_forget(&snapshot, CAMERA).unwrap();

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
        assert!(plan_forget(&snapshot, OTHER).is_none());
    }

    #[test]
    fn forgetting_a_camera_leaves_what_another_camera_uses() {
        let snapshot = set_up(set_up(empty(), OTHER), CAMERA);

        assert!(plan_forget(&snapshot, CAMERA).unwrap().is_empty());
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
