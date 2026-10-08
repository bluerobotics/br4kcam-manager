use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use autopilot::api;
use regex::Regex;
use ts_rs::TS;

use br4kcam_commands::{
    Action, CameraControl,
    protocol::display::{
        advanced_display::AdvancedParameterSetting, base_display::BaseParameterSetting,
    },
};
use mcm_client::{Camera, Stream, mcm_types};

fn main() -> Result<()> {
    if let Err(error) = generate_typescript_bindings_for_mcm_client() {
        println!("Failed generating typecript bindings for MCM: {error:?}");
        return Err(error);
    }

    if let Err(error) = generate_typescript_bindings_for_autopilot() {
        println!("Failed generating typecript bindings for Autopilot: {error:?}");
        return Err(error);
    }

    if let Err(error) = generate_typescript_bindings_for_br4kcam() {
        println!("Failed generating typecript bindings for 4K Cam: {error:?}");
        return Err(error);
    }

    if let Err(error) = generate_typescript_bindings_for_br4kcam_api() {
        println!("Failed generating typescript bindings for 4K Cam API: {error:?}");
        return Err(error);
    }

    println!("Typescript bindings successifully generated!");

    Ok(())
}

fn generate_typescript_bindings_for_mcm_client() -> Result<()> {
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");

    let inputs = vec![root_dir.join("backend/libs/mcm_client")];
    let output = root_dir.join("frontend/src/bindings/mcm_client.d.ts");

    prepare_output(&output)?;

    let tsync_bindings = {
        tsync::generate_typescript_defs(inputs, output.clone(), false, false);

        fs::read_to_string(&output)?
            .replace("declare ", "export ")
            .replace("type ", "export type ")
            .replace("interface ", "export interface ")
    };

    // Generate all typescript bindings and join them into a single String
    let ts_rs_bindings = [
        Camera::export_to_string()?,
        Stream::export_to_string()?,
        mcm_types::VideoEncodeType::export_to_string()?,
        mcm_types::CaptureConfiguration::export_to_string()?,
        mcm_types::VideoSourceType::export_to_string()?,
        mcm_types::VideoSourceOnvifType::export_to_string()?,
        mcm_types::Info::export_to_string()?,
        mcm_types::ApiVideoSource::export_to_string()?,
        mcm_types::Format::export_to_string()?,
        mcm_types::Size::export_to_string()?,
        mcm_types::FrameInterval::export_to_string()?,
        mcm_types::StreamInformation::export_to_string()?,
        mcm_types::ExtendedConfiguration::export_to_string()?,
        mcm_types::VideoCaptureConfiguration::export_to_string()?,
        mcm_types::RedirectCaptureConfiguration::export_to_string()?,
        mcm_types::StreamStatus::export_to_string()?,
        mcm_types::StreamStatusState::export_to_string()?,
        mcm_types::VideoAndStreamInformation::export_to_string()?,
        mcm_types::VideoSourceOnvif::export_to_string()?,
        mcm_types::VideoSourceLocal::export_to_string()?,
        mcm_types::VideoSourceGst::export_to_string()?,
        mcm_types::VideoSourceRedirect::export_to_string()?,
        mcm_types::OnvifDeviceInformation::export_to_string()?,
        mcm_types::PostStream::export_to_string()?,
        mcm_types::RemoveStream::export_to_string()?,
        mcm_types::OnvifDevice::export_to_string()?,
        mcm_types::AuthenticateOnvifDeviceRequest::export_to_string()?,
        mcm_types::UnauthenticateOnvifDeviceRequest::export_to_string()?,
    ]
    .join("\n\n");

    write_bindings(&output, ts_rs_bindings + &tsync_bindings)
}

fn generate_typescript_bindings_for_autopilot() -> Result<()> {
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");

    let inputs = vec![root_dir.join("backend/libs/autopilot")];
    let output = root_dir.join("frontend/src/bindings/autopilot.d.ts");

    prepare_output(&output)?;

    let tsync_bindings = {
        tsync::generate_typescript_defs(inputs, output.clone(), false, false);

        fs::read_to_string(&output)?
            .replace("declare ", "export ")
            .replace("type ", "export type ")
            .replace("interface ", "export interface ")
    };

    // Generate all typescript bindings and join them into a single String
    let ts_rs_bindings = [
        api::ActuatorsControl::export_to_string()?,
        api::Action::export_to_string()?,
        api::ActuatorsState::export_to_string()?,
        api::ActuatorsConfig::export_to_string()?,
        api::ActuatorsParametersConfig::export_to_string()?,
        api::ServoChannel::export_to_string()?,
        api::MountType::export_to_string()?,
        api::CameraID::export_to_string()?,
        api::ScriptFunction::export_to_string()?,
        api::FocusZoomPoints::export_to_string()?,
        api::FocusZoomPoint::export_to_string()?,
    ]
    .join("\n\n");

    write_bindings(&output, ts_rs_bindings + &tsync_bindings)
}

fn generate_typescript_bindings_for_br4kcam() -> Result<()> {
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");

    let inputs = vec![root_dir.join("backend/libs/br4kcam_commands")];
    let output = root_dir.join("frontend/src/bindings/br4kcam.ts");

    prepare_output(&output)?;

    let tsync_bindings = {
        tsync::generate_typescript_defs(inputs, output.clone(), false, false);

        fs::read_to_string(&output)?
            .replace("declare ", "export ")
            .replace("type ", "export type ")
    };

    // Generate all typescript bindings and join them into a single String
    let ts_rs_bindings = [
        CameraControl::export_to_string()?,
        Action::export_to_string()?,
        BaseParameterSetting::export_to_string()?,
        AdvancedParameterSetting::export_to_string()?,
    ]
    .join("\n\n");

    write_bindings(&output, ts_rs_bindings + &tsync_bindings)
}

fn generate_typescript_bindings_for_br4kcam_api() -> Result<()> {
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");
    let output = root_dir.join("frontend/src/bindings/br4kcam_api.d.ts");

    prepare_output(&output)?;

    let ts_rs_bindings = [
        br4kcam_api::CameraConnectivity::export_to_string()?,
        br4kcam_api::McmHealth::export_to_string()?,
        br4kcam_api::AutopilotHealth::export_to_string()?,
        br4kcam_api::LuaScriptStatus::export_to_string()?,
        br4kcam_api::ExpectedCamera::export_to_string()?,
        br4kcam_api::ParameterDrift::export_to_string()?,
        br4kcam_api::Diagnostics::export_to_string()?,
        br4kcam_api::SystemHealth::export_to_string()?,
        br4kcam_api::CameraStateEvent::export_to_string()?,
        br4kcam_api::CameraUiState::export_to_string()?,
        br4kcam_api::SetupProgress::export_to_string()?,
        br4kcam_api::SetupResult::export_to_string()?,
        br4kcam_api::OnePushAwbStatus::export_to_string()?,
        br4kcam_api::OnePushAwbPhase::export_to_string()?,
        br4kcam_api::ConnectionStats::export_to_string()?,
        br4kcam_api::WsRequest::export_to_string()?,
        br4kcam_api::WsClientMessage::export_to_string()?,
        br4kcam_api::UiDismissField::export_to_string()?,
        br4kcam_api::WsResponse::export_to_string()?,
        br4kcam_api::WsEvent::export_to_string()?,
    ]
    .join("\n\n");

    write_bindings(&output, ts_rs_bindings)
}

/// Removes a stale bindings file, creating its parent directory when missing.
fn prepare_output(output: &Path) -> Result<()> {
    match fs::remove_file(output) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed removing stale bindings at {output:?}"));
        }
    }

    if let Some(output_dir) = output.parent() {
        fs::create_dir_all(output_dir)
            .with_context(|| format!("Failed creating bindings directory {output_dir:?}"))?;
    }

    Ok(())
}

/// Strips per-type imports, replaces the generator notices by a custom one, and writes the file.
fn write_bindings(output: &Path, bindings: String) -> Result<()> {
    // Remove all typescript "import type" because all types are going to live in the same typescript file
    let re = Regex::new(r"(?m)^import type .*\n")?;
    let bindings = re.replace_all(bindings.as_str(), "").to_string();

    let re = Regex::new(r".*This file.*")?;
    let mut bindings = re.replace_all(bindings.as_str(), "\n").to_string();

    let custom_notice_str = "/* This file was generated using `cargo run --bin=bindings`. Do not edit this file manually. */\n";
    bindings.insert_str(0, custom_notice_str);

    fs::write(output, bindings.as_str().replace("\n\n\n", "\n"))?;

    println!(
        "Successfully wrote {:?}",
        output
            .canonicalize()
            .unwrap_or_else(|_| output.to_path_buf())
    );

    Ok(())
}
