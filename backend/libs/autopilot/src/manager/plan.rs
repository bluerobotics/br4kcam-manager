//! Hardware setup planning.
//!
//! An apply is decided here from a [`Snapshot`] of the saved setups, the autopilot parameters
//! and the installed script, without touching any of them, so it can be tested and previewed
//! without an autopilot. [`super::Manager::execute`] then carries the [`ApplyPlan`] out.

use anyhow::{Context, Result};
use indexmap::IndexMap;
use uuid::Uuid;

use super::MANAGER;
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
