use std::{future::Future, pin::Pin};

use anyhow::{Context, Result};
use axum::{Json, http::StatusCode, response::IntoResponse};
use mcm_client::camera_address;
use protocol::{
    display::{advanced_display::AdvancedParameterSetting, base_display::BaseParameterSetting},
    video::video_parameters::VideoParameterSettings,
};
use serde::{Deserialize, Serialize};
use tracing::*;
use ts_rs::TS;
use url::Url;
use uuid::Uuid;
use web_client::send_request;

pub mod protocol;

/// Number of consecutive transport-level failures needed to consider the camera offline,
/// so a single flaky timeout doesn't end the offline phase.
const OFFLINE_CONFIRMATIONS: u8 = 2;
/// How long to wait for the camera to stop answering after a restart request.
const REBOOT_OFFLINE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// How long to wait for the camera to answer again after it went offline.
const REBOOT_ONLINE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
// #[tsync] // FIXME: Disabled for now, see https://github.com/Wulf/tsync/issues/58
pub struct CameraControl {
    #[serde(default)]
    #[ts(as = "String")]
    pub camera_uuid: Uuid,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "action", content = "json")]
// #[tsync] // FIXME: Disabled for now, see https://github.com/Wulf/tsync/issues/58
pub enum Action {
    #[serde(rename = "getSysConfig")]
    GetSysConfig,
    #[serde(rename = "getImageAdjustment")]
    GetImageAdjustment,
    #[serde(rename = "getImageAdjustmentEx")]
    GetImageAdjustmentEx,
    #[serde(rename = "getVencConf")]
    GetVideoParameterSettings(VideoParameterSettings),
    #[serde(rename = "setImageAdjustment")]
    SetImageAdjustment(BaseParameterSetting),
    #[serde(rename = "setImageAdjustmentEx")]
    SetImageAdjustmentEx(AdvancedParameterSetting),
    /// Important: This is a wrapper, not part of the camera protocol
    #[serde(rename = "setImageAdjustmentExAll")]
    SetImageAdjustmentExAll(AdvancedParameterSetting),
    #[serde(rename = "setVencConf")]
    SetVideoParameterSettings(VideoParameterSettings),
    #[serde(rename = "restart")]
    Restart,
    /// Important: This is a wrapper, not part of the camera protocol
    #[serde(rename = "setRecommendedCameraSettings")]
    SetRecommendedCameraSettings,
}

/// Outcome of one reachability probe while waiting for a camera reboot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeStatus {
    /// Camera answered successfully.
    Reachable,
    /// Transport-level failure: camera looks offline.
    Offline,
    /// Camera answered with a non-transport error; keep waiting.
    SoftFailure,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[instrument(level = "debug")]
fn control_inner(
    camera_control: Json<CameraControl>,
) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send>> {
    Box::pin(async move {
        debug!("Got control query: {camera_control:#?}");

        // Special case for command wrappers:
        match &camera_control.action {
            Action::SetRecommendedCameraSettings => {
                return apply_recommended_camera_settings(camera_control.camera_uuid).await;
            }
            Action::SetImageAdjustmentExAll(params) => {
                return apply_set_image_adjustment_ex_all(params).await;
            }
            _ => (),
        }

        let action_value = serde_json::to_value(&camera_control.action).unwrap();
        let action_map = action_value.as_object().unwrap();
        let payload = action_map
            .get("json")
            .map(|value| value.to_string())
            .unwrap_or_default();

        let url = get_camera_api_url(&camera_control.0).await?;

        debug!("URL: {url}, json: {payload}");

        let res: serde_json::Value =
            send_request(reqwest::Client::new().post(url), payload).await?;

        debug!("Answer from the camera: {res:#?}");

        let res = match &camera_control.action {
            Action::SetImageAdjustment(_) => {
                let mut camera_control = camera_control.0.clone();
                camera_control.action = Action::GetImageAdjustment;

                control_inner(Json(camera_control)).await
            }
            Action::SetImageAdjustmentEx(_) => {
                let mut camera_control = camera_control.0.clone();
                camera_control.action = Action::GetImageAdjustmentEx;

                control_inner(Json(camera_control)).await
            }
            Action::SetVideoParameterSettings(video_parameters_settings) => {
                let mut camera_control = camera_control.0.clone();
                camera_control.action = Action::GetVideoParameterSettings(VideoParameterSettings {
                    channel: video_parameters_settings.channel.clone(),
                    ..Default::default()
                });

                control_inner(Json(camera_control)).await
            }
            Action::Restart => {
                let mut camera_control = camera_control.0.clone();
                camera_control.action = Action::GetSysConfig;
                let probe = Json(camera_control);

                wait_for_camera_reboot(|| async {
                    match control_inner(probe.clone()).await {
                        Ok(_) => ProbeStatus::Reachable,
                        Err(error) if is_transport_offline(&error) => ProbeStatus::Offline,
                        Err(_) => ProbeStatus::SoftFailure,
                    }
                })
                .await?;

                Ok(res)
            }
            _ => Ok(res),
        }?;

        debug!("res.to_string(): {}", res.to_string());

        Ok(res)
    })
}

/// Shared entry point for REST and WebSocket camera control requests.
#[instrument(level = "debug")]
pub async fn handle_control(camera_control: CameraControl) -> Result<serde_json::Value> {
    control_inner(Json(camera_control)).await
}

#[instrument(level = "debug")]
pub async fn control(camera_control: Json<CameraControl>) -> impl IntoResponse {
    match handle_control(camera_control.0).await {
        Ok(res) => (StatusCode::OK, res.to_string()).into_response(),
        Err(error) => {
            warn!("res from send_request: {error:#?}");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:?}")).into_response()
        }
    }
}

#[instrument(level = "debug")]
pub async fn list() -> impl IntoResponse {
    let cameras = mcm_client::cameras().await;

    let json = match serde_json::to_string(&cameras) {
        Ok(json) => json,
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:?}")).into_response();
        }
    };

    json.into_response()
}

#[instrument(level = "debug")]
async fn apply_set_image_adjustment_ex_all(
    params: &AdvancedParameterSetting,
) -> Result<serde_json::Value> {
    let cameras = mcm_client::cameras().await;
    let mut errors = vec![];

    for (camera_uuid, camera) in &cameras {
        let camera_control = CameraControl {
            camera_uuid: *camera_uuid,
            action: Action::SetImageAdjustmentEx(params.clone()),
        };

        if let Err(error) = control_inner(Json(camera_control)).await {
            let message = format!("{}: {error:#}", camera.hostname);
            error!(message);
            errors.push(message);
        }
    }

    match errors.len() {
        0 => Ok(serde_json::Value::Null),
        1 => Err(anyhow::anyhow!("{}", errors[0])),
        _ => Err(anyhow::anyhow!(
            "Multiple errors happened:\n{}",
            errors
                .iter()
                .map(|message| format!("- {message}"))
                .collect::<Vec<_>>()
                .join("\n")
        )),
    }
}

#[instrument(level = "debug")]
pub async fn apply_recommended_camera_settings(camera_uuid: Uuid) -> Result<serde_json::Value> {
    let mut errors = vec![];

    info!("Applying recommended camera settings to {camera_uuid:?}");

    // Set main channel to 4k@30fps, and unused channels to 480p@5fps
    {
        use protocol::video::video_parameters::*;

        let channel_configs = vec![
            CameraControl {
                camera_uuid,
                action: Action::SetVideoParameterSettings(VideoParameterSettings {
                    channel: Some(VideoChannelValue::MainStream),
                    encode_profile: Some(VideoEncodingProfileValue::HighProfile),
                    encode_type: Some(VideoEncodeTypeValue::H264),
                    pic_width: Some(3840),
                    pic_height: Some(2160),
                    rc_mode: Some(VideoRcModeValue::ConstantBitRate),
                    bitrate: Some(16384),
                    frame_rate: Some(30),
                    gop: Some(60),
                    ..Default::default()
                }),
            },
            CameraControl {
                camera_uuid,
                action: Action::SetVideoParameterSettings(VideoParameterSettings {
                    channel: Some(VideoChannelValue::AuxiliaryStream),
                    encode_profile: Some(VideoEncodingProfileValue::Baseline),
                    encode_type: Some(VideoEncodeTypeValue::H264),
                    pic_width: Some(640),
                    pic_height: Some(480),
                    rc_mode: Some(VideoRcModeValue::ConstantBitRate),
                    bitrate: Some(32),
                    frame_rate: Some(5),
                    gop: Some(60),
                    ..Default::default()
                }),
            },
            CameraControl {
                camera_uuid,
                action: Action::SetVideoParameterSettings(VideoParameterSettings {
                    channel: Some(VideoChannelValue::ThirdStream),
                    encode_profile: Some(VideoEncodingProfileValue::Baseline),
                    encode_type: Some(VideoEncodeTypeValue::H264),
                    pic_width: Some(640),
                    pic_height: Some(480),
                    rc_mode: Some(VideoRcModeValue::ConstantBitRate),
                    bitrate: Some(32),
                    frame_rate: Some(5),
                    gop: Some(60),
                    ..Default::default()
                }),
            },
        ];

        for camera_control in channel_configs {
            if let Err(error) = control_inner(Json(camera_control)).await {
                let message = format!(
                    "Failed applying recommended camera settings for VideoParameterSettings: {error:#}"
                );
                error!(message);
                errors.push(message);
            }
        }
    }

    {
        use protocol::display::base_display::*;

        let action = Action::SetImageAdjustment(BaseParameterSetting {
            set_default: Some(1),
            ..Default::default()
        });

        let camera_control = CameraControl {
            camera_uuid,
            action,
        };

        if let Err(error) = control_inner(Json(camera_control)).await {
            let message = format!(
                "Failed applying recommended camera settings for BaseParameterSetting: {error:#}"
            );
            error!(message);
            errors.push(message);
        }
    }

    {
        use protocol::display::advanced_display::*;

        let action = Action::SetImageAdjustmentEx(AdvancedParameterSetting {
            set_default: Some(1),
            ..Default::default()
        });

        let camera_control = CameraControl {
            camera_uuid,
            action,
        };

        if let Err(error) = control_inner(Json(camera_control)).await {
            let message = format!(
                "Failed applying recommended camera settings for AdvancedParameterSetting: {error:#}"
            );
            error!(message);
            errors.push(message);
        }
    }

    match errors.len() {
        0 => Ok(serde_json::Value::Null),
        1 => Err(anyhow::anyhow!("{}", errors[0])),
        _ => Err(anyhow::anyhow!(
            "Multiple errors happened:\n{}",
            errors
                .iter()
                .map(|message| format!("- {message}"))
                .collect::<Vec<_>>()
                .join("\n")
        )),
    }
}

#[instrument(level = "debug")]
async fn get_camera_api_url(camera_control: &CameraControl) -> Result<Url> {
    let camera_uuid = camera_control.camera_uuid;
    let action_value = serde_json::to_value(&camera_control.action).unwrap();
    let action_map = action_value.as_object().unwrap();
    let action = action_map.get("action").unwrap().as_str().unwrap();

    let hostname = camera_address(&camera_uuid)
        .await
        .context("Camera address unknown")?;

    let port = 80;

    // NOTE: This part of the protocol is not working: only the get requests using cgi_action parameter are working.
    // I'm leaving it here in case they fix it:
    // Some(Credentials { username, password }) => {
    //     format!(
    //         "http://{hostname}:{port}/action/cgi_action?user={username}&pwd={password}&action={action}", password=hash_password(&password)
    //     )
    // }
    // None => format!("http://{hostname}:{port}/action/{action}"),
    // So for now we are just using the protocol without the authentication:
    format!("http://{hostname}:{port}/action/{action}")
        .parse()
        .context("Invalid URl")
}

#[instrument(level = "debug")]
pub fn hash_password(password: &str) -> String {
    use md5::{Digest, Md5};

    let mut hasher = Md5::new();
    hasher.update(password.as_bytes());

    base16ct::lower::encode_string(&hasher.finalize())
}

/// Waits for `probe` to report offline (reboot started) then reachable again.
///
/// Each phase is wrapped in `timeout` so slow probes cannot push wall time past the budget.
#[instrument(level = "debug", skip_all)]
async fn wait_for_camera_reboot<F, Fut>(mut probe: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ProbeStatus>,
{
    match tokio::time::timeout(REBOOT_OFFLINE_TIMEOUT, wait_reboot_offline(&mut probe)).await {
        Ok(()) => {}
        Err(_) => {
            warn!(
                "Camera did not go offline after restart request, it might have already rebooted. Waiting for it to answer again..."
            );
        }
    }

    tokio::time::timeout(REBOOT_ONLINE_TIMEOUT, wait_reboot_online(&mut probe))
        .await
        .map_err(|_| anyhow::anyhow!("Camera did not come back online after restart"))?
}

#[instrument(level = "debug", skip_all)]
async fn wait_reboot_offline<F, Fut>(probe: &mut F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ProbeStatus>,
{
    let mut consecutive_offline = 0u8;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match probe().await {
            ProbeStatus::Reachable => {
                consecutive_offline = 0;
                debug!("Waiting for camera to go offline after restart...");
            }
            ProbeStatus::SoftFailure => {
                consecutive_offline = 0;
                debug!(
                    "Camera answered with a non-transport error while waiting for it to go offline"
                );
            }
            ProbeStatus::Offline => {
                consecutive_offline += 1;
                if consecutive_offline >= OFFLINE_CONFIRMATIONS {
                    debug!("Camera went offline after restart");
                    return;
                }
                debug!(
                    "Camera unreachable ({consecutive_offline}/{OFFLINE_CONFIRMATIONS}) after restart"
                );
            }
        }
    }
}

#[instrument(level = "debug", skip_all)]
async fn wait_reboot_online<F, Fut>(probe: &mut F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ProbeStatus>,
{
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match probe().await {
            ProbeStatus::Reachable => {
                debug!("Camera came back online after restart");
                return Ok(());
            }
            ProbeStatus::Offline | ProbeStatus::SoftFailure => {
                debug!("Waiting for camera to come back online");
            }
        }
    }
}

/// Tells whether the error means the camera is unreachable at the transport level, which is
/// the only kind of failure that proves it went offline. Protocol-level failures (HTTP status,
/// body decoding, URL building, camera not found, etc.) mean the camera is still answering.
fn is_transport_offline(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(is_transport_error)
    })
}

fn is_transport_error(error: &reqwest::Error) -> bool {
    if error.is_status() || error.is_decode() || error.is_builder() {
        return false;
    }

    error.is_timeout() || error.is_connect() || error.is_request()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use serde_json::json;

    use utils::deserialize;

    use super::{
        CameraControl, ProbeStatus, REBOOT_OFFLINE_TIMEOUT, REBOOT_ONLINE_TIMEOUT,
        is_transport_offline, wait_for_camera_reboot,
    };

    #[test]
    fn reboot_budget_fits_under_ui_reboot_grace() {
        // camera_ui::REBOOT_GRACE is 180s; keep the wait budget strictly below it.
        assert!(REBOOT_OFFLINE_TIMEOUT + REBOOT_ONLINE_TIMEOUT < Duration::from_secs(180));
    }

    #[tokio::test(start_paused = true)]
    async fn reboot_wait_succeeds_when_camera_goes_offline_then_online() {
        let calls = AtomicUsize::new(0);
        wait_for_camera_reboot(|| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                match call {
                    1 | 2 => ProbeStatus::Reachable,
                    3 | 4 => ProbeStatus::Offline,
                    _ => ProbeStatus::Reachable,
                }
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn reboot_wait_succeeds_when_camera_never_goes_offline() {
        wait_for_camera_reboot(|| async { ProbeStatus::Reachable })
            .await
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn reboot_wait_errors_when_camera_never_returns() {
        let error = wait_for_camera_reboot(|| async { ProbeStatus::Offline })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("did not come back online"));
    }

    #[tokio::test(start_paused = true)]
    async fn reboot_wait_bounds_wall_time_despite_slow_probes() {
        let start = tokio::time::Instant::now();
        let error = wait_for_camera_reboot(|| async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            ProbeStatus::Offline
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("did not come back online"));
        // Without phase timeouts, each 10s probe would push past REBOOT_GRACE (180s).
        assert!(start.elapsed() <= REBOOT_OFFLINE_TIMEOUT + REBOOT_ONLINE_TIMEOUT);
    }

    #[test]
    fn is_transport_offline_test() {
        // Non-reqwest errors never count as offline, even when their message looks like one
        let error = anyhow::anyhow!("error trying to connect: connection refused")
            .context("Failed getting camera settings");
        assert!(!is_transport_offline(&error));

        // Reqwest errors that aren't transport failures (here, an invalid URL) don't count either
        let error = anyhow::Error::from(
            reqwest::Client::new()
                .post("http:://camera/action/getSysConfig")
                .build()
                .unwrap_err(),
        )
        .context("Failed getting camera settings");
        assert!(!is_transport_offline(&error));
    }

    #[test]
    fn action_serde_test() {
        let payload = json!({
            "camera_uuid": "bc071801-c50f-8301-ac36-bc071801c50f",
            "action": "setImageAdjustmentEx",
            "json": {
                "onceAWB": 1
            }
        })
        .to_string();

        let camera_control = deserialize::<CameraControl>(&payload).unwrap();

        let serialized_action = serde_json::to_string(&camera_control.action).unwrap();

        let expected_action = json!({
            "action": "setImageAdjustmentEx",
            "json": {
                "onceAWB": 1
            }
        })
        .to_string();

        assert_eq!(expected_action, serialized_action);
    }

    #[test]
    fn set_image_adjustment_ex_all_does_not_require_camera_uuid() {
        let payload = json!({
            "action": "setImageAdjustmentExAll",
            "json": { "onceAWB": 1 }
        })
        .to_string();

        let camera_control = deserialize::<CameraControl>(&payload).unwrap();

        assert!(camera_control.camera_uuid.is_nil());
        assert!(matches!(
            camera_control.action,
            crate::Action::SetImageAdjustmentExAll(_)
        ));
    }
}
