//! Pins the public API to the contract kept in the repository's `api/` directory.
//!
//! Cockpit, BlueOS and third-party tools call this API, and Cockpit persists the
//! request bodies of its actions, so an accidental rename here breaks clients we
//! cannot update. See `api/README.md` for what may change and how.

use std::{
    any::TypeId,
    collections::{BTreeMap, HashSet},
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use autopilot::api::ActuatorsControl;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use br4kcam_api::{
    CameraStateEvent, ConnectionStats, SystemHealth, WsClientMessage, WsEvent, WsRequest,
    WsResponse, events,
};
use br4kcam_commands::CameraControl;
use br4kcam_manager::web::routes::{
    self,
    v1::{blueos, cockpit, info, ws},
};
use insta::{assert_debug_snapshot, assert_json_snapshot, assert_snapshot};
use mcm_client::{Camera, Cameras};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tower::ServiceExt;
use ts_rs::{TS, TypeVisitor};
use uuid::Uuid;

fn contract_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../api")
}

/// Keeps the snapshots next to the rest of the contract, in `api/snapshots/`.
fn contract_settings() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("../../../../api/snapshots");
    settings.set_prepend_module_to_snapshot(false);
    settings
}

/// The camera protocol enums travel as their discriminants (`serde_repr`), which ts-rs
/// renders as variant names. Every such enum is listed here and rewritten to the
/// numbers on the wire; `repr_enums_are_all_listed` keeps the list complete.
fn wire_numeric_enums() -> BTreeMap<String, String> {
    macro_rules! numeric {
        ($($ty:ty),* $(,)?) => {
            BTreeMap::from([$((<$ty as TS>::name(), numeric_union::<$ty>())),*])
        };
    }

    use br4kcam_commands::protocol::{
        display::{advanced_display::*, base_display::*},
        video::video_parameters::*,
    };

    numeric![
        VideoChannelValue,
        VideoEncodingProfileValue,
        VideoEncodeTypeValue,
        VideoRcModeValue,
        BaseMaxExposureValue,
        BaseAntiFogValue,
        BaseFrameTurboValue,
        BaseSceneModeValue,
        BaseAutoExposureStrategyModeValue,
        BaseExposureModeValue,
        BaseExposureTimeValue,
        BaseAutoWhiteBalanceModeValue,
        BaseAutoWhiteBalanceSceneValue,
        BaseAutoGainModeValue,
        BaseManualAGainEnableValue,
        BaseManualDGainEnableValue,
        BaseRotateValue,
        AdvancedDisplayMirrorValue,
        AdvancedDisplayFlipValue,
        AdvancedDisplayPowerFreqValue,
        AdvancedDisplayColorBlackValue,
        AdvancedDisplayInfrDetectModeValue,
        AdvancedDisplayLensCorrectionValue,
        AdvancedDisplayIRCUTLevelValue,
        AdvancedDisplayLDRLevelValue,
        AdvancedDisplayAntiflickerValue,
        AdvancedDisplaySceneModeValue,
        AdvancedDisplayHlcEnableValue,
        AdvancedDisplayLowFramerateValue,
        AdvancedDisplay2dNrLevelValue,
        AdvancedDisplayWDRSensorValue,
        AdvancedDisplayNoiseReductionValue,
        AdvancedDisplayAutoIrisValue,
        AdvancedDisplayLedControlValue,
        AdvancedDisplayLedControlAvailValue,
        AdvancedDisplayLightControlModeValue,
        AdvancedDisplayLampTypeValue,
    ]
}

/// `0 /* Variant */ | 1 /* Other */`, found by trying every value up to `u16::MAX`.
fn numeric_union<T: Serialize + DeserializeOwned + std::fmt::Debug>() -> String {
    (0..=u16::MAX)
        .filter_map(|value| serde_json::from_value::<T>(value.into()).ok())
        .map(|variant| {
            let wire = serde_json::to_value(&variant).unwrap();
            format!("{wire} /* {variant:?} */")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

#[test]
fn repr_enums_are_all_listed() {
    let listed = wire_numeric_enums();
    let repr_enum = regex::Regex::new(r"Serialize_repr[^{]*?pub enum (\w+)").unwrap();

    let mut sources =
        vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../br4kcam_commands/src")];
    let mut unlisted = vec![];
    while let Some(path) = sources.pop() {
        if path.is_dir() {
            sources.extend(
                fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        for captures in repr_enum.captures_iter(&source) {
            if !listed.contains_key(&captures[1]) {
                unlisted.push(captures[1].to_string());
            }
        }
    }

    assert!(
        unlisted.is_empty(),
        "Add these serde_repr enums to wire_numeric_enums(): {unlisted:?}"
    );
}

/// Collects the TypeScript declaration of a type and of every type it reaches.
struct Declarations {
    numeric_enums: BTreeMap<String, String>,
    seen: HashSet<TypeId>,
    by_name: BTreeMap<String, String>,
}

impl TypeVisitor for Declarations {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        if T::output_path().is_some() {
            let name = T::name();
            let declaration = match self.numeric_enums.get(&name) {
                Some(union) => format!("type {name} = {union};"),
                None => T::decl(),
            };
            self.by_name.insert(name, declaration);
        }
        T::visit_dependencies(self);
    }
}

/// Drops doc comments, which are not part of the contract, and breaks the members of
/// top-level object literals onto their own lines, so a snapshot diff points at the
/// field that changed instead of a whole declaration.
fn format_declaration(declaration: &str) -> String {
    let doc_comment = regex::Regex::new(r"(?s)/\*\*.*?\*/").unwrap();
    let declaration = doc_comment.replace_all(declaration, " ");
    let declaration = declaration.split_whitespace().collect::<Vec<_>>().join(" ");

    let mut formatted = String::new();
    let mut depth = 0usize;
    let mut characters = declaration.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '{' => {
                depth += 1;
                formatted.push('{');
                if depth == 1 {
                    formatted.push_str("\n    ");
                    while characters.next_if_eq(&' ').is_some() {}
                }
            }
            '}' => {
                if depth == 1 {
                    let trimmed = formatted.trim_end().len();
                    formatted.truncate(trimmed);
                    formatted.push('\n');
                }
                depth = depth.saturating_sub(1);
                formatted.push('}');
            }
            ',' if depth == 1 => {
                formatted.push_str(",\n    ");
                while characters.next_if_eq(&' ').is_some() {}
            }
            _ => formatted.push(character),
        }
    }

    formatted
}

/// TypeScript declarations of everything one entry point reaches. Each entry point is
/// its own snapshot, so types that share a name across crates (e.g. the camera and
/// autopilot `Action`s) never shadow each other.
fn declarations(
    numeric_enums: &BTreeMap<String, String>,
    visit: impl FnOnce(&mut Declarations),
) -> String {
    let mut declarations = Declarations {
        numeric_enums: numeric_enums.clone(),
        seen: HashSet::new(),
        by_name: BTreeMap::new(),
    };
    visit(&mut declarations);

    declarations
        .by_name
        .values()
        .map(|declaration| format!("export {}", format_declaration(declaration)))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[test]
fn wire_types() {
    let numeric_enums = wire_numeric_enums();

    contract_settings().bind(|| {
        assert_snapshot!(
            "types_camera_control",
            declarations(&numeric_enums, |types| types.visit::<CameraControl>())
        );
        assert_snapshot!(
            "types_camera_list",
            declarations(&numeric_enums, |types| types.visit::<Camera>())
        );
        assert_snapshot!(
            "types_autopilot_control",
            declarations(&numeric_enums, |types| types.visit::<ActuatorsControl>())
        );
        assert_snapshot!(
            "types_health",
            declarations(&numeric_enums, |types| types.visit::<SystemHealth>())
        );
        assert_snapshot!(
            "types_websocket",
            declarations(&numeric_enums, |types| {
                types.visit::<WsRequest>();
                types.visit::<WsResponse>();
                types.visit::<WsEvent>();
                types.visit::<WsClientMessage>();
                types.visit::<CameraStateEvent>();
                types.visit::<ConnectionStats>();
            })
        );
    });
}

fn test_camera() -> (Uuid, Camera) {
    let uuid = Uuid::parse_str("5b1c6a6c-3f5e-4d1e-9a3f-8a7d0e2b1c34").unwrap();
    let camera = Camera {
        uuid,
        hostname: Ipv4Addr::new(192, 168, 2, 10),
        credentials: None,
        streams: Default::default(),
    };

    (uuid, camera)
}

#[test]
fn cockpit_extras() {
    let cameras: Cameras = [test_camera()].into_iter().collect();

    contract_settings().bind(|| {
        assert_json_snapshot!("cockpit_extras", cockpit::extras(&cameras, false));
    });
}

#[test]
fn register_service() {
    contract_settings().bind(|| {
        assert_json_snapshot!(
            "register_service",
            blueos::ServerMetadata::default(),
            { ".version" => "[version]" }
        );
    });
}

#[tokio::test]
async fn info() {
    let info = info::info().await.0;

    contract_settings().bind(|| {
        assert_json_snapshot!("info", info, {
            ".service.version" => "[version]",
            ".service.sha" => "[sha]",
            ".service.build_date" => "[build_date]",
        });
    });
}

/// Whether every field `sent` carries survives into `understood`, so a renamed or
/// dropped field fails even though serde silently ignores unknown fields.
fn is_preserved(sent: &Value, understood: &Value) -> bool {
    match (sent, understood) {
        (Value::Object(sent), Value::Object(understood)) => sent.iter().all(|(key, value)| {
            understood
                .get(key)
                .is_some_and(|understood| is_preserved(value, understood))
        }),
        _ => sent == understood,
    }
}

/// Parses a recorded request as `T`, failing if it is rejected or loses a field.
fn parse_recorded<T: Serialize + DeserializeOwned>(path: &Path) -> T {
    let name = path.display();
    let sent: Value = serde_json::from_str(&fs::read_to_string(path).unwrap())
        .unwrap_or_else(|error| panic!("{name} is not valid JSON: {error}"));

    let parsed: T = serde_json::from_value(sent.clone())
        .unwrap_or_else(|error| panic!("{name} is no longer accepted: {error}"));
    let understood = serde_json::to_value(&parsed).unwrap();

    assert!(
        is_preserved(&sent, &understood),
        "{name} is accepted but loses fields.\n  sent:       {sent}\n  understood: {understood}"
    );

    parsed
}

/// Snapshots each recorded request in `api/requests/<kind>/` as the backend understands
/// it, so a reinterpreted value (e.g. a renumbered enum) shows up too.
///
/// Like `insta::glob!`, but without its process-wide collector, which a panicking
/// closure leaves behind and which then turns every later snapshot failure in the test
/// binary into a pass.
fn assert_recorded_requests<T: Serialize + DeserializeOwned + std::fmt::Debug>(kind: &str) {
    let dir = contract_dir().join("requests").join(kind);
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("Failed reading {dir:?}: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "No recorded requests in {dir:?}");

    let name = format!("{kind}_requests");
    for path in paths {
        let mut settings = contract_settings();
        settings.set_input_file(&path);
        settings.set_snapshot_suffix(path.file_name().unwrap().to_str().unwrap());
        settings.bind(|| {
            assert_debug_snapshot!(name.as_str(), parse_recorded::<T>(&path));
        });
    }
}

#[test]
fn camera_control_requests() {
    assert_recorded_requests::<CameraControl>("camera_control");
}

#[test]
fn autopilot_control_requests() {
    assert_recorded_requests::<ActuatorsControl>("autopilot_control");
}

#[test]
fn ws_client_message_requests() {
    assert_recorded_requests::<WsClientMessage>("ws_client_message");
}

/// Whether the HTTP router still serves `method path`, without running handlers
/// that change state.
async fn http_route_exists(method: &Method, path: &str) -> bool {
    let app = routes::router(1);

    if method == Method::GET {
        // GET handlers only read. A missing route falls through to the embedded
        // frontend, which answers 404.
        let request = Request::get(path).body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.unwrap();
        return response.status() != StatusCode::NOT_FOUND;
    }

    // Ask with a method the route does not serve: axum answers 405 and lists the
    // methods it does serve. A missing route only lists the frontend's GET.
    let probe = if method == Method::PATCH {
        Method::PUT
    } else {
        Method::PATCH
    };
    let request = Request::builder()
        .method(probe)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();

    response
        .headers()
        .get(header::ALLOW)
        .and_then(|allow| allow.to_str().ok())
        .is_some_and(|allow| {
            allow
                .split(',')
                .any(|allowed| allowed.trim() == method.as_str())
        })
}

/// Whether the WebSocket request dispatcher still serves `method path`. Requests are
/// sent without a body, so control routes reject them before doing anything.
async fn ws_route_exists(method: &str, path: &str) -> bool {
    let response = ws::handle_request(WsRequest {
        id: 0,
        method: method.to_string(),
        path: path.to_string(),
        body: None,
    })
    .await;

    response.status != StatusCode::NOT_FOUND.as_u16()
}

#[tokio::test]
async fn endpoints_still_exist() {
    let contract = fs::read_to_string(contract_dir().join("endpoints.txt")).unwrap();

    let mut missing = vec![];
    for line in contract.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let exists = match line.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["HTTP", method, path] => http_route_exists(&method.parse().unwrap(), path).await,
            ["WS", method, path] => ws_route_exists(method, path).await,
            ["EVENT", name] => events::ALL.contains(name),
            _ => panic!("Malformed line in api/endpoints.txt: {line:?}"),
        };

        if !exists {
            missing.push(line);
        }
    }

    assert!(
        missing.is_empty(),
        "Endpoints listed in api/endpoints.txt are gone; clients still use them:\n{}",
        missing.join("\n")
    );
}
