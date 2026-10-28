use std::{
    collections::HashSet,
    sync::{OnceLock, RwLock},
};

use anyhow::{Result, bail};
use indexmap::IndexMap;
use uuid::Uuid;

use crate::{
    api,
    health::{self, ObserveSource},
    parameters::{ActuatorsParameters, ParamType, Parameter},
};

use super::{MANAGER, camera, focus, script, tilt, zoom};

type PerCameraExpectations = IndexMap<Uuid, IndexMap<String, ParamType>>;
type EligibilityKey = (Uuid, String);

static EXPECTATIONS: OnceLock<RwLock<PerCameraExpectations>> = OnceLock::new();
static ELIGIBLE: OnceLock<RwLock<HashSet<EligibilityKey>>> = OnceLock::new();

fn expectations() -> &'static RwLock<PerCameraExpectations> {
    EXPECTATIONS.get_or_init(|| RwLock::new(IndexMap::new()))
}

fn eligible() -> &'static RwLock<HashSet<EligibilityKey>> {
    ELIGIBLE.get_or_init(|| RwLock::new(HashSet::new()))
}

pub(crate) fn mark_eligible(camera_uuid: &Uuid, name: &str) {
    eligible()
        .write()
        .expect("owned eligibility lock")
        .insert((*camera_uuid, name.to_owned()));
}

pub(crate) fn is_eligible(camera_uuid: &Uuid, name: &str) -> bool {
    eligible()
        .read()
        .expect("owned eligibility lock")
        .contains(&(*camera_uuid, name.to_owned()))
}

pub(crate) async fn rebuild() {
    let mut merged = IndexMap::new();
    if let Some(manager) = MANAGER.get() {
        let guard = manager.read().await;
        for (camera_uuid, actuators) in &guard.settings.actuators {
            let mut per_camera = IndexMap::new();
            push_all_expectations(&actuators.parameters, &mut per_camera);
            if !per_camera.is_empty() {
                merged.insert(*camera_uuid, per_camera);
            }
        }
    }
    *expectations().write().expect("owned expectations lock") = merged;
    prune_to_expectations();
}

pub(crate) fn expectations_for_param(name: &str) -> Vec<(Uuid, ParamType)> {
    expectations()
        .read()
        .expect("owned expectations lock")
        .iter()
        .filter_map(|(camera_uuid, per_camera)| {
            per_camera
                .get(name)
                .map(|expected| (*camera_uuid, *expected))
        })
        .collect()
}

pub(crate) async fn reevaluate_after_apply() {
    let Ok(component) = crate::mavlink::component() else {
        return;
    };
    let cache = component.inner.parameters.read().await;
    observe_cached(&cache, ObserveSource::AfterApply);
}

pub(crate) fn observe_cached(cache: &IndexMap<String, Parameter>, source: ObserveSource) {
    let expectations = expectations().read().expect("owned expectations lock");
    for (camera_uuid, per_camera) in expectations.iter() {
        for (name, expected) in per_camera {
            if let Some(parameter) = cache.get(name) {
                health::observe_owned_parameter_value(
                    camera_uuid,
                    name,
                    &parameter.value,
                    expected,
                    source,
                );
            }
        }
    }
}

/// Forget eligibility and reported drift for keys we no longer own.
///
/// Expectations only change here, so this is the only place a drift can go stale.
fn prune_to_expectations() {
    let expected: HashSet<EligibilityKey> = expectations()
        .read()
        .expect("owned expectations lock")
        .iter()
        .flat_map(|(camera_uuid, per_camera)| {
            per_camera.keys().map(|name| (*camera_uuid, name.clone()))
        })
        .collect();
    eligible()
        .write()
        .expect("owned eligibility lock")
        .retain(|key| expected.contains(key));
    health::clear_stale_parameter_drifts(&expected);
}

pub(crate) fn push_all_expectations(
    parameters: &ActuatorsParameters,
    map: &mut IndexMap<String, ParamType>,
) {
    // SERVO*_MIN/MAX/TRIM are written on hardware setup as recommended PWM ranges,
    // but they are not owned afterwards: users tune them in QGC / BlueOS parameters,
    // and the Lua script already reads the live values. Flagging drift would block
    // the UI over a customization we should allow.
    // A fixed-focus lens only has tilt, so it owns nothing else.
    if parameters.has_motorized_lens() {
        camera::push_owned_expectations(parameters, map);
        script::push_owned_expectations(parameters, map);
        focus::push_owned_expectations(parameters, map);
        zoom::push_owned_expectations(parameters, map);
    }
    tilt::push_owned_expectations(parameters, map);
}

/// Whether the saved setup stores this parameter as-is, so the autopilot value can be
/// adopted. Channel functions and camera types follow from the channel and camera roles,
/// so only re-applying the hardware setup restores those.
pub(crate) fn is_adoptable_parameter(name: &str) -> bool {
    let is_mount_param = name.starts_with("MNT")
        && (name.ends_with("_TYPE")
            || name.ends_with("_PITCH_MIN")
            || name.ends_with("_PITCH_MAX"));
    let is_script_param = name.starts_with(script::PARAM_PREFIX)
        && (name.ends_with("_ENABLE") || name.ends_with("_GAIN"));
    is_mount_param || is_script_param
}

/// Writes the autopilot's `value` for `name` into the saved setup. Fails for names the saved
/// setup does not store as-is, and for mount types it has no name for.
pub(crate) fn adopt_autopilot_value(
    parameters: &mut ActuatorsParameters,
    name: &str,
    value: f32,
) -> Result<()> {
    let mount = format!("{:?}", tilt::tilt_mount_id(parameters.camera_id));
    let channel = parameters.camera_id as u8;
    if name == format!("{mount}_TYPE") {
        parameters.tilt_mnt_type = match value as i32 {
            1 => api::MountType::Servo,
            7 => api::MountType::BrushlessPWM,
            other => bail!("{name} is {other}, which has no saved mount type"),
        };
    } else if name == format!("{mount}_PITCH_MIN") {
        parameters.tilt_mnt_pitch_min = value as i32;
    } else if name == format!("{mount}_PITCH_MAX") {
        parameters.tilt_mnt_pitch_max = value as i32;
    } else if name == format!("{}{channel}_ENABLE", script::PARAM_PREFIX) {
        parameters.enable_focus_and_zoom_correlation = value != 0.0;
    } else if name == format!("{}{channel}_GAIN", script::PARAM_PREFIX) {
        parameters.focus_margin_gain = value;
    } else {
        bail!(
            "{name} is derived from the channel setup, re-apply the hardware setup to restore it"
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn clear_eligibility_for_test() {
    eligible().write().expect("owned eligibility lock").clear();
}

#[cfg(test)]
pub(crate) fn install_expectations_for_test(camera_uuid: Uuid, parameters: &ActuatorsParameters) {
    let mut per_camera = IndexMap::new();
    push_all_expectations(parameters, &mut per_camera);
    expectations()
        .write()
        .expect("owned expectations lock")
        .insert(camera_uuid, per_camera);
}

#[cfg(test)]
pub(crate) fn clear_expectations_for_test() {
    expectations()
        .write()
        .expect("owned expectations lock")
        .clear();
}

#[cfg(test)]
mod adopt_tests {
    use super::*;

    #[test]
    fn adopt_writes_stored_parameters_and_rejects_derived_ones() {
        let mut parameters = ActuatorsParameters::default();
        for (name, value) in [
            ("MNT1_TYPE", 7.0),
            ("MNT1_PITCH_MIN", -90.0),
            ("MNT1_PITCH_MAX", 55.0),
        ] {
            assert!(is_adoptable_parameter(name));
            adopt_autopilot_value(&mut parameters, name, value).unwrap();
        }
        assert_eq!(parameters.tilt_mnt_type, api::MountType::BrushlessPWM);
        assert_eq!(parameters.tilt_mnt_pitch_min, -90);
        assert_eq!(parameters.tilt_mnt_pitch_max, 55);

        assert!(adopt_autopilot_value(&mut parameters, "MNT1_TYPE", 3.0).is_err());
        assert!(!is_adoptable_parameter("SERVO16_FUNCTION"));
        assert!(adopt_autopilot_value(&mut parameters, "SERVO16_FUNCTION", 0.0).is_err());
    }
}
