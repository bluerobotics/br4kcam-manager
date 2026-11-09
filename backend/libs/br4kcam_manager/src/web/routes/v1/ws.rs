use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use axum::{
    extract::{
        WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use br4kcam_api::{
    CameraStateEvent, SystemHealth, WsClientMessage, WsEvent, WsRequest, WsResponse, events,
};
use br4kcam_commands::CameraControl;
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{Notify, Semaphore, broadcast::error::RecvError, mpsc};
use tracing::*;
use uuid::Uuid;

use crate::web::ws_connections::ConnectionId;
use crate::web::{self, camera_state, camera_ui, connectivity, control_bridge, ws_connections};

pub(super) const PING_INTERVAL: Duration = Duration::from_secs(30);
/// Drop the connection when no pong has arrived within this wall-clock duration.
pub(super) const PONG_TIMEOUT: Duration = Duration::from_secs(90);
/// Bound socket writes so a half-open TCP peer cannot stall cleanup forever.
pub(super) const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const STATS_INTERVAL: Duration = Duration::from_secs(5);
pub(super) const HEALTH_INTERVAL: Duration = Duration::from_secs(15);
/// Outbound queue depth; the select loop drains it, so a full queue means a stuck client.
const RESPONSE_QUEUE: usize = 64;
/// Upper bound on concurrent request handlers per connection.
const MAX_INFLIGHT_REQUESTS: usize = 8;
/// Separate pool so subscribe snapshots cannot starve interactive requests (or vice versa).
const MAX_INFLIGHT_SNAPSHOTS: usize = 4;

/// Unregisters the connection on drop so panics cannot leak entries.
pub(super) struct ConnectionGuard(pub(super) ConnectionId);

/// Clears the lag-recovery flag when the recovery task ends or panics.
struct LagGuard {
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl LagGuard {
    fn new(flag: Arc<AtomicBool>) -> Self {
        Self { flag, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LagGuard {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(false, Ordering::SeqCst);
        }
    }
}

/// Why an outbound WebSocket message was not delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SendFailure {
    /// The payload could not be serialized; the socket is still usable.
    Serialize,
    /// The socket write failed; the connection is gone.
    Socket,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        cleanup(self.0);
    }
}

/// Upgrade an HTTP connection to the `/v1/ws` protocol.
#[instrument(level = "debug")]
pub async fn websocket_handler(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(websocket_connection)
}

#[instrument(level = "debug", skip_all, fields(connection_id = tracing::field::Empty))]
async fn websocket_connection(socket: WebSocket) {
    let connection_id = ws_connections::register();
    Span::current().record("connection_id", connection_id);
    let _guard = ConnectionGuard(connection_id);
    let (mut sender, mut receiver) = socket.split();
    let mut cameras_receiver = mcm_client::subscribe_cameras();
    let mut state_receiver = camera_state::subscribe_state();
    let mut health_receiver = connectivity::subscribe();
    let (response_tx, mut response_rx) = mpsc::channel::<String>(RESPONSE_QUEUE);
    let request_permits = Arc::new(Semaphore::new(MAX_INFLIGHT_REQUESTS));
    let snapshot_permits = Arc::new(Semaphore::new(MAX_INFLIGHT_SNAPSHOTS));
    let lag_recovering = Arc::new(AtomicBool::new(false));
    let lag_needs_resync = Arc::new(AtomicBool::new(false));
    let close_notify = Arc::new(Notify::new());
    let mut last_pong_at = Instant::now();
    let mut last_stats_text: Option<String> = None;
    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping_interval.tick().await;
    let mut stats_interval = tokio::time::interval(STATS_INTERVAL);
    stats_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    stats_interval.tick().await;
    let mut health_interval = tokio::time::interval(HEALTH_INTERVAL);
    health_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    health_interval.tick().await;

    if send_camera_list_event(&mut sender, connection_id).await == Err(SendFailure::Socket) {
        return;
    }

    if push_health(&mut sender, connection_id).await.is_err() {
        return;
    }

    if let Some(text) = connection_stats_text(connection_id) {
        last_stats_text = Some(text.clone());
        if send_text(&mut sender, connection_id, text).await.is_err() {
            return;
        }
    }

    'connection: loop {
        tokio::select! {
            _ = web::wait_shutdown() => {
                debug!("Closing WebSocket {connection_id} for shutdown");
                // Best-effort Close; peer may already be gone and we exit either way.
                let _ = tokio::time::timeout(
                    WRITE_TIMEOUT,
                    sender.send(Message::Close(None)),
                )
                .await;
                break 'connection;
            }
            _ = close_notify.notified() => {
                warn!("Closing WebSocket {connection_id}; outbound queue overflow");
                break 'connection;
            }
            message = receiver.next() => {
                let Some(message) = message else {
                    break 'connection;
                };

                match message {
                    Ok(message) => {
                        ws_connections::record_upload(connection_id, message_len(&message));

                        match message {
                            Message::Text(text) => {
                                handle_client_text(
                                    &text,
                                    connection_id,
                                    &response_tx,
                                    &request_permits,
                                    &snapshot_permits,
                                    &close_notify,
                                ).await;
                            }
                            // tungstenite answers inbound pings for us; track outbound pongs.
                            Message::Pong(_) => {
                                last_pong_at = Instant::now();
                            }
                            Message::Ping(_) => {}
                            Message::Close(_) => break 'connection,
                            _ => continue,
                        }
                    }
                    Err(error) => {
                        debug!("WebSocket receive error: {error:?}");
                        break 'connection;
                    }
                }
            }
            Some(text) = response_rx.recv() => {
                if send_text(&mut sender, connection_id, text).await.is_err() {
                    break 'connection;
                }
            }
            cameras = cameras_receiver.recv() => {
                match cameras {
                    Ok(()) | Err(RecvError::Lagged(_)) => {
                        if send_camera_list_event(&mut sender, connection_id).await == Err(SendFailure::Socket) {
                            break 'connection;
                        }
                    }
                    Err(RecvError::Closed) => {
                        warn!("Camera list broadcast closed; dropping WebSocket {connection_id}");
                        break 'connection;
                    }
                }
            }
            state = state_receiver.recv() => {
                match state {
                    Ok(event) => {
                        if !camera_state::connection_subscribed(connection_id, event.camera_uuid) {
                            continue;
                        }
                        // Route through the outbound queue so state cannot overtake a
                        // queued snapshot/UI push and resurrect a dismissed overlay.
                        let Some(text) = state_event_text(&event) else {
                            continue;
                        };
                        if !queue_text(connection_id, &response_tx, &close_notify, text) {
                            break 'connection;
                        }
                    }
                    Err(RecvError::Closed) => {
                        warn!("Camera state broadcast closed; dropping WebSocket {connection_id}");
                        break 'connection;
                    }
                    Err(RecvError::Lagged(samples)) => {
                        camera_state::record_state_events_lagged(samples);
                        debug!("WebSocket {connection_id} lagged by {samples} state events");
                        spawn_lag_recovery(
                            connection_id,
                            &response_tx,
                            &snapshot_permits,
                            &lag_recovering,
                            &lag_needs_resync,
                            &close_notify,
                        );
                    }
                }
            }
            _ = ping_interval.tick() => {
                if last_pong_at.elapsed() > PONG_TIMEOUT {
                    warn!("WebSocket {connection_id} timed out waiting for pong");
                    break 'connection;
                }
                if send_message(&mut sender, connection_id, Message::Ping(vec![].into())).await.is_err() {
                    break 'connection;
                }
            }
            _ = stats_interval.tick() => {
                let Some(text) = connection_stats_text(connection_id) else {
                    continue;
                };
                if last_stats_text.as_ref() == Some(&text) {
                    continue;
                }
                last_stats_text = Some(text.clone());
                if send_text(&mut sender, connection_id, text).await.is_err() {
                    break 'connection;
                }
            }
            _ = health_interval.tick() => {
                // Safety net for changes that reach no notifier; the aggregator
                // no-ops when only volatile ages moved, so clients see nothing.
                connectivity::publish_health_if_changed().await;
            }
            health = health_receiver.recv() => {
                match health {
                    Ok(()) | Err(RecvError::Lagged(_)) => {
                        if push_health(&mut sender, connection_id).await.is_err() {
                            break 'connection;
                        }
                    }
                    Err(RecvError::Closed) => {
                        warn!("System health broadcast closed; dropping WebSocket {connection_id}");
                        break 'connection;
                    }
                }
            }
        }
    }
}

#[instrument(level = "debug", skip_all, fields(%connection_id))]
fn cleanup(connection_id: ConnectionId) {
    camera_state::unsubscribe_connection(connection_id);
    ws_connections::unregister(connection_id);
}

/// Re-snapshot every subscribed camera after a dropped state event, at most once at a time.
#[instrument(level = "debug", skip_all, fields(%connection_id))]
fn spawn_lag_recovery(
    connection_id: ConnectionId,
    response_tx: &mpsc::Sender<String>,
    snapshot_permits: &Arc<Semaphore>,
    lag_recovering: &Arc<AtomicBool>,
    lag_needs_resync: &Arc<AtomicBool>,
    close_notify: &Arc<Notify>,
) {
    if lag_recovering.swap(true, Ordering::SeqCst) {
        lag_needs_resync.store(true, Ordering::SeqCst);
        return;
    }

    let response_tx = response_tx.clone();
    let close_notify = close_notify.clone();
    let snapshot_permits = snapshot_permits.clone();
    let lag_needs_resync = lag_needs_resync.clone();
    let lag_recovering = lag_recovering.clone();
    let mut lag_guard = LagGuard::new(lag_recovering.clone());
    tokio::spawn(
        async move {
            loop {
                lag_needs_resync.store(false, Ordering::SeqCst);
                for camera_uuid in camera_state::connection_cameras(connection_id) {
                    let Ok(permit) = snapshot_permits.clone().try_acquire_owned() else {
                        debug!(%connection_id, %camera_uuid, "No permit for lag-recovery snapshot; pushing actuators only");
                        // Slow watcher does not backfill actuators_state; push manager cache.
                        if let Some(event) =
                            camera_state::actuators_state_event(camera_uuid).await
                            && let Some(text) = state_event_text(&event)
                            && !queue_text(connection_id, &response_tx, &close_notify, text)
                        {
                            // Connection is dying: release recovering with the same
                            // hand-off as a clean exit so a concurrent Lagged cannot
                            // leave a sticky dirty bit.
                            lag_needs_resync.store(false, Ordering::SeqCst);
                            lag_recovering.store(false, Ordering::SeqCst);
                            lag_guard.disarm();
                            return;
                        }
                        continue;
                    };
                    let event = camera_state::snapshot(camera_uuid).await;
                    drop(permit);
                    if !camera_state::connection_subscribed(connection_id, camera_uuid) {
                        continue;
                    }
                    if let Some(text) = state_event_text(&event)
                        && !queue_text(connection_id, &response_tx, &close_notify, text)
                    {
                        lag_needs_resync.store(false, Ordering::SeqCst);
                        lag_recovering.store(false, Ordering::SeqCst);
                        lag_guard.disarm();
                        return;
                    }
                }
                if lag_needs_resync.load(Ordering::SeqCst) {
                    continue;
                }
                lag_recovering.store(false, Ordering::SeqCst);
                if !lag_needs_resync.swap(false, Ordering::SeqCst) {
                    lag_guard.disarm();
                    break;
                }
                if lag_recovering
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    continue;
                }
                lag_needs_resync.store(true, Ordering::SeqCst);
                lag_guard.disarm();
                break;
            }
        }
        .instrument(Span::current()),
    );
}

#[instrument(level = "debug", skip_all, fields(%connection_id))]
async fn handle_client_text(
    text: &str,
    connection_id: ConnectionId,
    response_tx: &mpsc::Sender<String>,
    request_permits: &Arc<Semaphore>,
    snapshot_permits: &Arc<Semaphore>,
    close_notify: &Arc<Notify>,
) {
    if let Ok(message) = serde_json::from_str::<WsClientMessage>(text) {
        match message {
            WsClientMessage::Subscribe { camera_uuid } => {
                if !connectivity::subscribe_allowed(camera_uuid, connection_id).await {
                    warn!(%connection_id, %camera_uuid, "Rejecting subscribe for unknown camera");
                    // close_notify already woken on Full; read loop exits via the notify arm.
                    let _ = queue_subscribe_rejected(
                        connection_id,
                        response_tx,
                        close_notify,
                        camera_uuid,
                        "unknown_camera",
                    );
                    return;
                }

                let already_subscribed =
                    camera_state::connection_subscribed(connection_id, camera_uuid);
                if !camera_state::subscribe(connection_id, camera_uuid) {
                    warn!(%connection_id, %camera_uuid, "Rejecting subscribe; at camera cap");
                    // close_notify already woken on Full; read loop exits via the notify arm.
                    let _ = queue_subscribe_rejected(
                        connection_id,
                        response_tx,
                        close_notify,
                        camera_uuid,
                        "camera_cap",
                    );
                    return;
                }

                // Always re-push UI so late remounts see in-flight overlays.
                let ui_event = CameraStateEvent {
                    camera_uuid,
                    ui: Some(camera_ui::get(camera_uuid)),
                    ..Default::default()
                };
                if let Some(text) = state_event_text(&ui_event)
                    && !queue_text(connection_id, response_tx, close_notify, text)
                {
                    return;
                }

                // Already subscribed: serve the cache instead of hammering the camera.
                if already_subscribed {
                    if let Some(text) =
                        state_event_text(&camera_state::cached_state_event(camera_uuid))
                    {
                        // close_notify already woken on Full; nothing left to unwind here.
                        let _ = queue_text(connection_id, response_tx, close_notify, text);
                    }
                    return;
                }

                // Never park on the permit — subscribe/unsubscribe churn must not pile up
                // tasks. The slow watcher fills params; actuators need an explicit push.
                let Ok(permit) = snapshot_permits.clone().try_acquire_owned() else {
                    debug!(%connection_id, %camera_uuid, "No permit for subscribe snapshot; pushing actuators only");
                    if let Some(event) = camera_state::actuators_state_event(camera_uuid).await
                        && let Some(text) = state_event_text(&event)
                    {
                        // close_notify already woken on Full; nothing left to unwind here.
                        let _ = queue_text(connection_id, response_tx, close_notify, text);
                    }
                    return;
                };

                let response_tx = response_tx.clone();
                let close_notify = close_notify.clone();
                tokio::spawn(
                    async move {
                        let _permit = permit;
                        let event = camera_state::snapshot(camera_uuid).await;
                        if !camera_state::connection_subscribed(connection_id, camera_uuid) {
                            return;
                        }
                        if let Some(text) = state_event_text(&event) {
                            // close_notify already woken on Full; nothing left to unwind here.
                            let _ = queue_text(connection_id, &response_tx, &close_notify, text);
                        }
                    }
                    .instrument(Span::current()),
                );
            }
            WsClientMessage::Unsubscribe { camera_uuid } => {
                camera_state::unsubscribe(connection_id, camera_uuid);
            }
            WsClientMessage::UiDismiss { camera_uuid, field } => {
                if !camera_state::connection_subscribed(connection_id, camera_uuid) {
                    debug!(
                        %connection_id,
                        %camera_uuid,
                        "Ignoring ui_dismiss for unsubscribed camera"
                    );
                    return;
                }
                camera_ui::dismiss(camera_uuid, field);
            }
        }
        return;
    }

    let request = match serde_json::from_str::<WsRequest>(text) {
        Ok(request) => request,
        Err(error) => {
            warn!("Invalid WebSocket request: {error:?}");
            // Best-effort: if an id is present, answer with 400 so the client does not hang.
            // close_notify already woken on Full; read loop exits via the notify arm.
            if let Ok(value) = serde_json::from_str::<Value>(text)
                && let Some(id) = value.get("id").and_then(Value::as_u64)
                && let Ok(id) = u32::try_from(id)
            {
                let _ = queue_response(
                    connection_id,
                    response_tx,
                    close_notify,
                    &WsResponse::new(id, 400, Value::String(format!("{error:?}"))),
                );
            }
            return;
        }
    };

    // Backpressure: refuse instead of blocking the read loop or spawning unbounded handlers.
    let Ok(permit) = request_permits.clone().try_acquire_owned() else {
        // close_notify already woken on Full; read loop exits via the notify arm.
        let _ = queue_response(
            connection_id,
            response_tx,
            close_notify,
            &WsResponse::new(
                request.id,
                503,
                Value::String("too many in-flight requests".to_string()),
            ),
        );
        return;
    };
    let response_tx = response_tx.clone();
    let close_notify = close_notify.clone();
    tokio::spawn(
        async move {
            let response = handle_request(request).await;
            // close_notify already woken on Full; nothing left to unwind here.
            let _ = queue_response(connection_id, &response_tx, &close_notify, &response);
            drop(permit);
        }
        .instrument(Span::current()),
    );
}

#[instrument(level = "debug", skip_all, fields(%connection_id))]
fn queue_response(
    connection_id: ConnectionId,
    response_tx: &mpsc::Sender<String>,
    close_notify: &Notify,
    response: &WsResponse,
) -> bool {
    match serde_json::to_string(response) {
        Ok(text) => queue_text(connection_id, response_tx, close_notify, text),
        Err(error) => {
            warn!(%connection_id, "Failed to serialize WebSocket response: {error:?}");
            true
        }
    }
}

/// Enqueues an outbound text frame. Returns false when the connection should close.
#[instrument(level = "debug", skip_all, fields(%connection_id))]
fn queue_text(
    connection_id: ConnectionId,
    response_tx: &mpsc::Sender<String>,
    close_notify: &Notify,
    text: String,
) -> bool {
    match response_tx.try_send(text) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            warn!(%connection_id, "Outbound WebSocket queue is full; closing connection");
            // Store a permit so a race with the select loop still wakes the closer.
            close_notify.notify_one();
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            debug!(%connection_id, "Dropping WebSocket message; connection already closed");
            false
        }
    }
}

#[instrument(level = "debug", skip_all, fields(%event.camera_uuid))]
fn state_event_text(event: &CameraStateEvent) -> Option<String> {
    let body = match serde_json::to_value(event) {
        Ok(body) => body,
        Err(error) => {
            warn!("Failed serializing camera/state: {error}");
            return None;
        }
    };
    match serde_json::to_string(&WsEvent::new(events::CAMERA_STATE, body)) {
        Ok(text) => Some(text),
        Err(error) => {
            warn!("Failed serializing camera/state event: {error}");
            None
        }
    }
}

#[instrument(level = "debug", skip_all, fields(%connection_id))]
async fn send_camera_list_event(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    connection_id: ConnectionId,
) -> Result<(), SendFailure> {
    let cameras = mcm_client::cameras().await;
    let body = match serde_json::to_value(cameras) {
        Ok(body) => body,
        Err(error) => {
            warn!("Failed serializing camera list: {error}");
            return Err(SendFailure::Serialize);
        }
    };
    let text = match serde_json::to_string(&WsEvent::new(events::CAMERA_LIST, body)) {
        Ok(text) => text,
        Err(error) => {
            warn!("Failed serializing camera/list event: {error}");
            return Err(SendFailure::Serialize);
        }
    };
    send_text(sender, connection_id, text).await
}

#[instrument(level = "debug", skip_all, fields(%connection_id))]
fn connection_stats_text(connection_id: ConnectionId) -> Option<String> {
    let stats = ws_connections::snapshot(connection_id);
    let body = match serde_json::to_value(&stats) {
        Ok(body) => body,
        Err(error) => {
            warn!("Failed serializing connection stats: {error}");
            return None;
        }
    };
    match serde_json::to_string(&WsEvent::new(events::CONNECTION_STATS, body)) {
        Ok(text) => Some(text),
        Err(error) => {
            warn!("Failed serializing connection/stats event: {error}");
            None
        }
    }
}

#[instrument(level = "debug", skip_all)]
fn system_health_event_text(health: &SystemHealth) -> Option<String> {
    let body = match serde_json::to_value(health) {
        Ok(body) => body,
        Err(error) => {
            warn!("Failed serializing system health: {error}");
            return None;
        }
    };
    match serde_json::to_string(&WsEvent::new(events::SYSTEM_HEALTH, body)) {
        Ok(text) => Some(text),
        Err(error) => {
            warn!("Failed serializing system/health event: {error}");
            None
        }
    }
}

#[instrument(level = "debug", skip_all, fields(%connection_id))]
async fn push_health(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    connection_id: ConnectionId,
) -> Result<(), SendFailure> {
    let health = connectivity::system_health().await;
    let Some(text) = system_health_event_text(&health) else {
        return Ok(());
    };
    send_text(sender, connection_id, text).await
}

#[instrument(level = "debug", skip_all, fields(%connection_id, %camera_uuid, reason))]
fn queue_subscribe_rejected(
    connection_id: ConnectionId,
    response_tx: &mpsc::Sender<String>,
    close_notify: &Notify,
    camera_uuid: Uuid,
    reason: &str,
) -> bool {
    let body = serde_json::json!({
        "camera_uuid": camera_uuid,
        "reason": reason,
    });
    match serde_json::to_string(&WsEvent::new(events::CAMERA_SUBSCRIBE_REJECTED, body)) {
        Ok(text) => queue_text(connection_id, response_tx, close_notify, text),
        Err(error) => {
            warn!(%connection_id, "Failed serializing subscribe_rejected: {error:?}");
            true
        }
    }
}

#[instrument(level = "debug", skip_all, fields(%connection_id, bytes = text.len()))]
pub(super) async fn send_text(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    connection_id: ConnectionId,
    text: String,
) -> Result<(), SendFailure> {
    let bytes = text.len() as u64;
    match tokio::time::timeout(WRITE_TIMEOUT, sender.send(Message::Text(text.into()))).await {
        Ok(Ok(())) => {
            ws_connections::record_download(connection_id, bytes);
            Ok(())
        }
        Ok(Err(error)) => {
            debug!("WebSocket text send failed for {connection_id}: {error}");
            Err(SendFailure::Socket)
        }
        Err(_) => {
            debug!("WebSocket text send timed out for {connection_id}");
            Err(SendFailure::Socket)
        }
    }
}

#[instrument(level = "debug", skip_all, fields(%connection_id))]
pub(super) async fn send_message(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    connection_id: ConnectionId,
    message: Message,
) -> Result<(), SendFailure> {
    let bytes = message_len(&message);
    match tokio::time::timeout(WRITE_TIMEOUT, sender.send(message)).await {
        Ok(Ok(())) => {
            ws_connections::record_download(connection_id, bytes);
            Ok(())
        }
        Ok(Err(error)) => {
            debug!("WebSocket message send failed for {connection_id}: {error}");
            Err(SendFailure::Socket)
        }
        Err(_) => {
            debug!("WebSocket message send timed out for {connection_id}");
            Err(SendFailure::Socket)
        }
    }
}

fn message_len(message: &Message) -> u64 {
    match message {
        Message::Text(text) => text.len() as u64,
        Message::Binary(data) => data.len() as u64,
        Message::Ping(data) | Message::Pong(data) => data.len() as u64,
        Message::Close(_) => 0,
    }
}

#[instrument(level = "debug", skip_all, fields(id = request.id, method = %request.method, path = %request.path))]
pub async fn handle_request(request: WsRequest) -> WsResponse {
    let id = request.id;

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/camera/list") => match serde_json::to_value(mcm_client::cameras().await) {
            Ok(body) => WsResponse::new(id, 200, body),
            Err(error) => WsResponse::new(id, 500, Value::String(format!("{error:?}"))),
        },
        ("POST", "/camera/control") => {
            let Some(body) = request.body else {
                return WsResponse::new(id, 400, Value::String("missing body".to_string()));
            };

            let camera_control = match serde_json::from_value::<CameraControl>(body) {
                Ok(camera_control) => camera_control,
                Err(error) => {
                    return WsResponse::new(id, 400, Value::String(format!("{error:?}")));
                }
            };

            match control_bridge::camera_control(camera_control).await {
                Ok(value) => WsResponse::new(id, 200, value),
                Err(error) => WsResponse::new(id, error.status, Value::String(error.message)),
            }
        }
        ("POST", "/autopilot/control") => {
            let Some(body) = request.body else {
                return WsResponse::new(id, 400, Value::String("missing body".to_string()));
            };

            let actuators_control =
                match serde_json::from_value::<autopilot::api::ActuatorsControl>(body) {
                    Ok(actuators_control) => actuators_control,
                    Err(error) => {
                        return WsResponse::new(id, 400, Value::String(format!("{error:?}")));
                    }
                };

            match control_bridge::autopilot_control(actuators_control).await {
                Ok(value) => WsResponse::new(id, 200, value),
                Err(error) => WsResponse::new(id, error.status, Value::String(error.message)),
            }
        }
        (method, path) => match super::pro::ws_request(method, path, request.body).await {
            Some((status, body)) => WsResponse::new(id, status, body),
            None => WsResponse::new(id, 404, Value::String("not found".to_string())),
        },
    }
}
