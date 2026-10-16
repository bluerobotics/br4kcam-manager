use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow};
use indexmap::IndexMap;
use tracing::*;

use crate::mcm_types::{
    ApiVideoSource, AuthenticateOnvifDeviceRequest, CaptureConfiguration, Format, Info,
    OnvifDevice, OnvifDeviceInformation, PostStream, RemoveStream, StreamInformation, StreamStatus,
    VideoCaptureConfiguration, VideoEncodeType, VideoSourceOnvif, VideoSourceOnvifType,
    VideoSourceType,
};

use super::{Camera, Credentials, Stream};

const KNOWN_BR4KCAM_HARDWARE: &[&str] = &["HW0100302", "HW20200610"];

/// MCM ships as its own BlueOS extension on its own release cadence, so this must not
/// be a caret range: `"0.2.4"` means `>=0.2.4, <0.3.0`, which locks every camera out
/// the day MCM releases 0.3.0.
const SUPPORTED_MCM_VERSIONS: &str = ">=0.2.4";

pub struct MCMClient {
    pub address: SocketAddr,
    skip_hardware_check: bool,
    _info: Info,
}

impl MCMClient {
    #[instrument(level = "debug")]
    pub async fn try_new(address: &SocketAddr, skip_hardware_check: bool) -> Result<Self> {
        let _info = Self::get_info(address).await?;

        let version = semver::Version::parse(&_info.version)?;
        let supported = semver::VersionReq::parse(SUPPORTED_MCM_VERSIONS)?;

        if !supported.matches(&version) {
            return Err(anyhow!(
                "Version {version:?} is not supported. Supported: {supported:?}"
            ));
        }

        Ok(Self {
            address: *address,
            skip_hardware_check,
            _info,
        })
    }

    #[instrument(level = "debug")]
    async fn get_info(address: &SocketAddr) -> Result<Info> {
        web_client::get(address, "info", (), ()).await
    }

    #[instrument(level = "debug", skip(self))]
    async fn get_onvif_devices(&self) -> Result<Vec<OnvifDevice>> {
        web_client::get(&self.address, "onvif/devices", (), ()).await
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn get_br4kcams(&self) -> Result<Vec<Camera>> {
        let devices = self.get_onvif_devices().await?;

        let br4kcam_devices = br4kcams_from_onvif_devices(devices, self.skip_hardware_check);

        Ok(br4kcam_devices)
    }

    #[instrument(level = "debug", skip(self))]
    async fn get_video_sources(&self) -> Result<Vec<ApiVideoSource>> {
        web_client::get(&self.address, "v4l", (), ()).await
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn get_br4kcam_video_sources(&self) -> Result<Vec<ApiVideoSource>> {
        let sources = self
            .get_video_sources()
            .await?
            .into_iter()
            .filter(|source| {
                // Firmware <= v11
                source.name.eq("HDIPC - IPCamera (IPCamera)")
                // Firmware >= v13
                    || source.name.eq("UnderwaterCam - IPCamera (UnderwaterCam)")
                // Firmware >= v17
                    || source.name.eq("HDIPC - RadCam (IPCamera)")
            })
            .collect();

        Ok(sources)
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn get_br4kcam_streams(&self) -> Result<Vec<Stream>> {
        let br4kcam_streams = self
            .get_streams()
            .await?
            .into_iter()
            .filter_map(|device| {
                let VideoSourceType::Onvif(VideoSourceOnvif {
                    name,
                    source: VideoSourceOnvifType::Onvif(endpoint),
                    device_information:
                        OnvifDeviceInformation {
                            manufacturer,
                            model,
                            ..
                        },
                }) = &device.video_and_stream.video_source
                else {
                    return None;
                };

                // Firmware <= v11
                let is_old_firmware = name.eq("HDIPC - IPCamera (IPCamera)")
                    && model.eq("HDIPC")
                    && manufacturer.eq("IPCamera");
                // Firmware >= v13
                let is_new_firmware = name.eq("UnderwaterCam - IPCamera (UnderwaterCam)")
                    && model.eq("UnderwaterCam")
                    && manufacturer.eq("IPCamera");
                // Firmware >= v17
                let is_radcam_firmware = name.eq("HDIPC - RadCam (IPCamera)")
                    && model.eq("HDIPC")
                    && manufacturer.eq("RadCam");

                if !(is_old_firmware || is_new_firmware || is_radcam_firmware) {
                    return None;
                }

                let Ok(source_endpoint) = endpoint.parse() else {
                    return None;
                };

                let stream_endpoints = device.video_and_stream.stream_information.endpoints;

                Some(Stream {
                    name: device.video_and_stream.name.clone(),
                    source_endpoint,
                    stream_endpoints,
                    state: device.state,
                    error: device.error.clone(),
                })
            })
            .collect::<Vec<Stream>>();

        // let endpoints = self
        //     .get_streams()
        //     .await?
        //     .into_iter()
        //     .flat_map(|stream| stream.video_and_stream.stream_information.endpoints)
        //     .collect::<Vec<Url>>();

        Ok(br4kcam_streams)
    }

    #[instrument(level = "debug", skip(self))]
    pub(crate) async fn authenticate(&self, camera: &Camera) -> Result<()> {
        let credentials = camera.credentials.clone().context("No credentials set")?;

        let data = AuthenticateOnvifDeviceRequest {
            device_uuid: camera.uuid,
            username: credentials.username,
            password: credentials.password,
        };

        web_client::post(&self.address, "onvif/authentication", (), data).await
    }

    #[instrument(level = "debug", skip(self))]
    async fn get_streams(&self) -> Result<Vec<StreamStatus>> {
        web_client::get(&self.address, "streams", (), ()).await
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn delete_stream(&self, name: &str) -> Result<Vec<StreamStatus>> {
        web_client::delete(
            &self.address,
            "delete_stream",
            (),
            RemoveStream {
                name: name.to_owned(),
            },
        )
        .await
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn create_stream(&self, source: ApiVideoSource) -> Result<Vec<StreamStatus>> {
        let formats = source
            .formats
            .into_iter()
            .filter(|format| matches!(format.encode, VideoEncodeType::H264 | VideoEncodeType::H265))
            .collect::<Vec<Format>>();

        // TODO: Decide a way to get the format
        let format = formats.first().context("Empty formats")?;
        let encode = format.encode.to_owned();
        let size = format.sizes.first().context("Empty sizes")?.to_owned();
        let frame_interval = size
            .intervals
            .first()
            .context("Empty intervals")?
            .to_owned();

        // TODO: When using DHCP, there's no guarantee that the IP of the camera is kept the same, so we NEED to find a more robust ID
        let id = {
            let source: url::Url = source.source.clone().parse()?;

            let host = source.host_str().context("No host")?;
            let stream_id = source.path().chars().last().context("No path")?;

            format!("{host}/{stream_id}")
        };

        let data = PostStream {
            name: format!("4K Cam {id}"),
            source: source.source,
            stream_information: StreamInformation {
                endpoints: vec![format!("rtsp://0.0.0.0:8554/br4kcam_{id}").parse()?],
                configuration: CaptureConfiguration::Video(VideoCaptureConfiguration {
                    encode,
                    height: size.height,
                    width: size.width,
                    frame_interval,
                }),
                extended_configuration: None,
            },
        };

        web_client::post(&self.address, "streams", data, ()).await
    }
}

fn br4kcams_from_onvif_devices(
    devices: Vec<OnvifDevice>,
    skip_hardware_check: bool,
) -> Vec<Camera> {
    devices
        .iter()
        .filter_map(|device| {
            let hardware_ok = skip_hardware_check
                || device
                    .hardware
                    .as_deref()
                    .is_some_and(|hardware| KNOWN_BR4KCAM_HARDWARE.contains(&hardware));

            if device.name != Some("hd".to_string()) || !hardware_ok {
                trace!("Skipping unknown {device:?}");

                return None;
            };

            trace!("4K Cam found: {device:?}");

            Some(Camera {
                hostname: device.ip,
                uuid: device.uuid,
                credentials: Some(Credentials {
                    username: "admin".to_string(),
                    password: "blue".to_string(),
                }),
                streams: IndexMap::new(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_versions_are_not_capped_at_the_next_minor() {
        let supported = semver::VersionReq::parse(SUPPORTED_MCM_VERSIONS).unwrap();

        assert!(!supported.matches(&semver::Version::parse("0.2.3").unwrap()));
        assert!(supported.matches(&semver::Version::parse("0.2.4").unwrap()));
        assert!(supported.matches(&semver::Version::parse("0.3.0").unwrap()));
        assert!(supported.matches(&semver::Version::parse("1.0.0").unwrap()));
    }
}
