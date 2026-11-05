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

## Camera firmware updates

The extension offers camera firmware updates from a firmware server. Without one, the
feature stays hidden.

### Configuring the extension

Set `FIRMWARE_SERVER_URL` (or `--firmware-server-url`) to the directory that holds
`index.json`, e.g. `https://example.com/4k-cam/firmware/`. On BlueOS, edit the extension's
settings and fill the `FIRMWARE_SERVER_URL=` entry under `Env`. An empty or invalid value
only disables firmware updates.

### Firmware server requirements

Any static HTTP(S) host works, a public bucket included. The extension downloads from it
itself (not the browser), so CORS does not matter.

- The vehicle must reach it. Vehicles without internet simply see no updates.
- Serve HTTPS. The checksum catches corrupted downloads, but over plain HTTP anyone on the
  path could swap both the image and its checksum.
- `index.json` lists the firmwares, each `file` relative to `index.json`:

  ```json
  {
    "firmwares": [
      {
        "version": "18.010.U35.6_0054_V18(261030)",
        "file": "IPC_18.010.U35.6_0054_V18(261030).img",
        "sha256": "6e80f33b4dbac5d7a0c5a32f093116a95cc304b599c7d17ed363c55fec128f07"
      }
    ]
  }
  ```

- `version` must be exactly what the camera reports once running it (`getSysConfig`'s
  `version`): the update is only reported successful when they match.
- `file` is the vendor's `IPC_*.img` update image, served unchanged (about 26 MB). The index
  must be fetched within 30 s and an image within 5 minutes.
- The banner offers the newest version (by `_V<n>(<date>)`) when it is newer than the
  camera's. Any listed version, older ones included, can be installed through the API.

[tools/firmware_server/make_index.py](tools/firmware_server/make_index.py) writes `index.json`
for a directory of `IPC_<version>.img` files. To test locally:

```bash
python3 tools/firmware_server/make_index.py ./firmware
python3 -m http.server 8765 -d ./firmware  # FIRMWARE_SERVER_URL=http://<this machine>:8765/
```

### What the extension checks

Before anything reaches the camera, the image must match its `sha256` and its header must
carry the hardware type the camera reports (`update_type`). The camera checks the image
again itself: it refuses a corrupted one and reboots into its old firmware, which the
extension reports as a failure.

A camera announces a new ONVIF UUID after a firmware change (its MAC stays in the UUID). The
extension moves the camera's saved hardware setup to the new UUID and tells the user.

## Public API

Other projects depend on this API, and [api/](api/README.md) pins it: `cargo test` fails
when it changes by accident. Read [api/README.md](api/README.md) before changing routes or
wire types.
