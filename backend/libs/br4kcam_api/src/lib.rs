//! Wire types and protocol envelopes shared by the 4K Cam Manager backend and the
//! generated TypeScript bindings.
//!
//! This crate must stay a dependency-light leaf: it must not depend on
//! `br4kcam_manager` (or other application crates). `bindings_generator` exports these
//! types into `frontend/src/bindings/br4kcam_api.d.ts`.
//!
//! That file is the source of truth for the WebSocket *envelopes* and shared UI state
//! (`WsRequest` / `WsResponse` / `WsEvent`, `CameraStateEvent`, `CameraUiState`,
//! `ConnectionStats`). Camera and autopilot *payload* shapes live in `br4kcam.ts` /
//! `autopilot.d.ts` (from `br4kcam_commands` and `autopilot::api`); several
//! `CameraStateEvent` fields remain `serde_json::Value` / TypeScript `unknown` for
//! that reason.

use chrono::{DateTime, Utc};
use serde_derive::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;
use uuid::Uuid;

/// Reachability of one camera from the 4K Cam Manager.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
pub enum CameraConnectivity {
    /// No probe has completed yet since this camera became interesting.
    #[default]
    Unknown,
    /// The camera answers its HTTP API.
    Online,
    /// The camera is in MCM's list but there is no TCP path to port 80,
    /// or a configured camera left discovery and its last IP no longer answers TCP.
    Unreachable,
    /// Port 80 accepts connections but the camera HTTP API does not answer.
    Unresponsive,
    /// A previously configured camera that MCM is no longer discovering, and we
    /// have no last IP to probe (so we cannot distinguish cable-loss from gone).
    Missing,
}

/// Whether the autopilot has the Lua script this install expects.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
pub enum LuaScriptStatus {
    /// No camera is configured yet, or the script file could not be read at all.
    #[default]
    Unknown,
    /// The installed script is what the current configuration generates.
    Ok,
    /// No script is installed in the autopilot scripts folder.
    Missing,
    /// A script is installed, but it is not the one this configuration generates —
    /// stale after a manager upgrade, or hand-edited.
    Outdated,
    /// The expected script is installed, but the autopilot reported it erroring. A
    /// firmware upgrade that changed a scripting API reaches the user this way.
    /// The message is in [`SystemHealth::lua_script_detail`].
    Failing,
}

/// Health of the 4K Cam Manager to Mavlink Camera Manager link.
///
/// "Refused" and "wedged" are deliberately one state: the user action is the
/// same for both. The distinction lives in [`SystemHealth::mcm_detail`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
pub enum McmHealth {
    /// No poll cycle has completed yet.
    #[default]
    Unknown,
    /// MCM answers `/info` with a supported version and lists devices.
    Online,
    /// MCM is not usable; see `mcm_detail`.
    Down,
}

/// Health of the autopilot control path, from outermost failure to innermost.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
pub enum AutopilotHealth {
    /// No assessment yet, or the first endpoint attempt has not returned.
    #[default]
    Unknown,
    /// BlueOS ardupilot-manager is not accepting our MAVLink endpoint setup.
    EndpointSetupFailed,
    /// The endpoint exists but no MAVLink traffic is arriving from anyone.
    MavlinkDown,
    /// MAVLink traffic flows but the autopilot is not heartbeating.
    AutopilotOffline,
    /// The autopilot heartbeats but does not answer commands or stream data.
    Unresponsive,
    /// Connected and heartbeating; the initial parameter download is running.
    Syncing,
    /// Fully usable.
    Online,
}

/// A configured camera that is absent from MCM's current list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct ExpectedCamera {
    /// Camera UUID from the persisted actuators settings.
    #[ts(as = "String")]
    pub uuid: Uuid,
    /// Last IP this camera was seen at in this process, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_hostname: Option<String>,
}

/// One autopilot parameter whose live value no longer matches persisted actuator settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct ParameterDrift {
    /// MAVLink parameter name, e.g. `SERVO1_FUNCTION`.
    pub name: String,
    /// Value 4K Cam Manager last wrote from the saved configuration.
    pub expected: f32,
    /// Value currently reported by the autopilot.
    pub actual: f32,
}

/// Support-oriented counters. Never a primary signal; rendered only inside the
/// collapsed "Health diagnostics" panel and included in the copy-to-clipboard blob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
pub struct Diagnostics {
    /// Negotiated MAVLink parameter encoding, e.g. `CCast`. `None` before sync.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub param_encoding: Option<String>,
    /// Successful MAVLink reconnects since process start.
    #[ts(type = "number")]
    pub mavlink_reconnects: u64,
    /// MAVLink frames this process dropped from the internal broadcast.
    #[ts(type = "number")]
    pub mavlink_frames_lagged: u64,
    /// `camera/state` events dropped by a WebSocket or the actuators bridge.
    #[ts(type = "number")]
    pub state_events_lagged: u64,
    /// Consecutive failing MCM poll cycles; 0 when healthy.
    pub mcm_consecutive_failures: u32,
    /// Lua reloads the manager triggered after the script stopped answering. A healthy
    /// system reloads rarely; the count is a support signal, not a user-facing problem.
    pub script_reloads: u32,
    /// Age of the newest MAVLink frame, milliseconds. `None` when never seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub last_frame_age_ms: Option<u64>,
    /// Age of the newest autopilot heartbeat, milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub last_heartbeat_age_ms: Option<u64>,
    /// Age of the newest `SERVO_OUTPUT_RAW` sample, milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub last_servo_age_ms: Option<u64>,
    /// Backend version and git sha, for the support blob.
    pub backend_version: String,
    /// Why the last settings write failed, e.g. a read-only filesystem or a full disk.
    /// `None` when the last write succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub settings_error: Option<String>,
}

/// Backend-wide health, pushed on the `system/health` WebSocket event and
/// served by `GET /v1/health`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default, TS)]
pub struct SystemHealth {
    /// Manager to MCM link state.
    pub mcm: McmHealth,
    /// Concrete failure text for the current non-`Online` MCM state.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mcm_detail: Option<String>,
    /// How many cameras MCM is currently discovering.
    pub cameras_discovered: usize,
    /// Cameras this install has configured that MCM is not currently listing.
    pub expected_missing: Vec<ExpectedCamera>,
    /// Autopilot control path state.
    pub autopilot: AutopilotHealth,
    /// Concrete failure text for the current non-`Online` autopilot state.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub autopilot_detail: Option<String>,
    /// True when `SCR_ENABLE` is known and not 1: focus/zoom correlation is inert.
    pub lua_scripting_disabled: bool,
    /// Whether the installed Lua script matches this configuration.
    pub lua_script: LuaScriptStatus,
    /// What the autopilot said when the script failed, e.g. the Lua file and line.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub lua_script_detail: Option<String>,
    /// Autopilot parameters that no longer match persisted actuator settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parameter_drifts: Option<Vec<ParameterDrift>>,
    /// Support counters.
    pub diagnostics: Diagnostics,
}

/// Shared UI overlay state synchronized to every subscribed frontend.
#[derive(Debug, Clone, Serialize, Default, PartialEq, TS)]
pub struct CameraUiState {
    /// True while a long-running backend action is in progress.
    pub loading: bool,
    /// Human-readable loading message.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub loading_message: Option<String>,
    /// True while waiting for the camera to come back after reboot.
    pub rebooting: bool,
    /// Modal error dialog message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error_dialog: Option<String>,
    /// Transient warning toast message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub warning_toast: Option<String>,
    /// One-push white balance lifecycle shared across clients. Absent = idle.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub one_push_awb: Option<OnePushAwbStatus>,
    /// Reachability of this camera from the 4K Cam Manager.
    pub connectivity: CameraConnectivity,
    /// MCM video stream failure detail when the stream stayed broken through
    /// self-heal backoff. `None` when the stream is healthy or not yet assessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stream_error: Option<String>,
    /// MCM ONVIF authentication failure detail when login with the expected
    /// factory credentials fails. `None` when ONVIF login succeeds or is not yet assessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub onvif_auth_error: Option<String>,
    /// Step the default hardware setup is on. `None` when it is not running.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub setup_progress: Option<SetupProgress>,
    /// Outcome of the last default hardware setup, kept until a client dismisses it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub setup_result: Option<SetupResult>,
}

/// Step-by-step progress of the default hardware setup.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
pub struct SetupProgress {
    /// One-based current step.
    pub step: u32,
    /// Number of steps in the run.
    pub total: u32,
    /// What the current step does.
    pub label: String,
}

/// Outcome of a default hardware setup run.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
pub struct SetupResult {
    /// Settings that differed from the defaults and were changed.
    pub changes: Vec<String>,
    /// True when the run was a forced full re-apply.
    pub forced: bool,
    /// Failure detail. `None` on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

/// Phase of a backend-owned one-push white balance run.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum OnePushAwbPhase {
    /// Camera is adjusting gains after `onceAWB`.
    Running,
    /// Settled; clients must keep WB controls disabled briefly.
    Cooldown,
}

/// Shared one-push white balance status broadcast on [`CameraUiState`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
pub struct OnePushAwbStatus {
    /// Current phase of the run.
    pub phase: OnePushAwbPhase,
}

/// Partial camera state pushed to subscribed WebSocket clients.
#[derive(Debug, Clone, Serialize, Default, TS)]
pub struct CameraStateEvent {
    /// Camera this event belongs to.
    #[ts(as = "String")]
    pub camera_uuid: Uuid,
    /// Default actuator configuration, included on initial subscribe snapshots.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub actuators_default_config: Option<serde_json::Value>,
    /// Current actuator configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub actuators_config: Option<serde_json::Value>,
    /// Current actuator positions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub actuators_state: Option<serde_json::Value>,
    /// Whether actuators are configured for this camera.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub actuators_configured: Option<bool>,
    /// Current video encoder parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub video_parameters: Option<serde_json::Value>,
    /// Current base image parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub base_parameters: Option<serde_json::Value>,
    /// Current advanced image parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub advanced_parameters: Option<serde_json::Value>,
    /// Shared UI overlay state for this camera.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ui: Option<CameraUiState>,
}

/// Per-connection and aggregate bandwidth statistics for the connection icon tooltip.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ConnectionStats {
    /// Always true for live connections; false if the id is unknown.
    pub connected: bool,
    /// When this WebSocket connected.
    #[ts(type = "string")]
    pub since: DateTime<Utc>,
    /// Number of currently open WebSocket clients.
    pub clients_connected: usize,
    /// Minute-average upload kbps for this connection (client → server).
    pub this_upload_kbps: f64,
    /// Minute-average download kbps for this connection (server → client).
    pub this_download_kbps: f64,
    /// Minute-average upload kbps summed across all connections.
    pub total_upload_kbps: f64,
    /// Minute-average download kbps summed across all connections.
    pub total_download_kbps: f64,
}

/// Request-response call sent by a client over the WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WsRequest {
    /// Client-chosen correlation id echoed back in the [`WsResponse`].
    pub id: u32,
    /// HTTP-like verb, e.g. `GET` or `POST`.
    pub method: String,
    /// Route being called, e.g. `/camera/list`.
    pub path: String,
    /// Optional payload, required by `POST` routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub body: Option<Value>,
}

/// Fire-and-forget message sent by a client over the WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientMessage {
    /// Start receiving [`CameraStateEvent`]s for a camera.
    Subscribe {
        /// Camera to subscribe to.
        #[ts(as = "String")]
        camera_uuid: Uuid,
    },
    /// Stop receiving [`CameraStateEvent`]s for a camera.
    Unsubscribe {
        /// Camera to unsubscribe from.
        #[ts(as = "String")]
        camera_uuid: Uuid,
    },
    /// Clear one of the shared [`CameraUiState`] overlays.
    UiDismiss {
        /// Camera whose overlay should be cleared.
        #[ts(as = "String")]
        camera_uuid: Uuid,
        /// Overlay to clear.
        field: UiDismissField,
    },
}

/// Shared [`CameraUiState`] overlay a client can dismiss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum UiDismissField {
    /// The modal error dialog.
    ErrorDialog,
    /// The transient warning toast.
    WarningToast,
    /// The default hardware setup result.
    SetupResult,
}

/// Answer to a [`WsRequest`], correlated by `id`.
///
/// `response_type` is a `String` (not an enum) so the TypeScript binding can pin it
/// to the literal `"response"` via `ts_rs` without a tagged enum.
#[derive(Debug, Clone, Serialize, TS)]
pub struct WsResponse {
    /// Message discriminator, always `response`.
    #[serde(rename = "type")]
    #[ts(type = "\"response\"")]
    pub response_type: String,
    /// Correlation id copied from the [`WsRequest`].
    pub id: u32,
    /// HTTP-like status code.
    pub status: u16,
    /// Result payload, or an error description for non-2xx statuses.
    #[ts(type = "unknown")]
    pub body: Value,
}

/// Unsolicited server push, e.g. `camera/state` or `connection/stats`.
///
/// `event_type` is a `String` (not an enum) so the TypeScript binding can pin it
/// to the literal `"event"` via `ts_rs` without a tagged enum.
#[derive(Debug, Clone, Serialize, TS)]
pub struct WsEvent {
    /// Message discriminator, always `event`.
    #[serde(rename = "type")]
    #[ts(type = "\"event\"")]
    pub event_type: String,
    /// Event name, e.g. `camera/list`.
    pub event: String,
    /// Event payload.
    #[ts(type = "unknown")]
    pub body: Value,
}

impl WsResponse {
    /// Builds a `response` message for the given request id.
    pub fn new(id: u32, status: u16, body: Value) -> Self {
        Self {
            response_type: "response".to_string(),
            id,
            status,
            body,
        }
    }
}

impl WsEvent {
    /// Builds an `event` message for the given event name.
    pub fn new(event: impl Into<String>, body: Value) -> Self {
        Self {
            event_type: "event".to_string(),
            event: event.into(),
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_messages_keep_the_wire_shape() {
        let uuid = "5b1c6a6c-3f5e-4d1e-9a3f-8a7d0e2b1c34";

        let message: WsClientMessage =
            serde_json::from_str(&format!(r#"{{"type":"subscribe","camera_uuid":"{uuid}"}}"#))
                .unwrap();
        assert!(matches!(message, WsClientMessage::Subscribe { .. }));

        let message: WsClientMessage = serde_json::from_str(&format!(
            r#"{{"type":"unsubscribe","camera_uuid":"{uuid}"}}"#
        ))
        .unwrap();
        assert!(matches!(message, WsClientMessage::Unsubscribe { .. }));

        let message: WsClientMessage = serde_json::from_str(&format!(
            r#"{{"type":"ui_dismiss","camera_uuid":"{uuid}","field":"warning_toast"}}"#
        ))
        .unwrap();
        let WsClientMessage::UiDismiss { field, .. } = message else {
            panic!("expected a ui_dismiss message");
        };
        assert_eq!(field, UiDismissField::WarningToast);
    }

    #[test]
    fn requests_accept_a_missing_body() {
        let request: WsRequest =
            serde_json::from_str(r#"{"id":7,"method":"GET","path":"/camera/list"}"#).unwrap();
        assert!(request.body.is_none());
    }

    #[test]
    fn envelopes_keep_the_wire_shape() {
        assert_eq!(
            serde_json::to_string(&WsResponse::new(7, 200, Value::Null)).unwrap(),
            r#"{"type":"response","id":7,"status":200,"body":null}"#
        );
        assert_eq!(
            serde_json::to_string(&WsEvent::new("camera/state", Value::Null)).unwrap(),
            r#"{"type":"event","event":"camera/state","body":null}"#
        );
    }

    #[test]
    fn health_types_keep_the_wire_shape() {
        assert_eq!(
            serde_json::to_string(&CameraConnectivity::Online).unwrap(),
            r#""online""#
        );
        assert_eq!(
            serde_json::to_string(&McmHealth::Down).unwrap(),
            r#""down""#
        );
        assert_eq!(
            serde_json::to_string(&AutopilotHealth::Syncing).unwrap(),
            r#""syncing""#
        );

        let health = SystemHealth::default();
        let json: serde_json::Value = serde_json::to_value(&health).unwrap();
        assert!(json.get("mcm_detail").is_none());
        assert!(json.get("autopilot_detail").is_none());
        assert!(json.get("parameter_drifts").is_none());
        assert!(json["diagnostics"].get("settings_error").is_none());

        let ui = CameraUiState::default();
        let ui_json: serde_json::Value = serde_json::to_value(&ui).unwrap();
        assert_eq!(ui_json.get("connectivity").unwrap(), "unknown");
    }
}
