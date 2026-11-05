//! Camera firmware updates from our firmware server.
//!
//! The server is any static host (a bucket is enough) serving `index.json`:
//! `{ "firmwares": [ { "version": "18.010.U35.6_0054_V18(261030)", "file": "IPC_….img", "sha256": "…" } ] }`,
//! where `file` is relative to the index URL.

use std::{sync::OnceLock, time::Duration};

use anyhow::{Context, Result, anyhow, ensure};
use axum::Json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::*;
use ts_rs::TS;
use url::Url;
use uuid::Uuid;

use super::{Action, CameraControl, ProbeStatus, control_inner, is_transport_offline};

/// The camera reports the flash progress through `fwUpgradeProgress`; give up if it stalls.
const FLASH_TIMEOUT: Duration = Duration::from_secs(600);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

static SERVER_URL: OnceLock<Option<Url>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FirmwareUpdate {
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct FirmwareRelease {
    pub version: String,
    pub file: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
struct FirmwareIndex {
    firmwares: Vec<FirmwareRelease>,
}

/// Sets the firmware server base URL (the directory holding `index.json`).
pub fn init(server_url: Option<Url>) {
    let _ = SERVER_URL.set(server_url);
}

fn index_url() -> Result<Url> {
    let base = SERVER_URL
        .get()
        .cloned()
        .flatten()
        .context("No firmware server configured (FIRMWARE_SERVER_URL)")?;
    // Without the trailing slash, `join` would replace the last path segment.
    let base = if base.path().ends_with('/') {
        base
    } else {
        format!("{base}/").parse()?
    };
    Ok(base.join("index.json")?)
}

async fn fetch_index() -> Result<(Url, Vec<FirmwareRelease>)> {
    let url = index_url()?;
    let index: FirmwareIndex = reqwest::Client::new()
        .get(url.clone())
        .timeout(Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .with_context(|| format!("Invalid firmware index at {url}"))?;
    Ok((url, index.firmwares))
}

async fn sys_config(camera_uuid: Uuid) -> Result<serde_json::Value> {
    control_inner(Json(CameraControl {
        camera_uuid,
        action: Action::GetSysConfig,
    }))
    .await
}

fn camera_version(sys_config: &serde_json::Value) -> Result<String> {
    sys_config["version"]
        .as_str()
        .map(str::to_string)
        .context("Camera did not report its firmware version")
}

/// `{ current, available: [version] }`.
#[instrument(level = "debug")]
pub async fn get_updates(camera_uuid: Uuid) -> Result<serde_json::Value> {
    let current = camera_version(&sys_config(camera_uuid).await?)?;
    // No server configured or reachable (a vehicle without internet) is normal, not an error.
    let available: Vec<String> = fetch_index()
        .await
        .map(|(_, firmwares)| firmwares.into_iter().map(|f| f.version).collect())
        .unwrap_or_else(|error| {
            debug!("No firmware index: {error:#}");
            vec![]
        });
    Ok(serde_json::json!({ "current": current, "available": available }))
}

/// The hardware compatibility word the camera's own web UI compares with `update_type` from
/// `getSysConfig`: little-endian header words 2, 3 and 4 packed as 8, 8 and 16 bits.
fn image_update_type(image: &[u8]) -> Option<u64> {
    let word = |i: usize| -> Option<u64> {
        Some(u32::from_le_bytes(image.get(i * 4..i * 4 + 4)?.try_into().ok()?) as u64)
    };
    Some((word(2)? << 24) | (word(3)? << 16) | word(4)?)
}

fn verify_sha256(image: &[u8], expected: &str) -> Result<()> {
    let actual = base16ct::lower::encode_string(&Sha256::digest(image));
    ensure!(
        actual.eq_ignore_ascii_case(expected.trim()),
        "Firmware checksum mismatch: expected {expected}, got {actual}"
    );
    Ok(())
}

/// Whether the camera answered with a non-zero `code`. A body without one is accepted, as
/// the camera answers `fwUpgrade` without JSON (its own web UI ignores that answer too).
fn camera_rejected(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|body| body["code"].as_i64())
        .is_some_and(|code| code != 0)
}

async fn camera_ok(response: reqwest::Response, step: &str) -> Result<()> {
    let body = response.error_for_status()?.text().await?;
    info!("Camera answered {step} with {body:?}");
    ensure!(!camera_rejected(&body), "Camera rejected {step}: {body}");
    Ok(())
}

/// Download, verify and flash `version`, then wait for the camera to come back running it.
///
/// `progress` receives human-readable step messages.
#[instrument(level = "debug", skip(progress))]
pub async fn update(
    camera_uuid: Uuid,
    version: &str,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<serde_json::Value> {
    let (index_url, firmwares) = fetch_index().await?;
    let release = firmwares
        .into_iter()
        .find(|release| release.version == version)
        .with_context(|| format!("Firmware {version} is not on the firmware server"))?;

    progress("Downloading firmware…");
    let image_url = index_url.join(&release.file)?;
    let client = reqwest::Client::new();
    let image = client
        .get(image_url.clone())
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await
        .with_context(|| format!("Failed downloading {image_url}"))?;

    verify_sha256(&image, &release.sha256)?;

    let sys_config = sys_config(camera_uuid).await?;
    let camera_type = sys_config["update_type"].as_u64();
    let image_type = image_update_type(&image);
    ensure!(
        camera_type.is_some() && camera_type == image_type,
        "Firmware {version} is not for this camera (image type {image_type:?}, camera type {camera_type:?})"
    );

    let host = mcm_client::camera_address(&camera_uuid)
        .await
        .context("Camera address unknown")?;
    let api = |action: &str| format!("http://{host}/action/{action}");

    info!(
        "Flashing {camera_uuid:?} from {} to {version}",
        camera_version(&sys_config)?
    );

    // Same sequence as the camera's own web UI upgrade page.
    progress("Uploading firmware to the camera…");
    camera_ok(
        client
            .post(api("fwInfo"))
            .timeout(Duration::from_secs(30))
            .json(&serde_json::json!({ "fw_size": image.len() }))
            .send()
            .await?,
        "fwInfo",
    )
    .await?;
    // Cloning `Bytes` shares the buffer instead of copying the whole image.
    let part = reqwest::multipart::Part::stream_with_length(image.clone(), image.len() as u64)
        .file_name(release.file.clone());
    camera_ok(
        client
            .post(api("uploadFile"))
            .timeout(UPLOAD_TIMEOUT)
            .multipart(reqwest::multipart::Form::new().part("file", part))
            .send()
            .await
            .context("Failed uploading firmware to the camera")?,
        "uploadFile",
    )
    .await?;

    progress("Flashing firmware… 0%");
    camera_ok(
        client
            .post(api("fwUpgrade"))
            .timeout(Duration::from_secs(30))
            .json(&serde_json::json!({ "pack_name": release.file, "pack_type": 0 }))
            .send()
            .await?,
        "fwUpgrade",
    )
    .await?;

    tokio::time::timeout(FLASH_TIMEOUT, async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Ok(body) = client
                .get(api("fwUpgradeProgress"))
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
            else {
                // The camera may already be rebooting into the new firmware.
                break;
            };
            let body: serde_json::Value = body.json().await.unwrap_or_default();
            // The camera spells it `precent`, sometimes as a string.
            let percent = body["precent"]
                .as_u64()
                .or_else(|| body["precent"].as_str()?.trim().parse().ok());
            if let Some(percent) = percent {
                progress(&format!("Flashing firmware… {percent}%"));
                if percent >= 100 {
                    break;
                }
            }
        }
    })
    .await
    .map_err(|_| anyhow!("Camera stopped reporting flash progress"))?;

    progress("Rebooting camera…");
    let probe = Json(CameraControl {
        camera_uuid,
        action: Action::GetSysConfig,
    });
    super::wait_for_camera_reboot(|| async {
        match control_inner(probe.clone()).await {
            Ok(_) => ProbeStatus::Reachable,
            Err(error) if is_transport_offline(&error) => ProbeStatus::Offline,
            Err(_) => ProbeStatus::SoftFailure,
        }
    })
    .await?;

    let running = camera_version(&self::sys_config(camera_uuid).await?)?;
    // A camera that refuses an image (e.g. its own header checksum fails) still reports 100%
    // and reboots, just into the old firmware.
    ensure!(
        running == version,
        "The camera refused firmware {version} and is still running {running}"
    );
    info!("{camera_uuid:?} is now running {version}");

    Ok(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_update_type_matches_camera_word() {
        // Header of IPC_18.010.U35.6_0054_V18(261030).img; the camera reports 318898227.
        let header: Vec<u8> = [0xabcabccbu32, 0x42, 0x13, 0x2, 0x33]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        assert_eq!(image_update_type(&header), Some(318898227));
        assert_eq!(image_update_type(&header[..19]), None);
    }

    #[test]
    fn camera_rejected_only_on_nonzero_code() {
        assert!(!camera_rejected("begain upgrade"));
        assert!(!camera_rejected("{\"code\":0 }"));
        assert!(camera_rejected("{\"code\":-1}"));
    }

    #[test]
    fn verify_sha256_checks_digest() {
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(verify_sha256(b"", empty).is_ok());
        assert!(verify_sha256(b"", &empty.to_uppercase()).is_ok());
        assert!(verify_sha256(b"x", empty).is_err());
    }
}
