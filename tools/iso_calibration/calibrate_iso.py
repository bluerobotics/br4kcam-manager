#!/usr/bin/env python3
"""Calibrate the PRO API's ISO stops (`ISO_GAINS` in backend/.../routes/v1/pro.rs).

Halving the exposure time removes exactly one stop of light, so the gain that brings the
image back to the reference brightness adds exactly one stop. Comparing equal brightness
needs no linear image: gamma and tone curves cancel out. ISO 100 is no gain; each further
stop doubles it.

Usage:
  calibrate_iso.py --manager http://192.168.2.2:<port> --camera <uuid> [--out log.csv]
  calibrate_iso.py --self-test

Setup (see README_DEVS.md, "ISO calibration"): camera fixed on a flat, evenly lit target
(gray card, white wall), steady light (daylight or DC LED, no mains flicker or PWM
dimming). While it measures, the tool puts every image setting in a fixed, manual state
(no denoise, dehaze, WDR, HLC, backlight compensation, slow shutter, anti-flicker, IR lamps
or day/night switch; neutral picture settings; white balance held). Everything it changes
is restored at the end, also on Ctrl+C or failure.

Needs GStreamer (gst-launch-1.0) and the packages in requirements.txt, in a virtual environment:
  python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
Talks to the 4K Cam Manager, so the UI follows along.
"""

import argparse
import csv
import itertools
import json
import math
import subprocess
import sys
import threading
import time
import urllib.request

from scipy.optimize import brentq

# Exposure times the camera takes (1/x s) that double cleanly, as chains: a stop is only
# exact between neighbours of one chain. The second chain continues from the first. None is
# longer than a frame at 30 fps (1/25 s is not: the camera shortens it, and its stream froze).
# Not 1/8000: it is only ~half a stop shorter than 1/4000 (1/8000, 1/10000 and "1/100000",
# 34464, all expose alike, ~1/5600 s, the camera's shortest).
SHUTTER_CHAINS = [[50, 100, 200, 400, 800], [1000, 2000, 4000]]
# The chains span ~6 stops but the gain ~11.7 (~70 dB): when they run out, the light is
# dimmed (by however much; the reference is measured again) and they run again from the gain
# reached, as many laps as the gain lasts.
# The reference brightness: mid-gray (18 % gray is luma ~118). The gain a stop needs depends
# on it (ISO 800 measured A=40 at luma 120, 51 at 87, 55 at 37: near black the image is no
# function of exposure x gain alone), so every reference sits in this narrow range.
REFERENCE_RANGE = (100, 140)
# Seconds without a frame that mean the stream froze. After a big darkening step (from a
# clipped image, say) the camera pauses its stream for up to ~0.4 s and carries on, while a
# frozen connection never recovers; this tells the two apart and still catches a freeze fast.
FREEZE_SECONDS = 1.0
# The camera's stream connection dies when a darkening change comes within ~2 s of a
# brightening one (seen at 1 s and once at 2 s, never at 3 s), so darkening waits this long.
DARKEN_AFTER_BRIGHTEN_SECONDS = 3.0


def brighter(previous, current):
    """+1 when setting `current` (shutter 1/x, (analog, digital)) is brighter than `previous`,
    -1 when darker, 0 when equal or unknown. A slower shutter or more gain is brighter; across
    digital levels the gain is only estimated, which at worst costs a needless wait."""
    if previous is None or previous == current:
        return 0
    (shutter_before, gain_before), (shutter_now, gain_now) = previous, current
    if shutter_now != shutter_before:
        return 1 if shutter_now < shutter_before else -1
    return 1 if rough_gain(gain_now) > rough_gain(gain_before) else -1


def rough_gain(gain):
    analog, digital = gain
    return max(1, analog) * max(1, digital // 4)
# Gain = (analog A, digital D). Analog: 0 to ~8 do nothing, then the gain about doubles with
# A. Digital multiplies on top in levels of 4 (D 0-7 do nothing; 8-11 alike, 12-15 alike...)
# by amounts not known exactly (D=8 is less than 2x), so the search assumes no law: analog at
# the current digital level, else the lowest digital level whose full analog reaches the
# target, then analog at that level: as fine as the analog steps throughout. Manual gains
# ignore max_sys_gain (it only caps auto exposure's gain), even on a camera booted with 12.
MAX_GAIN_INDEX = 255  # analog
DIGITAL_LEVELS = [0, *range(8, 256, 4)]
# Starting guess for ISO 200's gain (measured 11 to 12); any guess converges, a good one is
# just quicker. Later stops start from twice the previous stop's gain.
FIRST_STOP_GUESS = 12
# Every image setting in a fixed state while measuring, so the brightness depends only on
# shutter and gain: nothing adapts (dehaze, WDR, HLC, backlight, slow shutter, IR lamps,
# day/night switch) or smooths over time (denoise). Only fields the camera reports are
# written, and all of them are restored at the end.
KNOWN_BASE = {
    "brightness": 128,
    "contrast": 128,
    "saturation": 128,
    "sharpness": 128,
    "hue": 128,
    "gamma": 128,
    "blc_level": 0,
    "antiFog": 0,
    "sceneMode": 0,
    # White balance held at the camera's current gains.
    "auto_awb": 1,
}
KNOWN_ADVANCED = {
    "noiseReduction": 0,
    "_2DNR_level": 0,
    "wdr_sensor": 0,
    "hlc_enable": 0,
    "low_farme_rate": 0,
    "anti_flicker": 0,
    "color_black": 0,
    "led_control": 2,
    "scene_mode": 0,
}
# Exposure fields this tool changes. Everything changed is restored at the end.
EXPOSURE = [
    "auto_exposureEx",
    "auto_gain_mode",
    "exposure_time",
    "manual_AGain_enable",
    "manual_AGain",
    "manual_DGain_enable",
    "manual_DGain",
]


def search_gain(measure, target, low, known, guess, high=MAX_GAIN_INDEX):
    """Gain index in [low, high] whose brightness is closest to `target`, with few readings.

    `measure(index)` must not decrease as the index grows; plateaus are fine. `known` maps
    indexes already measured to their brightness and must include `low`, which must be below
    the target. Brightness grows about linearly with the logarithm of the gain, so the search
    works on log(gain + 1): it starts at `guess`, steps up along the measured slope (at most
    4x at a time) until the target is passed, then interpolates inside that bracket, bisecting
    instead after any reading that did not halve it. Returns (index, brightness), or None when even
    `high` stays clearly below the target, which it measures only once the slope says nothing
    lower reaches it.
    """
    seen = dict(known)

    def at(index):
        if index not in seen:
            seen[index] = measure(index)
        return seen[index]

    def log(index):
        return math.log(index + 1)

    def unlog(value):
        return round(math.exp(value) - 1)

    lo = low
    hi = min(high, max(lo + 1, guess))
    while at(hi) < target:
        if hi == high:
            return None if seen[hi] < target * 0.97 else (hi, seen[hi])
        if hi - lo < 4:
            # Readings this close sit on one flat step, where noise decides their slope:
            # step about a quarter stop instead of extrapolating it.
            estimate = hi + max(2, hi // 4)
        else:
            slope = (seen[hi] - seen[lo]) / (log(hi) - log(lo))
            # Overshoot the estimate a little, so the target usually ends up bracketed.
            estimate = unlog(log(hi) + 1.25 * (target - seen[hi]) / slope) if slope > 0 else high
        lo, hi = hi, min(high, 4 * (hi + 1), max(hi + 1, estimate))
    if hi - lo > 1:
        # Brent's method (interpolation, falling back to bisection) on log(gain + 1); gains
        # are integers, so it stops once the bracket is narrower than one gain step there.
        root = brentq(
            lambda x: at(min(hi, max(lo, unlog(x)))) - target,
            log(lo),
            log(hi),
            xtol=0.25 / (hi + 1),
        )
        lo = max(lo, min(hi - 1, math.floor(math.exp(root) - 1)))
        # Settle on the two neighbouring gains around the crossing (cached, so cheap).
        while lo > low and at(lo) >= target:
            lo -= 1
        while at(lo + 1) < target:
            lo += 1
        hi = lo + 1
    # Equally close: the lower gain, which is less noisy.
    best = min((lo, hi), key=lambda index: (abs(seen[index] - target), index))
    return best, seen[best]


def agreeing_mean(values):
    """Mean of `values` when its two halves agree (the image held still), else None."""
    if len(values) < 2:
        return None
    half = len(values) // 2
    first, second = values[:half], values[half:]
    mean_first, mean_second = sum(first) / len(first), sum(second) / len(second)
    if abs(mean_first - mean_second) > max(0.5, 0.02 * mean_second):
        return None
    return sum(values) / len(values)


def crossing_delay(samples, start, before, after):
    """Seconds from `start` to the first frame past halfway from `before` to `after`."""
    halfway = (before + after) / 2
    for arrived, value in samples:
        if arrived >= start and (value - halfway) * (after - before) > 0:
            return arrived - start
    return None


class Stream:
    """Brightness of every frame of one stream, read by a single long-lived GStreamer.

    Reconnecting per reading costs seconds; staying connected lets a reading be just the
    next few frames.
    """

    def __init__(self, rtsp, crop):
        if not 0 < crop <= 1:
            raise SystemExit("--crop must be in (0, 1]")
        self.rtsp, self.crop = rtsp, crop
        self._start()

    # Each frame's centered square, scaled down to SIDE x SIDE luma bytes: averaging those is
    # the square's brightness, and Python only handles a few KB per frame.
    SIDE = 64

    def _start(self):
        # GStreamer, not ffmpeg: decoding 4K in software, ffmpeg fell behind the camera and
        # its readings grew minutes stale. The leaky one-frame queue drops frames instead, so
        # a reading is always the latest picture; decodebin uses a hardware decoder if any.
        side = self.SIDE
        command = [
            "gst-launch-1.0", "-q", "rtspsrc", f"location={self.rtsp}", "protocols=tcp",
            "latency=0", "!", "decodebin", "!", "queue", "leaky=downstream",
            "max-size-buffers=1", "!", "videoconvert", "!", "aspectratiocrop",
            "aspect-ratio=1/1", "!", "videoscale", "!",
            f"video/x-raw,format=GRAY8,width={side},height={side}", "!",
            "fdsink", "fd=1", "sync=false",
        ]  # fmt: skip
        self.process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.samples = []  # (arrival time, luma)
        self.lock = threading.Lock()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        side = self.SIDE
        # `crop` of the square's side, centered (the square is already the shorter side).
        inner = max(1, round(side * self.crop))
        first = (side - inner) // 2
        while len(frame := self.process.stdout.read(side * side)) == side * side:
            rows = (frame[row * side + first : row * side + first + inner] for row in range(first, first + inner))
            value = sum(sum(row) for row in rows) / (inner * inner)
            with self.lock:
                self.samples.append((time.monotonic(), value))
                del self.samples[:-600]

    def wait_ready(self, timeout=30.0):
        """Waits for the first frame: connecting to the stream takes a few seconds."""
        deadline = time.monotonic() + timeout
        while not self.samples:
            if time.monotonic() > deadline:
                raise RuntimeError("No frames from the stream; check --rtsp")
            if self.process.poll() is not None:
                # The camera restarts its stream after some image changes: connect again.
                self._start()
            time.sleep(0.1)

    def between(self, start, end):
        """(arrival, luma) of the frames that arrived in [start, end], once `end` has passed."""
        # Returns early when frames stop coming (`stalled` then reports it), so a frozen
        # connection costs a fraction of a second, not the whole window.
        while time.monotonic() < end and self.process.poll() is None:
            with self.lock:
                last = self.samples[-1][0] if self.samples else 0.0
            if time.monotonic() - max(last, start) > FREEZE_SECONDS:
                break
            time.sleep(0.02)
        with self.lock:
            return [(arrived, value) for arrived, value in self.samples if start <= arrived <= end]

    def frame_interval(self):
        with self.lock:
            times = [arrived for arrived, _ in self.samples[-30:]]
        gaps = sorted(later - earlier for earlier, later in zip(times, times[1:]))
        return gaps[len(gaps) // 2] if gaps else 0.2

    def stalled(self, samples, start, end):
        """Whether frames stopped arriving in [start, end]: the camera sometimes freezes
        a connection after a big exposure change while new connections still work."""
        times = [start] + [arrived for arrived, _ in samples] + [min(end, time.monotonic())]
        return max(later - earlier for earlier, later in zip(times, times[1:])) > FREEZE_SECONDS

    def reconnect(self):
        print("    (the stream froze; reconnecting)")
        self.close()
        self._start()
        self.wait_ready()

    def close(self):
        # Killed outright: it only reads, and blocked on a frozen connection it ignores SIGTERM.
        self.process.kill()
        self.process.wait()


class Camera:
    def __init__(self, manager, uuid, stream, step_seconds):
        self.manager = manager.rstrip("/")
        self.uuid = uuid
        self.stream = stream
        self.step_seconds = step_seconds
        # Set by `measure_latency` before measuring: command-to-frame delay plus a frame.
        self.settle = None
        self.point = None  # (shutter, gain index) last written
        self.brightened_at = 0.0
        self.readings = 0

    def control(self, action, payload=None):
        body = {"camera_uuid": self.uuid, "action": action}
        if payload is not None:
            body["json"] = payload
        request = urllib.request.Request(
            f"{self.manager}/v1/camera/control",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.loads(response.read() or b"null")

    def read(self):
        return self.control("getImageAdjustment")

    def read_advanced(self):
        return self.control("getImageAdjustmentEx")

    def write(self, **fields):
        self.control("setImageAdjustment", fields)

    def write_advanced(self, **fields):
        self.control("setImageAdjustmentEx", fields)

    def apply(self, shutter, gain):
        """Writes a setting, first waiting out a recent brightening when this one darkens
        (see DARKEN_AFTER_BRIGHTEN_SECONDS). Returns when the write went out."""
        direction = brighter(self.point, (shutter, gain))
        # Not knowing the last setting (the first write), it may darken too.
        if direction < 0 or self.point is None:
            wait = self.brightened_at + DARKEN_AFTER_BRIGHTEN_SECONDS - time.monotonic()
            if wait > 0:
                time.sleep(wait)
        analog, digital = gain
        start = time.monotonic()
        self.write(exposure_time=shutter, manual_AGain=analog, manual_DGain=digital)
        if direction > 0:
            self.brightened_at = start
        self.point = (shutter, gain)
        return start

    def set_point(self, shutter, gain):
        """(brightness, frames averaged) of a setting: every frame from `settle` after the
        command to the end of the step. A step whose frames still drift is measured again,
        and one whose stream froze is measured again on a new connection."""
        for _ in range(3):
            start = self.apply(shutter, gain)
            since, end = start + self.settle, start + self.step_seconds
            for _ in range(3):
                samples = self.stream.between(since, end)
                if self.stream.stalled(samples, since, end):
                    self.stream.reconnect()
                    break
                values = [value for _, value in samples]
                mean = agreeing_mean(values)
                if mean is not None:
                    self.readings += 1
                    return mean, len(values)
                since, end = end, end + (self.step_seconds - self.settle)
            else:
                raise RuntimeError("The brightness kept drifting; is the light steady?")
        raise RuntimeError("The stream kept freezing.")

    def measure_latency(self, shutter, low=(0, 0), trials=4):
        """Times an ISO step (gain `low` <-> `step`) from the command to its first frame past
        halfway, and sets `settle` to the slowest of `trials` plus one frame. The step is about
        two stops, enough to time; a bigger one clips a bright target to white, a jump that
        upsets the camera's stream. Only a too dim target needs the bigger one."""
        # The levels, read freeze-safe with a generous settle until the delay is known.
        self.settle = 1.0
        levels = {low: self.set_point(shutter, low)[0]}
        # About two stops up, then more for a dim target (resuming starts from a gain).
        analog, digital = low
        upper = DIGITAL_LEVELS[min(DIGITAL_LEVELS.index(digital) + 2, len(DIGITAL_LEVELS) - 1)]
        for step in ((2 * FIRST_STOP_GUESS, 0), (64, 0)) if low == (0, 0) else ((min(255, 4 * analog), digital), (255, upper)):
            levels[step] = self.set_point(shutter, step)[0]
            if levels[step] - levels[low] >= 20:
                break
        else:
            raise SystemExit(f"An ISO step barely changes the image ({levels}); cannot time it.")
        delays = []
        attempts = 0
        while len(delays) < trials:
            attempts += 1
            if attempts > 2 * trials:
                raise SystemExit("Could not time the ISO step: the stream keeps freezing.")
            target, previous = (low, step) if len(delays) % 2 == 0 else (step, low)
            start = self.apply(shutter, target)
            samples = self.stream.between(start, start + 2.0)
            if self.stream.stalled(samples, start, start + 2.0):
                # Put the other level back so the next trial steps from it again.
                self.stream.reconnect()
                self.apply(shutter, previous)
                time.sleep(1.0)
                continue
            delay = crossing_delay(samples, start, levels[previous], levels[target])
            if delay is None:
                raise SystemExit("An ISO step did not show on the stream within 2 s.")
            delays.append(delay)
        frame = self.stream.frame_interval()
        self.settle = max(delays) + frame
        print(
            f"Command-to-frame delay: {', '.join(f'{d:.2f}' for d in delays)} s; "
            f"frames every {frame:.2f} s; settle {self.settle:.2f} s of each "
            f"{self.step_seconds:.1f} s step."
        )
        if self.settle + 2 * frame > self.step_seconds:
            raise SystemExit("--step-seconds leaves fewer than 2 frames after settling.")


def wait_for_light(camera, shutter, gain, advice):
    """Waits until the light is set so the image at (`shutter`, `gain`) is mid-gray and holds
    still for three readings, printing each reading. False when the user gives up (Ctrl+C)."""
    low, high = REFERENCE_RANGE
    print(
        f"\n{advice} Without flicker, keeping camera and target still, until the reading is in "
        f"{low}..{high}. It carries on once the reading holds steady; Ctrl+C stops here and "
        "keeps the stops found so far."
    )
    steady = []
    try:
        while len(steady) < 3:
            try:
                value = camera.set_point(shutter, gain)[0]
            except RuntimeError:  # drifting while the light is being changed
                steady = []
                continue
            print(f"    luma {value:6.1f}")
            if not low <= value <= high:
                steady = []
            elif steady and abs(value / steady[0] - 1) >= 0.01:
                steady = [value]
            else:
                steady.append(value)
            if len(steady) < 3:
                time.sleep(1.0)
    except KeyboardInterrupt:
        return False
    return True


def find_gain(read, target, gain, darker):
    """The (analog, digital) gain whose brightness `read(analog, digital)` is closest to
    `target`, at least `gain`, which reads `darker` (below the target). Returns (gain, value),
    or None when even full gain stays clearly below the target."""
    analog, digital = gain
    # Analog at the current digital level.
    found = search_gain(
        lambda a: read(a, digital), target, low=analog, known={analog: darker},
        guess=min(255, 2 * analog) if analog else FIRST_STOP_GUESS,
    )  # fmt: skip
    if found and (read(found[0], digital) >= target or found[0] < 255):
        return (found[0], digital), found[1]
    # Full analog is not enough: the lowest digital level whose full analog reaches it.
    level = DIGITAL_LEVELS.index(digital)
    if level == len(DIGITAL_LEVELS) - 1:
        return None if read(255, digital) < target * 0.97 else ((255, digital), read(255, digital))
    found = search_gain(
        lambda i: read(255, DIGITAL_LEVELS[i]), target, low=level, known={level: read(255, digital)},
        guess=level + 1, high=len(DIGITAL_LEVELS) - 1,
    )  # fmt: skip
    if found is None:
        return None
    level = found[0] + (read(255, DIGITAL_LEVELS[found[0]]) < target and found[0] < len(DIGITAL_LEVELS) - 1)
    digital, below = DIGITAL_LEVELS[level], (255, DIGITAL_LEVELS[level - 1])
    # Analog at that level, from no analog gain: the closest of it and full analog below it.
    if read(0, digital) >= target:
        found = (0, read(0, digital))
    else:
        found = search_gain(lambda a: read(a, digital), target, low=0, known={0: read(0, digital)}, guess=128)
    candidates = [(below, read(*below))] + ([((found[0], digital), found[1])] if found else [((255, digital), read(255, digital))])
    return min(candidates, key=lambda candidate: abs(candidate[1] - target))


def calibrate(camera, log, base_shutter, max_stops, start=(100, (0, 0))):
    """Stops from `start` (ISO, (analog, digital)), ISO 100 with no gain unless resuming."""
    def measure(shutter, gain):
        value, frames = camera.set_point(shutter, gain)
        log.writerow([shutter, *gain, f"{value:.2f}", frames])
        print(f"    1/{shutter} s  A={gain[0]:3d} D={gain[1]:3d}  luma={value:6.1f}  ({frames} frames)")
        return value

    chains = [chain for chain in SHUTTER_CHAINS if chain[-1] >= base_shutter]
    chains[0] = [shutter for shutter in chains[0] if shutter >= base_shutter]

    print(f"ISO {start[0]} (A={start[1][0]} D={start[1][1]}) at 1/{base_shutter} s:")
    reference = measure(base_shutter, start[1])
    low, high = REFERENCE_RANGE
    if not low <= reference <= high:
        advice = f"ISO {start[0]} shows luma {reference:.0f}: {'brighten' if reference < low else 'dim'} the light."
        if not wait_for_light(camera, base_shutter, start[1], advice):
            return [start]
        reference = measure(base_shutter, start[1])

    stops = [start]
    gain = start[1]
    span = math.log2(chains[-1][-1] / chains[0][0])
    # Later laps run the same chains under dimmer light, until the gain runs out.
    for lap, chain_number, chain in ((l, n, c) for l in itertools.count() for n, c in enumerate(chains)):
        if math.log2(stops[-1][0] / 100) >= max_stops or gain == (255, DIGITAL_LEVELS[-1]):
            return stops
        if lap > 0 and chain_number == 0:
            advice = (
                f"Out of shutter range at ISO {stops[-1][0]}. Dim the light about {span:.0f} stops "
                "(ND filter, lamp farther away or dimmed)."
            )
            if not wait_for_light(camera, chain[0], gain, advice):
                return stops
            print(f"Reference for the next lap at 1/{chain[0]} s:")
            reference = measure(chain[0], gain)
        elif chain_number > 0:
            # New chain: re-measure the reference at its first shutter with the last gain.
            print(f"Reference for the next chain at 1/{chain[0]} s:")
            reference = measure(chain[0], gain)
            if not 10 <= reference <= 200:
                print("  Too dark or clipped to continue: this is the top ISO.")
                return stops
        for shutter in chain[1:]:
            if math.log2(stops[-1][0] / 100) >= max_stops:
                return stops
            iso = 2 * stops[-1][0]
            print(f"ISO {iso}: 1/{shutter} s, matching luma {reference:.1f}:")
            # Half the light at the same gain must look darker, or something still adapts.
            darker = measure(shutter, gain)
            if darker > reference * 0.9:
                raise SystemExit(
                    "Halving the exposure did not darken the image: something still adapts "
                    "it (auto white balance, a flickering light, the scene). See the setup."
                )
            readings = {gain: darker}

            def read(analog, digital, shutter=shutter, readings=readings):
                if (analog, digital) not in readings:
                    readings[(analog, digital)] = measure(shutter, (analog, digital))
                return readings[(analog, digital)]

            found = find_gain(read, reference, gain, darker)
            if found is None:
                print("  Out of gain: this is the top ISO.")
                return stops
            gain, value = found
            print(f"  -> A={gain[0]} D={gain[1]} ({value / reference - 1:+.1%})")
            stops.append((iso, gain))
    return stops


def rust_table(stops):
    rows = "\n".join(f"    ({iso}, {analog}, {digital})," for iso, (analog, digital) in stops)
    return f"const ISO_GAINS: [(u32, u8, u8); {len(stops)}] = [\n{rows}\n];"


def self_test():
    def check(model, target, low=0, guess=FIRST_STOP_GUESS):
        """The search finds what reading every gain would, in few readings."""
        calls = []

        def measure(index):
            calls.append(index)
            return model(index)

        found = search_gain(measure, target, low, {low: model(low)}, guess)
        best = min(abs(model(i) - target) for i in range(low, MAX_GAIN_INDEX + 1))
        if model(MAX_GAIN_INDEX) < target * 0.97:
            assert found is None, (target, found)
        else:
            # As close as reading every gain would get (gains can tie in brightness).
            assert abs(found[1] - target) == best, (target, found, best)
        return len(calls)

    # Brightness linear in log(gain), saturating at 210.
    def logarithmic(index):
        return min(210.0, 45.0 + 22.0 * math.log2(1 + index))

    # The camera at 1/100 s (tools/iso_calibration/data), interpolated in log(gain), with its
    # quirks: no effect below 8, gain moving in steps of two, nothing gained above ~130.
    readings = [(0, 60.4), (7, 60.7), (11, 81.5), (13, 94.2), (31, 133.0), (63, 176.7),
                (127, 200.6), (255, 201.3)]

    def camera(index):
        effective = 0 if index < 8 else index - index % 2
        for (g0, v0), (g1, v1) in zip(readings, readings[1:]):
            if g0 <= effective <= g1:
                fraction = (math.log(effective + 1) - math.log(g0 + 1)) / (
                    math.log(g1 + 1) - math.log(g0 + 1)
                )
                return round(v0 + fraction * (v1 - v0), 1)
        return readings[-1][1]

    for model in (logarithmic, camera):
        for target in (45.0, 70.0, 90.0, 110.0, 120.0, 150.0, 180.0, 199.0):
            for low, guess in ((0, FIRST_STOP_GUESS), (11, 22), (24, 48), (50, 100)):
                if model(low) < target:
                    assert check(model, target, low, guess) <= 8, (model.__name__, target, low)
    # Analog and digital gain with a digital law the search does not know (D=8 is 1.5x
    # here): find_gain still lands within an analog step of the target, in few readings.
    def two_gains(analog, digital):
        gain = max(1.0, analog / 5) * (1.0 if digital < 8 else 1.5 + 0.4 * (digital // 4 - 2))
        return min(228.0, 30.0 + 18.0 * math.log2(gain))

    for start in ((84, 0), (175, 0), (255, 0), (255, 8), (175, 8), (200, 24), (255, 244)):
        for target in (60.0, 90.0, 110.0, 130.0, 150.0):
            if two_gains(*start) >= target:
                continue
            reads = {}
            found = find_gain(lambda a, d: reads.setdefault((a, d), two_gains(a, d)), target, start, two_gains(*start))
            top = two_gains(255, DIGITAL_LEVELS[-1])
            if top < target * 0.97:
                assert found is None, (start, target, found)
                continue
            # One analog step at full gain moves the model by about 18 * log2(256/255) * ...;
            # anything within 1 % of the target (or the top) is as good as reading every gain.
            assert found and abs(found[1] - target) <= max(0.01 * target, 0.5), (start, target, found)
            assert len(reads) <= 18, (start, target, len(reads))
    # Out of gain: found out without stepping through the plateau.
    assert check(camera, 500.0, 51, 102) <= 5

    class FakeCamera:
        def __init__(self, readings):
            self.readings = iter(readings)

        def set_point(self, shutter, gain):
            value = next(self.readings)
            if value is None:
                raise RuntimeError("drifting")
            return value, 30

    sleep, time.sleep = time.sleep, lambda seconds: None
    try:
        # Clipped, drifting while dimmed, then steady in range: done after three steady readings.
        fake = FakeCamera([228.0, None, 160.0, 125.0, 110.0, 110.5, 110.2, 110.4])
        assert wait_for_light(fake, 50, (255, 8), "Dim the light.")
        assert next(fake.readings) == 110.4
    finally:
        time.sleep = sleep
    assert rust_table([(100, (0, 0)), (200, (11, 0))]).splitlines()[1:3] == [
        "    (100, 0, 0),",
        "    (200, 11, 0),",
    ]
    assert agreeing_mean([100.0, 100.4, 100.2, 100.2]) == 100.2
    assert agreeing_mean([80.0, 90.0, 100.0, 100.0]) is None
    assert agreeing_mean([100.0]) is None
    frames = [(0.2, 50.0), (0.4, 52.0), (0.6, 120.0), (0.8, 150.0)]
    assert crossing_delay(frames, 0.0, 50.0, 150.0) == 0.6
    falling = [(0.2, 150.0), (0.4, 148.0), (0.6, 80.0), (0.8, 50.0)]
    assert crossing_delay(falling, 0.0, 150.0, 50.0) == 0.6
    assert crossing_delay(frames, 0.7, 50.0, 150.0) == 0.8 - 0.7
    assert crossing_delay(frames[:2], 0.0, 50.0, 150.0) is None
    steady = [(0.1 * i, 100.0) for i in range(1, 20)]
    assert not Stream.stalled(None, steady, 0.0, 2.0)
    assert Stream.stalled(None, steady[:5], 0.0, 2.0)  # frames stop at 0.5 s
    assert Stream.stalled(None, [], 0.0, 2.0)
    assert brighter(None, (100, (0, 0))) == 0
    assert brighter((100, (0, 0)), (100, (64, 0))) == 1 and brighter((100, (64, 0)), (100, (0, 0))) == -1
    assert brighter((100, (24, 0)), (200, (24, 0))) == -1 and brighter((200, (0, 0)), (100, (64, 0))) == 1
    assert brighter((100, (255, 0)), (100, (175, 8))) == 1
    print("self-test ok")


def camera_rtsp(manager, uuid):
    with urllib.request.urlopen(f"{manager.rstrip('/')}/v1/camera/list", timeout=30) as response:
        cameras = json.loads(response.read())
    if uuid not in cameras:
        raise SystemExit(f"Camera {uuid} is not in the camera list now; pass --rtsp, or retry.")
    return f"rtsp://{cameras[uuid]['hostname']}:554/stream_1"


# The substream the tool measures: H.264 is light to decode with low delay at any frame
# size. Not MJPEG: this camera sends whole JPEGs inside RTP, which GStreamer's decoders drop.
# The camera caps this stream at 8192 kbps (it accepts more, then keeps 8192).
SUBSTREAM = {"channel": 1, "encode_type": 1, "pic_width": 640, "pic_height": 480, "frame_rate": 30, "bitrate": 8192}


def ensure_substream(camera):
    """Sets the substream to SUBSTREAM unless it already is; it stays so after the run."""
    def wrong():
        try:
            current = camera.control("getVencConf", {"channel": SUBSTREAM["channel"]})
        except Exception:  # the manager cannot read encodings it does not know (MJPEG)
            current = {}
        return {key: current.get(key) for key, value in SUBSTREAM.items() if current.get(key) != value}

    found = wrong()
    if not found:
        return
    print(f"Setting the substream to {SUBSTREAM} (it had {found}).")
    camera.control("setVencConf", SUBSTREAM)
    time.sleep(2.0)
    if still := wrong():
        raise SystemExit(f"The camera did not take the substream setting: {still}. Pass --rtsp to measure another stream.")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--manager", help="4K Cam Manager URL, e.g. http://192.168.2.2:32802")
    parser.add_argument("--camera", help="Camera UUID, see /v1/camera/list")
    parser.add_argument("--rtsp", help="Stream to measure (default: the camera's substream, set to H.264 640x480 30 fps first if it is not)")
    parser.add_argument("--base-shutter", type=int, default=50, choices=SHUTTER_CHAINS[0], help="1/x s for ISO 100 (default 50)")
    parser.add_argument("--max-stops", type=int, default=12, help="Stops above ISO 100 to find (default 12, the sensor's 72 dB)")
    parser.add_argument("--step-seconds", type=float, default=2.0, help="Seconds per measurement; frames after the measured settle time are averaged (default 2)")
    parser.add_argument("--crop", type=float, default=0.5, help="Side of the measured center square, as a fraction of the image's shorter side (default 0.5)")
    parser.add_argument("--out", default="iso_calibration.csv", help="Measurement log (default iso_calibration.csv)")
    parser.add_argument("--start", help="Resume from a stop already measured, as ISO:analog:digital (e.g. 3200:175:0), under the light for the next lap; prints the table from there on")
    parser.add_argument("--self-test", action="store_true", help="Check the search logic and exit")
    args = parser.parse_args()

    sys.stdout.reconfigure(line_buffering=True)
    if args.self_test:
        self_test()
        return
    if not (args.manager and args.camera):
        parser.error("--manager and --camera are required")

    start = (100, (0, 0))
    if args.start:
        iso, analog, digital = map(int, args.start.split(":"))
        start = (iso, (analog, digital))
        if not 0 <= analog <= 255 or digital not in DIGITAL_LEVELS:
            parser.error(f"--start: analog 0-255, digital one of {DIGITAL_LEVELS[:4]}...")
    rtsp = args.rtsp or camera_rtsp(args.manager, args.camera)
    camera = Camera(args.manager, args.camera, None, args.step_seconds)
    if not args.rtsp:
        ensure_substream(camera)
    base = camera.read()
    advanced = camera.read_advanced()
    known_base = {key: value for key, value in KNOWN_BASE.items() if key in base}
    known_advanced = {key: value for key, value in KNOWN_ADVANCED.items() if key in advanced}
    original = {key: base[key] for key in EXPOSURE + list(known_base) if key in base}
    original_advanced = {key: advanced[key] for key in known_advanced}
    print(f"Saved settings to restore: {original} {original_advanced}")
    stream = None
    try:
        camera.write_advanced(**known_advanced)
        camera.write(
            **known_base,
            auto_exposureEx=1,
            auto_gain_mode=1,
            manual_AGain_enable=1,
            manual_DGain_enable=1,
        )
        missing = sorted(set(KNOWN_BASE) - set(known_base) | set(KNOWN_ADVANCED) - set(known_advanced))
        print(f"Known image state set (not on this camera: {', '.join(missing) or 'none'}).")
        # It may have brightened the image a lot (gamma, say): treat it as a brightening.
        camera.brightened_at = time.monotonic()
        started = time.monotonic()
        print(f"Measuring {rtsp}.")
        stream = Stream(rtsp, args.crop)
        stream.wait_ready()
        camera.stream = stream
        camera.measure_latency(args.base_shutter, start[1])
        with open(args.out, "w", newline="") as file:
            log = csv.writer(file, lineterminator="\n")
            log.writerow(["shutter_1_over", "analog_gain", "digital_gain", "luma", "frames"])
            stops = calibrate(camera, log, args.base_shutter, args.max_stops, start)
    finally:
        if stream:
            stream.close()
        # Each write is tried on its own, so one rejected value cannot skip the other.
        failed = []
        for write, settings in ((camera.write_advanced, original_advanced), (camera.write, original)):
            try:
                write(**settings)
            except Exception as error:
                failed.append(f"{settings} ({error})")
        if failed:
            print("Could NOT restore these settings, set them back by hand:\n  " + "\n  ".join(failed))
        else:
            print("Restored the camera's settings.")

    print(
        f"\n{camera.readings} readings in {time.monotonic() - started:.0f} s. "
        f"Measurements in {args.out}. Paste into pro.rs:\n\n{rust_table(stops)}"
    )


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
