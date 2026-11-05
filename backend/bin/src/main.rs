use anyhow::Result;
use tracing::*;

use br4kcam_manager::{
    logger,
    web::{self, ShutdownReason},
};

pub mod cli;

async fn start_application(first_start: bool) -> Result<bool> {
    cli::init();

    logger::init(cli::log_path(), cli::is_verbose(), cli::is_tracing());

    autopilot::set_backend_version(format!(
        "{}-{}",
        env!("CARGO_PKG_VERSION"),
        option_env!("VERGEN_GIT_SHA").unwrap_or("?"),
    ));

    info!(
        "{}, version: {}-{}, build date: {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        option_env!("VERGEN_GIT_SHA").unwrap_or("?"),
        env!("VERGEN_BUILD_DATE"),
    );
    info!(
        "Starting at {}",
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
    );
    debug!("Command line call: {}", cli::command_line_string());
    debug!("Command line input struct call: {}", cli::command_line());

    settings::init(cli::settings_file(), first_start && cli::is_reset())
        .await
        .unwrap();

    let mcm_client_startup_task = tokio::spawn(mcm_client::init(
        cli::mcm_address().await,
        cli::mcm_skip_hardware_check(),
    ));

    blueos_client::init(cli::blueos_address().await).await;

    br4kcam_commands::firmware::init(cli::firmware_server_url());

    let autopilot_startup_task = tokio::spawn(async move {
        let mut endpoint_failures = 0u32;
        loop {
            match blueos_client::create_mavlink_endpoint(&cli::mavlink_connection_string().await)
                .await
            {
                Ok(()) => {
                    autopilot::report_endpoint_setup(true, None);
                    info!("Successfully created MAVLink endpoint!");
                    break;
                }
                Err(error) => {
                    let detail = format!("{error:?}");
                    autopilot::report_endpoint_setup(false, Some(detail.clone()));
                    if endpoint_failures == 0 {
                        error!("Failed creating MAVLink Endpoint: {detail}");
                    } else {
                        warn!("Failed creating MAVLink Endpoint: {detail}");
                    }
                    endpoint_failures += 1;
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }

        // BlueOS users can delete our endpoint at runtime; re-create it while
        // MAVLink is down so traffic can resume without restarting this process.
        tokio::spawn(async move {
            let mut warned = false;
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                if !autopilot::needs_mavlink_endpoint_ensure() {
                    warned = false;
                    continue;
                }
                match blueos_client::ensure_mavlink_endpoint(
                    &cli::mavlink_connection_string().await,
                )
                .await
                {
                    Ok(changed) => {
                        autopilot::report_endpoint_setup(true, None);
                        if changed {
                            info!("Re-created BlueOS MAVLink endpoint");
                        }
                        warned = false;
                    }
                    Err(error) => {
                        let detail = format!("{error:?}");
                        autopilot::report_endpoint_setup(false, Some(detail.clone()));
                        if !warned {
                            warn!("Failed re-ensuring MAVLink endpoint: {detail}");
                            warned = true;
                        }
                    }
                }
            }
        });

        loop {
            if let Err(error) = autopilot::init(
                cli::autopilot_scripts_file(),
                cli::mavlink_connection_string().await,
                cli::mavlink_system_id(),
                cli::mavlink_component_id(),
            )
            .await
            {
                error!("Failed initializing autopilot: {error:?}");
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                continue;
            }

            info!("Successfully started autopilot client!");

            break;
        }
    });

    let shutdown_reason = web::run(cli::web_server().await, cli::default_api_version()).await;

    autopilot_startup_task.abort();
    mcm_client_startup_task.abort();
    mcm_client::shutdown().await;
    autopilot::shutdown_actuators_stream().await;

    if shutdown_reason == ShutdownReason::Signal {
        return Ok(false);
    }

    Ok(true)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let mut first_start = true;

    while start_application(first_start).await? {
        first_start = false;
    }

    Ok(())
}
