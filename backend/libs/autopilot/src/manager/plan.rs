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
