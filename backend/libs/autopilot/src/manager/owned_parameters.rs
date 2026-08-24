use std::{
    collections::HashSet,
    sync::{OnceLock, RwLock},
};

use indexmap::IndexMap;
use uuid::Uuid;

use crate::{
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

pub(crate) fn establish_baseline_from_cache(cache: &IndexMap<String, Parameter>) {
    observe_cached(cache, ObserveSource::BulkSync);
}

pub(crate) async fn reevaluate_after_apply() {
    let Ok(component) = crate::mavlink::component() else {
        return;
    };
    let cache = component.inner.parameters.read().await;
    observe_cached(&cache, ObserveSource::AfterApply);
}

fn observe_cached(cache: &IndexMap<String, Parameter>, source: ObserveSource) {
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

fn push_all_expectations(parameters: &ActuatorsParameters, map: &mut IndexMap<String, ParamType>) {
    // SERVO*_MIN/MAX/TRIM are written on hardware setup as recommended PWM ranges,
    // but they are not owned afterwards: users tune them in QGC / BlueOS parameters,
    // and the Lua script already reads the live values. Flagging drift would block
    // the UI over a customization we should allow.
    camera::push_owned_expectations(parameters, map);
    script::push_owned_expectations(parameters, map);
    focus::push_owned_expectations(parameters, map);
    zoom::push_owned_expectations(parameters, map);
    tilt::push_owned_expectations(parameters, map);
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
