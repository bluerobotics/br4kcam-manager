use autopilot::api::ActuatorsControl;
use axum::{
    Json, Router,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::*;

use crate::web::{control_bridge, request_restart};

pub mod blueos;
pub mod camera;
pub mod cockpit;
pub mod cockpit_ws;
pub mod health;
pub mod info;
pub mod log;
pub mod pro;
pub mod settings;
pub mod ws;

#[instrument(level = "trace")]
pub fn router() -> Router {
    Router::new()
        .nest("/camera", camera::router())
        .nest("/log", log::router())
        .nest("/info", info::router())
        .nest("/health", health::router())
        .nest("/settings", settings::router())
        .nest("/pro", pro::router())
        .nest(
            "/autopilot",
            Router::new().route("/control", post(autopilot_control)),
        )
        .route("/ws", get(ws::websocket_handler))
        .route("/service/restart", post(restart))
        .route("/register_service", get(blueos::server_metadata))
        .route("/cockpit_extras.json", get(cockpit::cockpit_extras))
        .route("/cockpit/ws", get(cockpit_ws::websocket_handler))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}

#[instrument(level = "debug")]
async fn autopilot_control(Json(actuators_control): Json<ActuatorsControl>) -> impl IntoResponse {
    match control_bridge::autopilot_control(actuators_control).await {
        Ok(value) => (StatusCode::OK, value.to_string()).into_response(),
        Err(error) => {
            let status =
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, error.message).into_response()
        }
    }
}

#[instrument(level = "debug")]
async fn restart() -> StatusCode {
    request_restart();
    StatusCode::OK
}
