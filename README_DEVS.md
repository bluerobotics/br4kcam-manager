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

### PRO controls

`/v1/pro/{camera_uuid}` exposes traditional camera controls for integrators, mapped onto
the camera and autopilot controls in the backend (types in `br4kcam_api::pro`):

- `GET` the current state; `PATCH` any of `mode`, `shutter`, `iso`, `max_iso`,
  `max_shutter`, `exposure_compensation` (EV), `white_balance`, `focus` (%), `zoom`
  (magnification) and `tilt` (degrees), and it answers the new state. Modes:
  - `program`: the camera picks shutter and ISO, within `max_iso` and `max_shutter`.
  - `shutter` (shutter priority): `shutter` is set, the camera picks ISO within `max_iso`.
    It is the camera's manual exposure with its gain switches (`manual_AGain_enable`,
    `manual_DGain_enable`) off, so auto exposure drives the gain alone.
  - `manual`: `shutter` and `iso` are set.

  The camera has no ISO priority (whenever its auto exposure is off, the shutter is manual).
  A value the mode does not take is a 400: a `shutter` in `program`, an `iso` outside
  `manual`, `max_iso` and `exposure_compensation` in `manual`, `max_shutter` outside
  `program`. Every shutter the camera takes is accepted; `shutter_calibrated` in the
  capabilities lists the ones that expose what they say (1/30 to 1/10000 s): longer ones are
  held to the frame time at 30 fps, and 1/100000 s exposes no shorter than ~1/33000 s.
  Switching mode turns the camera's `auto_iris` off (the lens has no driven iris; on, it
  holds every exposure to 200 µs or more).
- `exposure_compensation` shifts the brightness auto exposure aims for, in `program` and
  `shutter` (the camera's `blc_level`, -2 to +1.5 EV in half stops, `EXPOSURE_COMPENSATION`
  in `pro.rs`, measured from the exposure auto exposure settled on at each level).
- `max_iso` is the highest ISO the camera may pick, one limit for `program` and `shutter`:
  an ISO up to 204800 or `"full"` (the camera's full gain, past the calibrated ISOs, shown
  as H). It is kept in the camera's `max_sys_gain`, which `manual` does not touch; its table
  (`MAX_ISO_GAINS` in `pro.rs`) was read with the lens covered, where auto exposure sits at
  its gain ceiling. `max_sys_gain` does not hold shutter priority, so there the limit is also
  written as the camera's `auto_AGain_max` and `auto_DGain_max` (`shutter_gain_caps`, checked
  on the camera at ISO 6400, 51200 and 204800); entering `shutter` with no limit sets ISO
  51200, since uncapped it climbs to camera ISO 1.9 M in the dark. `program` and `manual` set
  those two to 255 instead: they would stop auto exposure first and also clamp the manual
  gains.
- `max_shutter` is the longest shutter `program` may pick (the camera's `max_exposure`, 1/30
  to 1/4000 s): in dim light auto exposure holds the shutter there and raises ISO. The camera
  holds the shutter of its manual exposure (`manual` and `shutter`) to the `max_exposure` it
  had when that was entered, so leaving `program` first sets it to 1 s in a request of its own
  (in the same request it does not work). The manager keeps the `program` limit meanwhile, in
  its settings file so it survives a restart, and puts it back when `program` returns, unless
  that change sets its own; with none kept, `program` gets 1/60 s.
- What the camera is using now comes from its `getISPInfo`, read only: `shutter` in
  `program`, `iso` in `program` and `shutter`, `exposure_warning` (`too_dark` or
  `too_bright` when auto exposure is off its target by more than its error ever reads on
  target) and `color_temperature` (K) in every mode. Its ISO is 100 x the gain it applies,
  which is not what the image shows at high gain (up to ~0.6 stop apart), so it is converted
  to the light-calibrated scale of `ISO_GAINS` (`CAMERA_ISO` in `pro.rs`); above ISO 204800
  that is extrapolated. Its shutter is measured in sensor lines of 14.8 µs (1/4000 reads
  1/3968); the UI shows picked values in third stops. Its exposure time only follows auto
  exposure, so `manual` and `shutter` report the shutter they set.
- Exposure is slew-rate limited in the backend, for every client: `shutter`, `iso`,
  `exposure_compensation`, `max_iso` and `max_shutter` move toward what was asked by at most
  one stop per step (one value per step; a list with a gap wider than a stop, as `max_iso`
  from 100 to 6400, moves one entry), a step per 250 ms at most, about how long a change
  takes to reach the video. A change during a ramp retargets it, and the state reports
  where it is going. A change of mode first lands at the shutter and ISO the camera uses
  now, then ramps to the values it asks for; a `mode` equal to the current one is no change
  of mode. Unramped jumps of 8 to 10 stops froze a V16 camera's stream for 26 s and dropped
  its connection; ramped, both cameras streamed on (tested on V16 and V18). White balance is
  written at once. Changes to one camera are applied one at a time.
- `GET .../capabilities` lists the accepted values, and the range of each actuator the
  camera has set up. The aperture is fixed: on the camera, `auto_iris` and `irisLevel` were
  swept with exposure locked and the image did not change.
- `white_balance` modes: `auto` (tuned for a water `scene`), `manual` (red, green and blue
  gains, 0 to 255) and `temperature` (`kelvin`, 1500 to 15000 K, `white_balance_kelvin` in
  the capabilities): the camera computes the gains for light of that colour temperature from
  its calibration (`auto_awb` 2 and `wb_temp`; checked on V16 and V18). Left out, `kelvin`
  keeps the camera's, or 5500 K when it holds none in range. Manual and temperature hold the
  scene `auto` returns to. White balance is written at once, never ramped.
- `POST .../white_balance/one_push` starts a one-push white balance (202). The camera's
  one-push is its own closed loop on the video that writes its result once, as Manual white
  balance; only its first run after the camera starts switches to auto white balance first.
  So the backend puts white balance on Auto for 1 s before every onceAWB, whoever sends it
  (`one_push_awb::prime`): each run starts from auto white balance's gains, and its end shows
  as Auto turning Manual, even when the gains come out unchanged. The white balance controls
  are busy from the click until then (typically about 2 s; at most 90 s).

```bash
curl -X PATCH localhost:8080/v1/pro/$UUID -H 'Content-Type: application/json' \
  -d '{"mode":"manual","shutter":"1/250","iso":800}'
```

The ISO stops (100 to 204800) map to analog and digital gain through `ISO_GAINS` in
`routes/v1/pro.rs`, measured with the calibration tool below; the measurements are in
[tools/iso_calibration/data/](tools/iso_calibration/data/).

### ISO calibration

[tools/iso_calibration/calibrate_iso.py](tools/iso_calibration/calibrate_iso.py) measures
the gain of each ISO stop and prints `ISO_GAINS` ready to paste. Halving the exposure time
removes exactly one stop of light, so the gain that brings the image back to the same
brightness adds exactly one stop. It needs GStreamer (`gst-launch-1.0`) and SciPy (pinned in
its `requirements.txt`, like the zoom calibration's), and talks to a running 4K Cam Manager.

The gain is analog gain A (0 to ~8 do nothing, then the gain about doubles with A) and
digital gain D on top, in levels of 4 (0 to 7 do nothing) by amounts not known exactly
(D=8 is about 1.9x). Together they reach ~11.85 stops (~71 dB); the camera's Max SysGain
(`max_sys_gain`) does not limit them, it only caps auto exposure's gain. The search
assumes no gain law: analog gain at the current digital level (a guess at twice the
previous stop's gain, then Brent's method on log(gain)), else the lowest digital level whose
full analog gain reaches the target, then analog gain at that level. Each stop matches as
finely as the analog steps allow, in about five to fifteen measurements.

It measures a centered square (`--crop`, half the image height by default) of the camera's
substream, which it first sets to H.264 640x480 at 30 fps if it is not (MJPEG does not work:
the camera sends whole JPEGs inside RTP, which GStreamer drops; the camera caps this stream
at 8192 kbps); `--rtsp` measures another stream instead. GStreamer, with a one-frame leaky
queue, so a reading is always the latest picture (ffmpeg fell minutes behind on 4K). It first
times how long an ISO step takes to show on the stream (~0.3 s), then gives each
measurement `--step-seconds` (use 6: after every change the image creeps ~1.5 % for 6 to
8 s), averaging every frame after that delay. The camera's stream connection dies when a
darkening change comes within ~2 s of a brightening one, so the tool waits 3 s before such
a change. When the stream still freezes (no frame for 1 s), it reconnects and measures again.

1. Point the camera at a flat, evenly lit target (gray card, white wall) lit only by a
   steady lamp: a DC LED, not mains-powered or PWM-dimmed, and no daylight, which fades
   during a run. Let the lamp warm up (LEDs dim for a few minutes after a change). Keep the
   camera, target and lamp still during each lap.
2. Every reference must be mid-gray, luma 100 to 140: the gain a stop needs depends on it
   (ISO 800 measured A=40 at luma ~120 but 55 at 37). The tool shows the live reading and
   waits, at the start and between laps, until it holds steady in range.
3. The shutters only span 1/50 to 1/4000 s (1/8000, 1/10000 and 1/100000 all expose like
   ~1/5600 s, so they are left out): 6 stops from 1/50, while the gain spans ~12. When the
   shutters run out, the tool asks to dim the light about that many stops (ND filter, lamp
   farther away or dimmed; the amount needs no precision since the reference is measured
   again). A full calibration from `--base-shutter 50` takes two laps. Ctrl+C at a light
   prompt keeps the stops found so far, and `--start ISO:A:D` resumes from a stop.
4. Run it, and paste the table it prints into `pro.rs`:

```bash
cd tools/iso_calibration
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt   # once
.venv/bin/python calibrate_iso.py \
  --manager http://192.168.2.2/extensionv2/4kcammanager --camera <uuid> --base-shutter 50 --step-seconds 6
```

While it runs, it puts every image setting in a fixed, manual state: no denoise, dehaze,
WDR, HLC, backlight compensation, slow shutter, anti-flicker, IR lamps or day/night switch;
brightness, contrast, saturation, sharpness, hue and gamma at 128; white balance held at its
current gains. It restores every setting it changed at the end, also on Ctrl+C or failure
(the substream setting stays). It stops when halving the exposure does not darken the image
(something still adapts it). The top stop is reached when no gain can match, and every
reading goes to `iso_calibration.csv`. Low analog gains move in coarse steps (A 9 to 11 look
alike), so the low stops match within a few percent, the rest within about 1 %. A full run
takes about 25 minutes. `--self-test` checks the search logic without a camera.
