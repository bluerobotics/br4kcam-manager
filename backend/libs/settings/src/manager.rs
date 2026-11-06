use anyhow::{Context, Result, anyhow};
use indexmap::IndexMap;
use once_cell::sync::OnceCell;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use tokio::{fs, io::AsyncWriteExt, sync::RwLock};
use tracing::*;
use uuid::Uuid;

use crate::{CameraActuatorsSettings, RawSettingsData, SettingsDataImpl, v1::SettingsDataV1};

pub static MANAGER: OnceCell<RwLock<Manager>> = OnceCell::new();

/// A read-only filesystem or a full SD card only surfaces as a failed request at the
/// moment the user changes something, which is long after it starts mattering.
static LAST_SAVE_ERROR: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug)]
pub struct Manager {
    pub settings: Settings,
}

#[derive(Debug)]
pub struct Settings {
    path: PathBuf,
    inner: Box<dyn SettingsDataImpl>,
}

impl Settings {
    pub async fn try_new(
        path: PathBuf,
        actuators: IndexMap<Uuid, CameraActuatorsSettings>,
    ) -> Result<Self> {
        let settings = Self {
            path,
            inner: Box::new(SettingsDataV1 {
                actuators,
                program_max_exposure: IndexMap::default(),
            }),
        };

        settings.save().await?;

        Ok(settings)
    }

    pub async fn from_path(path: &Path) -> Result<Self> {
        // Reading a backup must still leave the manager writing to the live file, or every
        // later save lands on the backup — which, with a single fixed backup name, would
        // copy that file over itself.
        async fn read_inner(source: &Path, destination: &Path) -> Result<Settings> {
            let contents = fs::read_to_string(source)
                .await
                .with_context(|| format!("Failed to read settings file: {source:?}"))?;

            let raw: RawSettingsData = serde_json::from_str(&contents)
                .with_context(|| format!("Failed to parse JSON from settings: {source:?}"))?;

            let inner = match raw {
                RawSettingsData::V1(v1) => Box::new(v1),
                RawSettingsData::V0(v0) => {
                    warn!("Migrating settings V0 to V1 from {source:?}");
                    Box::new(SettingsDataV1::from(v0))
                }
            };

            let settings = Settings {
                path: destination.to_owned(),
                inner,
            };

            settings.save().await?;

            Ok(settings)
        }

        if path.exists() {
            match read_inner(path, path).await {
                Ok(settings) => return Ok(settings),
                Err(error) => {
                    warn!("Failed reading settings from {path:?}: {error:?}");
                    let corrupt_path = corrupt_path_for(path);
                    if let Err(error) = fs::rename(path, &corrupt_path).await {
                        warn!(
                            "Failed renaming unreadable settings {path:?} to {corrupt_path:?}: {error:?}"
                        );
                    }
                }
            }
        }

        let backup_path = backup_path_for(path);
        if backup_path.exists() {
            return read_inner(&backup_path, path).await;
        }

        Err(anyhow!("No settings file or backup found"))
    }

    pub async fn save(&self) -> Result<()> {
        let result = self.save_inner().await;

        if let Ok(mut last) = LAST_SAVE_ERROR.lock() {
            *last = result.as_ref().err().map(|error| format!("{error:#}"));
        }

        result
    }

    async fn save_inner(&self) -> Result<()> {
        let path = self.path.as_path();
        let settings_file = path.to_string_lossy();

        let raw = self.to_raw();
        let new_contents =
            serde_json::to_string_pretty(&raw).context("Failed to serialize settings to JSON")?;

        if path.exists() {
            let current_contents = fs::read_to_string(path).await.with_context(|| {
                format!("Failed to read existing settings file: {settings_file:?}")
            })?;

            if current_contents == new_contents {
                trace!("Settings unchanged, skipping write");
                return Ok(());
            }

            let backup_path = backup_path_for(path);

            fs::copy(path, &backup_path)
                .await
                .with_context(|| format!("Failed to create backup at {backup_path:?}"))?;
            debug!("Created settings backup: {backup_path:?}");
        }

        // Rename is atomic within a filesystem, so an unclean power-down can lose the new
        // settings but can never leave a torn file where they used to be.
        let temporary_path = temp_path_for(path);
        let mut file = fs::File::create(&temporary_path)
            .await
            .with_context(|| format!("Failed to create {temporary_path:?}"))?;
        file.write_all(new_contents.as_bytes())
            .await
            .with_context(|| format!("Failed to write settings to {temporary_path:?}"))?;
        file.sync_all()
            .await
            .with_context(|| format!("Failed to flush settings to {temporary_path:?}"))?;

        fs::rename(&temporary_path, path)
            .await
            .with_context(|| format!("Failed to replace settings at {settings_file:?}"))?;

        debug!("Wrote new settings to {settings_file:?}:\n{:?}", self.inner);

        Ok(())
    }

    pub fn get_actuators(&self) -> &IndexMap<Uuid, CameraActuatorsSettings> {
        self.inner.get_actuators()
    }

    pub fn get_actuators_mut(&mut self) -> &mut IndexMap<Uuid, CameraActuatorsSettings> {
        self.inner.get_actuators_mut()
    }

    pub fn get_program_max_exposure_mut(&mut self) -> &mut IndexMap<Uuid, u32> {
        self.inner.get_program_max_exposure_mut()
    }

    pub fn to_raw(&self) -> RawSettingsData {
        self.inner.to_raw()
    }
}

/// Constructs our manager, Should be done inside main
#[instrument(level = "debug")]
pub async fn init(settings_file: String, reset: bool) -> Result<()> {
    let settings_path = Path::new(&settings_file);
    let settings = match (reset, Settings::from_path(settings_path).await) {
        (false, Ok(settings)) => settings,
        (false, Err(error)) => {
            if backup_path_for(settings_path).exists() {
                return Err(error).context(
                    "Settings file is unreadable and backup could not be recovered; refusing to overwrite backup with empty settings",
                );
            }

            warn!("Failed reading settings file: {error:?}. Using empty settings.");

            Settings::try_new(settings_path.to_path_buf(), IndexMap::default()).await?
        }
        (true, _) => {
            warn!(
                "Ignoring previous settings files because `--reset` CLI arg was used. Using empty settings."
            );

            Settings::try_new(settings_path.to_path_buf(), IndexMap::default()).await?
        }
    };

    if let Some(manager) = MANAGER.get() {
        manager.write().await.settings = settings;
        return Ok(());
    }

    MANAGER.get_or_init(|| RwLock::new(Manager { settings }));

    Ok(())
}

/// Text of the last failed settings write, cleared by the next successful one.
pub fn last_save_error() -> Option<String> {
    LAST_SAVE_ERROR.lock().ok()?.clone()
}

#[instrument(level = "debug")]
pub async fn clear() -> Result<()> {
    let manager = MANAGER.get().context("settings not initialized")?;
    let mut guard = manager.write().await;

    guard.settings.get_actuators_mut().clear();
    guard.settings.get_program_max_exposure_mut().clear();
    guard.settings.save().await
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "settings.json".into());
    path.with_file_name(format!("{name}{suffix}"))
}

fn backup_path_for(path: &Path) -> PathBuf {
    sibling_path(path, ".bak")
}

fn temp_path_for(path: &Path) -> PathBuf {
    sibling_path(path, ".tmp")
}

fn corrupt_path_for(path: &Path) -> PathBuf {
    sibling_path(path, ".corrupt")
}

// #[cfg(test)]
// mod tests {
//     use tempfile::NamedTempFile;

//     use crate::{CameraActuatorsSettings, api::ActuatorsState};

//     use super::*;

//     #[tokio::test]
//     async fn test_migrate_v0_insert_and_persist_actuators() -> Result<()> {
//         // Create temp file path
//         let tmp_file = NamedTempFile::new()?;
//         let path = tmp_file.path().to_path_buf();

//         // Step 1: Write a SettingsDataV0 JSON to the file
//         let v0 = RawSettingsData::V0(SettingsDataV0);
//         let json = serde_json::to_string_pretty(&v0)?;
//         fs::write(&path, json).await?;

//         // Step 2: Read settings (should auto-migrate to V1)
//         let mut settings = Settings::from_path(&path).await?;
//         let actuators = settings.get_actuators();
//         assert!(
//             actuators.is_empty(),
//             "Expected empty actuator map from V0 migration"
//         );

//         // Step 3: Insert a new actuator
//         let uuid = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"uuid.example.com");
//         let mut actuators = IndexMap::new();
//         actuators.insert(
//             uuid,
//             CameraActuators {
//                 config: CameraActuatorsSettings::default(),
//                 state: ActuatorsState {
//                     focus: Some(1.0),
//                     zoom: Some(2.0),
//                     tilt: Some(3.0),
//                 },
//             },
//         );
//         *settings.get_actuators_mut() = actuators;

//         // Step 4: Save updated settings
//         settings.save().await?;

//         // Step 5: Reload and verify data persisted
//         let settings = Settings::from_path(&path).await?;
//         let reloaded = settings.get_actuators();

//         assert_eq!(reloaded.len(), 1);
//         let loaded = reloaded.get(&uuid).unwrap();
//         assert_eq!(loaded.state.focus, Some(1.0));
//         assert_eq!(loaded.state.zoom, Some(2.0));
//         assert_eq!(loaded.state.tilt, Some(3.0));

//         Ok(())
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn repeated_saves_leave_one_backup_and_no_temporary() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("settings.json");

        let settings = Settings::try_new(path.clone(), IndexMap::default()).await?;
        let expected = fs::read_to_string(&path).await?;

        // An unchanged file is skipped, so dirty the live copy to force a real write.
        for revision in 0..3 {
            fs::write(&path, format!("clobbered {revision}")).await?;
            settings.save().await?;

            assert_eq!(fs::read_to_string(&path).await?, expected);
        }

        let mut leftovers: Vec<String> = std::fs::read_dir(dir.path())?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "settings.json")
            .collect();
        leftovers.sort();

        assert_eq!(leftovers, vec!["settings.json.bak".to_string()]);

        Ok(())
    }
}
