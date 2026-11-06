pub mod manager;
mod v0;
mod v1;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Note: make only the current API version accessible:
pub use v1::*;

use crate::v0::SettingsDataV0;

pub use manager::{MANAGER, Manager, clear, init, last_save_error};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "version")]
pub enum RawSettingsData {
    V0(SettingsDataV0),
    V1(SettingsDataV1),
}

pub trait SettingsDataImpl: std::fmt::Debug + Send + Sync {
    fn get_actuators(&self) -> &IndexMap<Uuid, CameraActuatorsSettings>;
    fn get_actuators_mut(&mut self) -> &mut IndexMap<Uuid, CameraActuatorsSettings>;
    fn get_program_max_exposure_mut(&mut self) -> &mut IndexMap<Uuid, u32>;

    fn to_raw(&self) -> RawSettingsData;
}
