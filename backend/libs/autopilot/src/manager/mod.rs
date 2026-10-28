mod calibration;
mod camera;
mod focus;
mod macros;
pub(crate) mod owned_parameters;
mod script;
mod tilt;
mod zoom;

use std::collections::HashSet;

use ::mavlink::ardupilotmega::SERVO_OUTPUT_RAW_DATA;
use anyhow::{Context, Result};
use futures::future::BoxFuture;
use indexmap::IndexMap;
use once_cell::sync::OnceCell;
use tokio::sync::RwLock;
use tracing::*;
use uuid::Uuid;

use script::ScriptHealthTracker;
use settings::MANAGER as SETTINGS_MANAGER;

use crate::{
    CameraActuators,
    api::{self, ServoChannel},
    health::ObserveSource,
    mavlink::{self, MavlinkComponent},
    parameters::ActuatorsParameters,
};

pub static MANAGER: OnceCell<RwLock<Manager>> = OnceCell::new();

/// Serializes config apply/reset/clear. Watcher never takes this.
/// Lock order: CONFIG_APPLY → MANAGER → mavlink txn.
pub static CONFIG_APPLY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Hold [`CONFIG_APPLY`] for `under_apply`. If it returns `true`, drop the mutex,
/// run `reboot`, re-acquire, then run `finalize`.
///
/// Futures are type-erased (`BoxFuture`) so composing this into instrumented
/// `handle_control` / WS spawn paths does not overflow rustc's Send recursion limit.
pub async fn reboot_outside_apply_with(
    under_apply: BoxFuture<'_, Result<bool>>,
    reboot: BoxFuture<'_, Result<()>>,
    finalize: BoxFuture<'_, Result<()>>,
) -> Result<()> {
    let mut apply = CONFIG_APPLY.lock().await;
    let needs_reboot = under_apply.await?;
    if needs_reboot {
        drop(apply);
        reboot.await?;
        apply = CONFIG_APPLY.lock().await;
        finalize.await?;
    }
    drop(apply);
    Ok(())
}

/// Hold [`CONFIG_APPLY`] for `under_apply`. If it returns `true`, drop the mutex,
/// reboot the autopilot, re-acquire, then run `finalize`.
pub async fn reboot_outside_apply(
    under_apply: BoxFuture<'_, Result<bool>>,
    finalize: BoxFuture<'_, Result<()>>,
) -> Result<()> {
    reboot_outside_apply_with(
        under_apply,
        Box::pin(async { crate::mavlink::component()?.reboot_autopilot().await }),
        finalize,
    )
    .await
}

#[cfg(test)]
mod apply_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::reboot_outside_apply_with;

    #[tokio::test]
    async fn finalize_runs_only_after_reboot_when_needed() {
        let _health_tests = crate::health::lock_health_tests().await;
        let phase = AtomicUsize::new(0);
        reboot_outside_apply_with(
            Box::pin(async {
                assert_eq!(phase.load(Ordering::SeqCst), 0);
                phase.store(1, Ordering::SeqCst);
                Ok(true)
            }),
            Box::pin(async {
                assert_eq!(phase.load(Ordering::SeqCst), 1);
                phase.store(2, Ordering::SeqCst);
                Ok(())
            }),
            Box::pin(async {
                assert_eq!(phase.load(Ordering::SeqCst), 2);
                phase.store(3, Ordering::SeqCst);
                Ok(())
            }),
        )
        .await
        .unwrap();
        assert_eq!(phase.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn finalize_skipped_when_no_reboot() {
        let _health_tests = crate::health::lock_health_tests().await;
        let finalized = AtomicUsize::new(0);
        reboot_outside_apply_with(
            Box::pin(async { Ok(false) }),
            Box::pin(async {
                panic!("reboot must not run");
            }),
            Box::pin(async {
                finalized.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        )
        .await
        .unwrap();
        assert_eq!(finalized.load(Ordering::SeqCst), 0);
    }
}

#[derive(Debug)]
pub struct Manager {
    pub autopilot_scripts_file: String,
    pub settings: State,
    pub(crate) script_health: ScriptHealthTracker,
}

#[derive(Debug)]
pub struct State {
    pub actuators: IndexMap<Uuid, CameraActuators>,
}

impl State {
    #[instrument(level = "debug")]
    pub async fn from_settings() -> Result<Self> {
        let settings = &SETTINGS_MANAGER
            .get()
            .context("Not available")?
            .read()
            .await
            .settings;

        let actuators = settings
            .get_actuators()
            .iter()
            .map(|(uuid, actuator_settings)| (*uuid, CameraActuators::from(actuator_settings)))
            .collect();

        Ok(Self { actuators })
    }
}

impl Manager {
    /// Send focus/zoom/tilt setpoints without waiting for SERVO (caller measures separately).
    ///
    /// Does not touch `MANAGER` — caller must validate the camera has actuators first.
    // DO_MOUNT_CONTROL is deprecated upstream, but it is the command ArduPilot's servo mount
    // backends honour on this firmware (verified on the DUT). GIMBAL_MANAGER_* is untested here.
    #[allow(deprecated)]
    #[instrument(level = "debug", skip(parameters))]
    pub async fn apply_state_setpoints(
        new_state: &api::ActuatorsState,
        parameters: &ActuatorsParameters,
    ) -> Result<()> {
        use ::mavlink::ardupilotmega::{
            COMMAND_LONG_DATA, CameraZoomType, MavCmd, MavMountMode, SetFocusType,
        };

        let mavlink = crate::mavlink::component()?;

        if let Some(tilt) = new_state.tilt {
            // Tilt 0 is the PWM min. Reversed servos map that to pitch max, otherwise pitch min.
            let pitch_min = parameters.tilt_mnt_pitch_min as f32;
            let pitch_max = parameters.tilt_mnt_pitch_max as f32;
            let fraction = tilt.clamp(0.0, 100.0) / 100.0;
            let pitch = if parameters.tilt_channel_reversed {
                pitch_max - fraction * (pitch_max - pitch_min)
            } else {
                pitch_min + fraction * (pitch_max - pitch_min)
            };
            mavlink
                .send_command(COMMAND_LONG_DATA {
                    target_system: 1,
                    target_component: 1,
                    command: MavCmd::MAV_CMD_DO_MOUNT_CONTROL,
                    confirmation: 0,
                    param1: pitch,
                    param7: MavMountMode::MAV_MOUNT_MODE_MAVLINK_TARGETING as u8 as f32,
                    ..Default::default()
                })
                .await
                .context("Failed sending MAV_CMD_DO_MOUNT_CONTROL command")?;
        }

        if let Some(focus) = new_state.focus {
            mavlink
                .send_command(COMMAND_LONG_DATA {
                    target_system: 1,
                    target_component: 1,
                    command: MavCmd::MAV_CMD_SET_CAMERA_FOCUS,
                    confirmation: 0,
                    param1: SetFocusType::FOCUS_TYPE_RANGE as u8 as f32,
                    param2: focus,
                    param3: 0 as f32,
                    ..Default::default()
                })
                .await
                .context("Failed sending MAV_CMD_SET_CAMERA_FOCUS command")?;
        }

        if let Some(zoom) = new_state.zoom {
            mavlink
                .send_command(COMMAND_LONG_DATA {
                    target_system: 1,
                    target_component: 1,
                    command: MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
                    confirmation: 0,
                    param1: CameraZoomType::ZOOM_TYPE_RANGE as u8 as f32,
                    param2: zoom,
                    param3: 0 as f32,
                    ..Default::default()
                })
                .await
                .context("Failed sending MAV_CMD_SET_CAMERA_ZOOM command")?;
        }

        Ok(())
    }

    /// Persist actuators settings without holding `MANAGER.write` across disk I/O.
    #[instrument(level = "debug")]
    pub async fn save_actuators_settings() -> Result<()> {
        let actuators = {
            let manager = MANAGER.get().context("Not available")?.read().await;
            manager
                .settings
                .actuators
                .iter()
                .map(|(uuid, actuator_settings)| (*uuid, actuator_settings.into()))
                .collect()
        };

        let settings = &mut SETTINGS_MANAGER
            .get()
            .context("Not available")?
            .write()
            .await
            .settings;

        *settings.get_actuators_mut() = actuators;
        settings.save().await
    }

    /// Apply config without holding `MANAGER.write` across MAVLink I/O.
    ///
    /// Caller must hold [`CONFIG_APPLY`]. Returns `true` if the autopilot must be
    /// rebooted before [`Self::finalize_config_after_reboot`] (enable/gain + save).
    #[instrument(level = "debug", skip(new_config))]
    pub async fn update_config(
        camera_uuid: &Uuid,
        new_config: &api::ActuatorsConfig,
        overwrite: bool,
    ) -> Result<bool> {
        let mut autopilot_reboot_required = overwrite;

        if let Some(parameters) = &new_config.parameters {
            let kept = Self::kept_parameters(camera_uuid, parameters).await?;

            autopilot_reboot_required |=
                Self::update_camera_parameters(camera_uuid, parameters, overwrite, &kept).await?;

            autopilot_reboot_required |=
                Self::update_script_parameters(camera_uuid, parameters, overwrite, &kept).await?;

            autopilot_reboot_required |=
                Self::update_focus_parameters(camera_uuid, parameters, overwrite, &kept).await?;

            autopilot_reboot_required |=
                Self::update_zoom_parameters(camera_uuid, parameters, overwrite, &kept).await?;

            autopilot_reboot_required |=
                Self::update_tilt_parameters(camera_uuid, parameters, overwrite, &kept).await?;
        }

        let mut reload_script = overwrite;

        if let Some(points) = &new_config.closest_points {
            reload_script |= Self::update_closest_points(camera_uuid, points, overwrite).await?;
        }
        if let Some(points) = &new_config.furthest_points {
            reload_script |= Self::update_furthest_points(camera_uuid, points, overwrite).await?;
        }

        // File write only — disk settings save is deferred to finalize / no-reboot path.
        reload_script |= Self::export_script(camera_uuid, overwrite).await?;

        autopilot_reboot_required |= crate::mavlink::component()?
            .enable_lua_script(overwrite)
            .await?;

        if reload_script && !autopilot_reboot_required {
            crate::mavlink::component()?
                .reload_lua_scripts(overwrite)
                .await?;
        }

        if autopilot_reboot_required {
            // Caller drops CONFIG_APPLY, reboots, then calls finalize_config_after_reboot.
            return Ok(true);
        }

        let force_script_params = reload_script || overwrite;
        if let Some(parameters) = &new_config.parameters {
            Self::update_script_enable(camera_uuid, parameters, force_script_params).await?;
            Self::update_script_gain(camera_uuid, parameters, force_script_params).await?;
        }

        Self::save_actuators_settings().await?;
        owned_parameters::rebuild().await;
        owned_parameters::reevaluate_after_apply().await;
        Ok(false)
    }

    /// Parameters the camera's setup once `parameters` apply, or another camera's setup,
    /// uses. A channel or camera id the camera moves away from is not turned off when it is
    /// one of them, e.g. on a swap of two channels or a channel shared with another camera.
    async fn kept_parameters(
        camera_uuid: &Uuid,
        parameters: &api::ActuatorsParametersConfig,
    ) -> Result<HashSet<String>> {
        let manager = MANAGER.get().context("Not available")?.read().await;
        let current = manager
            .settings
            .actuators
            .get(camera_uuid)
            .map(|actuators| actuators.parameters.clone())
            .unwrap_or_default();
        let next = ActuatorsParameters::from(
            merge_struct::merge(&api::ActuatorsParametersConfig::from(&current), parameters)
                .context("Failing to merge structs")?,
        );

        let mut kept = IndexMap::new();
        owned_parameters::push_all_expectations(&next, &mut kept);
        for (uuid, actuators) in &manager.settings.actuators {
            if uuid != camera_uuid {
                owned_parameters::push_all_expectations(&actuators.parameters, &mut kept);
            }
        }
        Ok(kept.into_keys().collect())
    }

    /// Post-reboot enable/gain push and settings save. Caller must hold [`CONFIG_APPLY`].
    #[instrument(level = "debug", skip(parameters))]
    pub async fn finalize_config_after_reboot(
        camera_uuid: &Uuid,
        parameters: Option<&api::ActuatorsParametersConfig>,
    ) -> Result<()> {
        if let Some(parameters) = parameters {
            Self::update_script_enable(camera_uuid, parameters, true).await?;
            Self::update_script_gain(camera_uuid, parameters, true).await?;
        }
        Self::save_actuators_settings().await?;
        owned_parameters::rebuild().await;
        owned_parameters::reevaluate_after_apply().await;
        Ok(())
    }

    #[instrument(level = "debug")]
    pub async fn reset_config(camera_uuid: &Uuid) -> Result<bool> {
        let actuators = CameraActuators::default();
        let config = api::ActuatorsConfig::from(&actuators);

        {
            let mut manager = MANAGER.get().context("Not available")?.write().await;
            manager.settings.actuators.insert(*camera_uuid, actuators);
        }

        Self::update_config(camera_uuid, &config, true).await
    }
}

/// Constructs our manager, Should be done inside main
#[instrument(level = "debug")]
pub async fn init(
    autopilot_scripts_file: String,
    mavlink_address: String,
    mavlink_system_id: u8,
    mavlink_component_id: u8,
) -> Result<()> {
    let settings = State::from_settings().await?;

    // Publish settings before MAVLink param sync so GetActuatorsConfig works while
    // try_new is still downloading parameters (can take tens of seconds).
    if let Some(manager) = MANAGER.get() {
        let _apply = CONFIG_APPLY.lock().await;
        let mut guard = manager.write().await;
        guard.autopilot_scripts_file = autopilot_scripts_file.clone();
        guard.settings = settings;
    } else {
        MANAGER.get_or_init(|| {
            RwLock::new(Manager {
                autopilot_scripts_file: autopilot_scripts_file.clone(),
                settings,
                script_health: ScriptHealthTracker::default(),
            })
        });
    }

    if let Ok(component) = mavlink::component() {
        owned_parameters::rebuild().await;
        let cache = component.inner.parameters.read().await;
        owned_parameters::observe_cached(&cache, ObserveSource::BulkSync);
        crate::health::refresh_lua_script_status().await;
        return Ok(());
    }

    let mavlink =
        MavlinkComponent::try_new(mavlink_address, mavlink_system_id, mavlink_component_id).await?;
    mavlink::init_component(mavlink)?;

    crate::actuators_watch::start();

    crate::health::refresh_lua_script_status().await;

    Ok(())
}

#[instrument(level = "debug")]
pub async fn clear_saved_settings() -> Result<()> {
    let mut apply = CONFIG_APPLY.lock().await;

    settings::clear().await?;

    let path = {
        let manager = MANAGER.get().context("Not available")?.read().await;
        manager.autopilot_scripts_file.clone()
    };

    Manager::delete_script_file(&path).await?;
    let needs_reboot = Manager::disable_or_reload_lua().await?;

    if needs_reboot {
        drop(apply);
        crate::mavlink::component()?.reboot_autopilot().await?;
        apply = CONFIG_APPLY.lock().await;
    }

    // Post-conditions always (reload Done path and post-reboot): never await under write.
    let settings = State::from_settings().await?;
    {
        let mut guard = MANAGER.get().context("Not available")?.write().await;
        guard.script_health = ScriptHealthTracker::default();
        guard.settings = settings;
    }
    drop(apply);

    owned_parameters::rebuild().await;
    if let Ok(component) = mavlink::component() {
        let cache = component.inner.parameters.read().await;
        owned_parameters::observe_cached(&cache, ObserveSource::BulkSync);
    }

    Ok(())
}

pub(super) fn get_output_raw_from_channel(
    data: &SERVO_OUTPUT_RAW_DATA,
    channel: ServoChannel,
) -> Option<u16> {
    match channel {
        ServoChannel::SERVO1 => Some(data.servo1_raw),
        ServoChannel::SERVO2 => Some(data.servo2_raw),
        ServoChannel::SERVO3 => Some(data.servo3_raw),
        ServoChannel::SERVO4 => Some(data.servo4_raw),
        ServoChannel::SERVO5 => Some(data.servo5_raw),
        ServoChannel::SERVO6 => Some(data.servo6_raw),
        ServoChannel::SERVO7 => Some(data.servo7_raw),
        ServoChannel::SERVO8 => Some(data.servo8_raw),
        ServoChannel::SERVO9 => Some(data.servo9_raw),
        ServoChannel::SERVO10 => Some(data.servo10_raw),
        ServoChannel::SERVO11 => Some(data.servo11_raw),
        ServoChannel::SERVO12 => Some(data.servo12_raw),
        ServoChannel::SERVO13 => Some(data.servo13_raw),
        ServoChannel::SERVO14 => Some(data.servo14_raw),
        ServoChannel::SERVO15 => Some(data.servo15_raw),
        ServoChannel::SERVO16 => Some(data.servo16_raw),
        _ => None,
    }
}

fn percentage_within_range(value: u16, min: u16, max: u16) -> f32 {
    if max == min {
        return 0.0;
    }
    let clamped = value.clamp(min, max);
    (100.0 * ((clamped - min) as f32 / (max - min) as f32)).round()
}

/// Builds an [`api::ActuatorsState`] from a raw `SERVO_OUTPUT_RAW` sample.
///
/// When `enable_focus_and_zoom_correlation` is set, focus is read from the script
/// channel instead of the dedicated focus channel. Each axis is `None` when its
/// channel is unmapped.
pub(crate) fn actuators_state_from_servo(
    actuators: &CameraActuators,
    servo_output_raw: &SERVO_OUTPUT_RAW_DATA,
) -> api::ActuatorsState {
    let focus = {
        let (channel, min, max) = if actuators.parameters.enable_focus_and_zoom_correlation {
            (
                actuators.parameters.script_channel,
                actuators.parameters.script_channel_min,
                actuators.parameters.script_channel_max,
            )
        } else {
            (
                actuators.parameters.focus_channel,
                actuators.parameters.focus_channel_min,
                actuators.parameters.focus_channel_max,
            )
        };

        get_output_raw_from_channel(servo_output_raw, channel)
            .map(|value| percentage_within_range(value, min, max))
    };

    let zoom = {
        let channel = actuators.parameters.zoom_channel;
        let min = actuators.parameters.zoom_channel_min;
        let max = actuators.parameters.zoom_channel_max;

        get_output_raw_from_channel(servo_output_raw, channel)
            .map(|value| percentage_within_range(value, min, max))
    };

    let tilt = {
        let channel = actuators.parameters.tilt_channel;
        let min = actuators.parameters.tilt_channel_min;
        let max = actuators.parameters.tilt_channel_max;

        get_output_raw_from_channel(servo_output_raw, channel)
            .map(|value| percentage_within_range(value, min, max))
    };

    api::ActuatorsState { focus, zoom, tilt }
}
