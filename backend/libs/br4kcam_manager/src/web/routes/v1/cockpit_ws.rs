//! `/cockpit/ws`: the cameras' state as Cockpit data-lake variables. Offered to Cockpit in
//! [`cockpit_extras.json`](super::cockpit); once the operator accepts it, Cockpit turns every
//! leaf of each JSON message into a variable named by its path, e.g.
//! `external/4k-cam/<camera uuid>/lens/focus`. `api/snapshots/cockpit_variables.snap` lists
//! them all. With `?live=true`, each online camera is also asked once a second what its image
//! pipeline is using now.
//!
//! Cockpit types a variable by its first value and keeps the last value it got. So each field
//! keeps one JSON type, and a field that stops applying (the shutter while auto exposure picks
//! it, say) is left out and keeps its last value: `exposure/mode` and `white_balance/mode` tell
//! which apply.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use axum::{
    extract::{
        Query, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use br4kcam_api::{
    AutopilotHealth, CameraConnectivity, LuaScriptStatus, McmHealth, SystemHealth,
    pro::{ExposureMode, MaxIso, ProState, WhiteBalance},
};
use futures::{SinkExt, StreamExt, stream::SplitSink};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast::error::RecvError, mpsc};
use tracing::*;
use uuid::Uuid;

use super::{
    pro,
    ws::{self, ConnectionGuard, SendFailure},
};
use crate::web::{
    self, camera_state, camera_ui, connectivity,
    ws_connections::{self, ConnectionId},
};

/// First segment of every variable, under Cockpit's own `external/`.
const ROOT: &str = "4k-cam";
const LIVE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize)]
pub struct Params {
    /// Also send what each camera's image pipeline is using now.
    #[serde(default)]
    live: bool,
}

/// One camera's variables.
#[derive(Debug, Serialize)]
struct CameraVariables {
    connectivity: CameraConnectivity,
    /// False while MCM cannot keep the camera's video stream up.
    stream_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    exposure: Option<Exposure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    white_balance: Option<WhiteBalance>,
    lens: Lens,
}

/// The exposure settings, in the units of the PRO API.
#[derive(Debug, Serialize)]
struct Exposure {
    mode: ExposureMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    shutter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iso: Option<u32>,
    /// The ISO, or `"full"`: a string either way, as Cockpit keeps a variable to one type.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_iso: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_shutter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compensation: Option<f32>,
}

/// Lens positions in the units of the PRO API, each left out when not set up.
#[derive(Debug, Serialize)]
struct Lens {
    #[serde(skip_serializing_if = "Option::is_none")]
    focus: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    zoom: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tilt: Option<f32>,
}

/// What the camera's image pipeline is using now, whoever picked it.
#[derive(Debug, Serialize)]
struct Live {
    #[serde(skip_serializing_if = "Option::is_none")]
    shutter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exposure_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iso: Option<u32>,
    /// How far auto exposure is off its target, positive when too dark. Left out in manual,
    /// where the camera reads a constant.
    #[serde(skip_serializing_if = "Option::is_none")]
    exposure_error: Option<i64>,
    /// The camera's estimate of the light's color temperature in kelvin, in every white
    /// balance mode. The white balance gains it applies cannot be read over HTTP.
    #[serde(skip_serializing_if = "Option::is_none")]
    color_temperature: Option<u32>,
}

#[derive(Debug, Serialize)]
struct SystemVariables {
    mcm: McmHealth,
    autopilot: AutopilotHealth,
    lua_script: LuaScriptStatus,
    lua_scripting_disabled: bool,
    cameras_discovered: usize,
    /// Cameras this install has set up that MCM is not listing.
    cameras_missing: usize,
    /// True when autopilot parameters no longer match the saved camera setup.
    parameters_drifted: bool,
}

/// Work done off the connection loop, ready to send.
enum Fetched {
    /// The camera's snapshot is cached.
    Camera(Uuid),
    Live(Uuid, Live),
}

/// Upgrade an HTTP connection to the Cockpit data-lake feed.
#[instrument(level = "debug")]
pub async fn websocket_handler(ws: WebSocketUpgrade, Query(params): Query<Params>) -> Response {
    ws.on_upgrade(move |socket| connection(socket, params.live))
}

#[instrument(level = "debug", skip_all, fields(connection_id = tracing::field::Empty, live))]
async fn connection(socket: WebSocket, live: bool) {
    let connection_id = ws_connections::register();
    Span::current().record("connection_id", connection_id);
    let _guard = ConnectionGuard(connection_id);
    let (sender, mut receiver) = socket.split();
    let mut publisher = Publisher {
        sender,
        connection_id,
        sent: HashMap::new(),
    };
    let mut cameras_receiver = mcm_client::subscribe_cameras();
    let mut state_receiver = camera_state::subscribe_state();
    let mut health_receiver = connectivity::subscribe();
    // Bounded by the cameras one connection may subscribe to, plus a live reading each.
    let (fetched_tx, mut fetched_rx) = mpsc::channel::<Fetched>(32);
    let mut last_pong_at = Instant::now();
    let mut ping_interval = tokio::time::interval(ws::PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping_interval.tick().await;
    let mut health_interval = tokio::time::interval(ws::HEALTH_INTERVAL);
    health_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    health_interval.tick().await;

    if live {
        tokio::spawn(poll_live(connection_id, fetched_tx.clone()).instrument(Span::current()));
    }
    subscribe_cameras(connection_id, &fetched_tx).await;
    if publisher.system().await.is_err() || publisher.cameras().await.is_err() {
        return;
    }

    loop {
        let sent = tokio::select! {
            _ = web::wait_shutdown() => {
                // Best-effort Close; peer may already be gone and we exit either way.
                let _ = tokio::time::timeout(
                    ws::WRITE_TIMEOUT,
                    publisher.sender.send(Message::Close(None)),
                )
                .await;
                break;
            }
            message = receiver.next() => match message {
                Some(Ok(Message::Pong(_))) => {
                    last_pong_at = Instant::now();
                    Ok(())
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                // Cockpit sends nothing else this needs.
                Some(Ok(_)) => Ok(()),
            },
            cameras = cameras_receiver.recv() => match cameras {
                Ok(()) | Err(RecvError::Lagged(_)) => {
                    subscribe_cameras(connection_id, &fetched_tx).await;
                    Ok(())
                }
                Err(RecvError::Closed) => break,
            },
            state = state_receiver.recv() => match state {
                Ok(event) if camera_state::connection_subscribed(connection_id, event.camera_uuid) => {
                    publisher.camera(event.camera_uuid).await
                }
                Ok(_) => Ok(()),
                Err(RecvError::Lagged(samples)) => {
                    camera_state::record_state_events_lagged(samples);
                    // The cache is ahead of the missed events, so it catches up.
                    publisher.cameras().await
                }
                Err(RecvError::Closed) => break,
            },
            health = health_receiver.recv() => match health {
                Ok(()) | Err(RecvError::Lagged(_)) => publisher.system().await,
                Err(RecvError::Closed) => break,
            },
            Some(fetched) = fetched_rx.recv() => match fetched {
                Fetched::Camera(camera_uuid) => publisher.camera(camera_uuid).await,
                Fetched::Live(camera_uuid, live) => publisher.live(camera_uuid, &live).await,
            },
            _ = ping_interval.tick() => {
                if last_pong_at.elapsed() > ws::PONG_TIMEOUT {
                    warn!("Cockpit WebSocket {connection_id} timed out waiting for pong");
                    break;
                }
                ws::send_message(&mut publisher.sender, connection_id, Message::Ping(vec![].into())).await
            }
            _ = health_interval.tick() => {
                // Same safety net as `/ws`, for when Cockpit is the only client.
                connectivity::publish_health_if_changed().await;
                Ok(())
            }
        };
        if sent.is_err() {
            break;
        }
    }
}

/// Subscribes to every camera MCM lists that this connection has not yet, and has their
/// snapshot fetched so their variables are complete from the start.
async fn subscribe_cameras(connection_id: ConnectionId, fetched_tx: &mpsc::Sender<Fetched>) {
    for camera_uuid in mcm_client::cameras().await.into_keys() {
        if camera_state::connection_subscribed(connection_id, camera_uuid) {
            continue;
        }
        if !camera_state::subscribe(connection_id, camera_uuid) {
            warn!(%connection_id, %camera_uuid, "Cockpit WebSocket is at the camera cap");
            continue;
        }
        let fetched_tx = fetched_tx.clone();
        tokio::spawn(
            async move {
                camera_state::snapshot(camera_uuid).await;
                // Closed once the connection is gone, with nothing left to tell.
                let _ = fetched_tx.send(Fetched::Camera(camera_uuid)).await;
            }
            .instrument(Span::current()),
        );
    }
}

/// Reads every online camera's image pipeline each [`LIVE_INTERVAL`], until the connection
/// is gone.
async fn poll_live(connection_id: ConnectionId, fetched_tx: mpsc::Sender<Fetched>) {
    let mut interval = tokio::time::interval(LIVE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    while !fetched_tx.is_closed() {
        interval.tick().await;
        for camera_uuid in camera_state::connection_cameras(connection_id) {
            if camera_ui::get(camera_uuid).connectivity != CameraConnectivity::Online {
                continue;
            }
            let Ok(info) = pro::isp_readings(camera_uuid).await else {
                continue;
            };
            let manual = pro::cached_state(&camera_state::cached_state_event(camera_uuid))
                .is_some_and(|state| state.mode == ExposureMode::Manual);
            if fetched_tx
                .send(Fetched::Live(camera_uuid, live_of(&info, manual)))
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

struct Publisher {
    sender: SplitSink<WebSocket, Message>,
    connection_id: ConnectionId,
    /// Last message sent under each key, so only changes go out.
    sent: HashMap<String, Value>,
}

impl Publisher {
    async fn publish(&mut self, key: String, message: Value) -> Result<(), SendFailure> {
        if self.sent.get(&key) == Some(&message) {
            return Ok(());
        }
        ws::send_text(&mut self.sender, self.connection_id, message.to_string()).await?;
        self.sent.insert(key, message);
        Ok(())
    }

    async fn camera(&mut self, camera_uuid: Uuid) -> Result<(), SendFailure> {
        let ui = camera_ui::get(camera_uuid);
        let variables = camera_variables(
            ui.connectivity,
            ui.stream_error.is_none(),
            pro::cached_state(&camera_state::cached_state_event(camera_uuid)).as_ref(),
        );
        self.publish(
            camera_uuid.to_string(),
            camera_message(camera_uuid, &variables),
        )
        .await
    }

    async fn cameras(&mut self) -> Result<(), SendFailure> {
        for camera_uuid in camera_state::connection_cameras(self.connection_id) {
            self.camera(camera_uuid).await?;
        }
        Ok(())
    }

    async fn live(&mut self, camera_uuid: Uuid, live: &Live) -> Result<(), SendFailure> {
        self.publish(
            format!("{camera_uuid}/live"),
            live_message(camera_uuid, live),
        )
        .await
    }

    async fn system(&mut self) -> Result<(), SendFailure> {
        let health = connectivity::system_health().await;
        self.publish(
            "system".to_string(),
            system_message(&system_variables(&health)),
        )
        .await
    }
}

fn camera_message(camera_uuid: Uuid, variables: &CameraVariables) -> Value {
    json!({ ROOT: { camera_uuid.to_string(): variables } })
}

fn live_message(camera_uuid: Uuid, live: &Live) -> Value {
    json!({ ROOT: { camera_uuid.to_string(): { "live": live } } })
}

fn system_message(variables: &SystemVariables) -> Value {
    json!({ ROOT: { "system": variables } })
}

fn camera_variables(
    connectivity: CameraConnectivity,
    stream_ok: bool,
    state: Option<&ProState>,
) -> CameraVariables {
    CameraVariables {
        connectivity,
        stream_ok,
        exposure: state.map(|state| Exposure {
            mode: state.mode,
            shutter: state.shutter.clone(),
            iso: state.iso,
            max_iso: state.max_iso.map(|max_iso| match max_iso {
                MaxIso::Iso(iso) => iso.to_string(),
                MaxIso::Full(_) => "full".to_string(),
            }),
            max_shutter: state.max_shutter.clone(),
            compensation: state.exposure_compensation,
        }),
        white_balance: state.map(|state| state.white_balance),
        lens: Lens {
            focus: state.and_then(|state| state.focus),
            zoom: state.and_then(|state| state.zoom),
            tilt: state.and_then(|state| state.tilt),
        },
    }
}

fn live_of(info: &HashMap<String, i64>, manual: bool) -> Live {
    let exposure_us = info
        .get("ExpTime")
        .and_then(|&us| u64::try_from(us).ok())
        .filter(|&us| us > 0);
    Live {
        shutter: exposure_us.map(pro::shutter_of_micros),
        exposure_us,
        iso: info
            .get("ISO")
            .and_then(|&iso| u64::try_from(iso).ok())
            .map(pro::iso_of_camera),
        exposure_error: info.get("HistError").copied().filter(|_| !manual),
        color_temperature: info.get("ColorTemp").and_then(|&k| u32::try_from(k).ok()),
    }
}

fn system_variables(health: &SystemHealth) -> SystemVariables {
    SystemVariables {
        mcm: health.mcm,
        autopilot: health.autopilot,
        lua_script: health.lua_script,
        lua_scripting_disabled: health.lua_scripting_disabled,
        cameras_discovered: health.cameras_discovered,
        cameras_missing: health.expected_missing.len(),
        parameters_drifted: health
            .parameter_drifts
            .as_ref()
            .is_some_and(|drifts| !drifts.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use br4kcam_api::pro::WhiteBalanceScene;

    use super::*;

    /// Cockpit's variables for `message`: `external/` and the path of each leaf, as
    /// `generic-websocket.ts` names them, with the type Cockpit gives it.
    fn variables(message: &Value) -> Vec<String> {
        fn walk(value: &Value, path: String, out: &mut Vec<String>) {
            match value {
                Value::Object(object) => object
                    .iter()
                    .for_each(|(key, value)| walk(value, format!("{path}/{key}"), out)),
                Value::String(_) => out.push(format!("{path}: string")),
                Value::Number(_) => out.push(format!("{path}: number")),
                Value::Bool(_) => out.push(format!("{path}: boolean")),
                Value::Null | Value::Array(_) => panic!("{path} is not a Cockpit variable"),
            }
        }
        let mut out = vec![];
        walk(message, "external".to_string(), &mut out);
        out
    }

    /// The names are what operators bind widgets and actions to: renaming one breaks them.
    #[test]
    fn cockpit_variables() {
        let camera_uuid = Uuid::nil();
        let state = ProState {
            mode: ExposureMode::Shutter,
            shutter: Some("1/250".to_string()),
            iso: Some(800),
            max_iso: Some(MaxIso::Iso(6400)),
            max_shutter: Some("1/30".to_string()),
            exposure_compensation: Some(0.0),
            exposure_warning: None,
            color_temperature: None,
            white_balance: WhiteBalance::Temperature {
                kelvin: Some(5600),
                scene: Some(WhiteBalanceScene::BlueWater),
            },
            focus: Some(50.0),
            zoom: Some(1.5),
            tilt: Some(-10.0),
        };
        let info = HashMap::from([
            ("ExpTime".to_string(), 4000),
            ("ISO".to_string(), 2089),
            ("HistError".to_string(), 12),
            ("ColorTemp".to_string(), 5847),
        ]);
        let health = SystemHealth {
            parameter_drifts: Some(vec![]),
            ..Default::default()
        };

        let mut all = [
            camera_message(
                camera_uuid,
                &camera_variables(CameraConnectivity::Online, true, Some(&state)),
            ),
            live_message(camera_uuid, &live_of(&info, false)),
            system_message(&system_variables(&health)),
        ]
        .iter()
        .flat_map(variables)
        .collect::<Vec<_>>();
        all.sort();

        insta::with_settings!({
            snapshot_path => "../../../../../../../api/snapshots",
            prepend_module_to_snapshot => false,
        }, {
            insta::assert_snapshot!("cockpit_variables", all.join("\n"));
        });
    }

    #[test]
    fn camera_without_a_cached_state_still_tells_its_connectivity() {
        let message = camera_message(
            Uuid::nil(),
            &camera_variables(CameraConnectivity::Unreachable, false, None),
        );
        assert_eq!(
            variables(&message),
            [
                "external/4k-cam/00000000-0000-0000-0000-000000000000/connectivity: string",
                "external/4k-cam/00000000-0000-0000-0000-000000000000/stream_ok: boolean",
            ]
        );
    }

    #[test]
    fn manual_drops_the_constant_exposure_error() {
        let info = HashMap::from([("HistError".to_string(), 50), ("ExpTime".to_string(), 0)]);
        let live = live_of(&info, true);
        assert_eq!(live.exposure_error, None);
        assert_eq!(live.shutter, None);
        assert_eq!(live_of(&info, false).exposure_error, Some(50));
    }
}
