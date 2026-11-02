# 4K Cam Manager

It provides a Vue3 application to be loaded either as a BlueOS extension, and as an iframe for Cockpit.

The project has a backend with basically three services:

1. A [Mavlink Camera Manager](http://github.com/mavlink/mavlink-camera-manager) client, responsible to authenticate and setup 4K Cam streams.
2. A web server, responsible to serve the frontend as a static websitem as well as providing a REST API for it.
3. A 4K Cam protocol proxy, translating requests from the frontend to the camera itself.

## How to run

There are a few options:

### As a BlueOS extension:

1. Find the 4K Cam Manager on the Extensions Store, and install it.
2. A "4K Cam" should appear on the left menu after a few seconds.
3. Click the "4K Cam" menu item to access the application's interface.
4. Open Cockpit, there should be an available configured iframe

### As a BlueOS extension, manually:

- Extension Identifier: `bluerobotics.br4kcam_manager`

- Extension Name: `4K Cam Manager`

- Docker image: `joaoantoniocardoso/br4kcam-manager`

- Docker tag: `latest`

- Original Settings:

```
{
  "ExposedPorts": {
    "8080/tcp": {}
  },
  "HostConfig": {
    "Binds": [
      "/var/logs/blueos/extensions/br4kcam-manager:/logs",
      "/usr/blueos/extensions/br4kcam-manager:/app",
      "/root/.config/blueos/ardupilot-manager/firmware/scripts:/scripts"
    ],
    "ExtraHosts": [
      "blueos.internal:host-gateway"
    ],
    "PortBindings": {
      "8080/tcp": [
        {
          "HostPort": ""
        }
      ]
    }
  },
  "entrypoint": [
    "./br4kcam-manager",
    "--verbose",
    "--web-server", "0.0.0.0:8080",
    "--mcm-address", "blueos.internal:6020",
    "--mavlink", "udpout:blueos.internal:11001",
    "--mavlink-system-id", "$MAV_SYSTEM_ID",
    "--mavlink-component-id", "56",
    "--log-path", "/logs",
    "--settings-file", "/app/settings.json",
    "--autopilot-scripts-file", "/scripts/br4kcam.lua",
    "--blueos-address", "blueos.internal"
  ]
}
```

### As a standalone application, via Docker container:

```bash
docker build . -t br4kcam-manager --progress=plain
docker run --rm -it --net=host br4kcam-manager:latest --help
```

###  As a standalone application, locally:
```bash
./build.sh
./backend/target/br4kcam-manager --help
```

**NOTE**: this software requires:
- [Mavlink Camera Manager](http://github.com/mavlink/mavlink-camera-manager) instance running somewhere. It can configurable using the `--mcm-address <MCM_ADDRESS>` command line argument.
- Some MAVLink-compatible autopilot firmware running with version `>=4.7.0`
- Some MAVLink router endpoint, configurable using the `--mavlink` command line argument.

## Public API

Other projects depend on this API, and [api/](api/README.md) pins it: `cargo test` fails
when it changes by accident. Read [api/README.md](api/README.md) before changing routes or
wire types.
