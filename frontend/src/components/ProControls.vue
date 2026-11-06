<template>
  <div
    ref="root"
    class="px-6 pt-4 pb-1"
  >
    <BlueExpansiblePanel
      title="Exposure"
      :expanded="panelsOpen.exposure"
      body-class="px-13 pt-4 pb-4"
      theme="dark"
      @update:expanded="setPanel('exposure', $event)"
    >
      <!-- BlueButtonGroup picks its selection only when it mounts (the first button when none
           is marked): each group waits for its value and remounts when it changes. -->
      <BlueButtonGroup
        v-if="pro"
        :key="`mode-${pro.mode}`"
        label="Mode"
        :disabled="controlsDisabled"
        :button-items="modeButtonItems"
        theme="dark"
        type="switch"
        class="mode-switch"
      />
      <!-- Shutter and ISO keep their place in every mode: set by hand, or read back from the
           camera (marked A) where it picks them. -->
      <ProListControl
        v-if="!program && pro?.shutter != null && shutters.length"
        label="Shutter"
        name="shutter"
        :items="shutters.map((shutter) => `${shutter} s`)"
        :index="nearestIndex(shutters.map(denominator), denominator(pro.shutter))"
        :disabled="controlsDisabled"
        :compact="compact"
        @update="setShutter(shutters[$event])"
      >
        <template #flag>
          <BlueTooltip
            v-if="uncalibratedReason(pro.shutter)"
            :text="uncalibratedReason(pro.shutter)!"
            theme="dark"
          >
            <span
              class="flag"
              tabindex="0"
            >uncalibrated</span>
          </BlueTooltip>
        </template>
      </ProListControl>
      <div
        v-else-if="program"
        class="readout"
      >
        <span class="readout-label">Shutter</span>
        <BlueTooltip
          :text="measuredShutterTip"
          theme="dark"
        >
          <span
            class="readout-value"
            tabindex="0"
          >{{ autoShutter }}</span>
        </BlueTooltip>
        <span
          class="readout-tag"
          title="Picked by the camera"
        >A</span>
      </div>
      <ProListControl
        v-if="manual && pro?.iso != null && isos.length"
        label="ISO"
        name="iso"
        :items="isos.map(String)"
        :index="nearestIndex(isos, pro.iso)"
        :disabled="controlsDisabled"
        :compact="compact"
        @update="send({ iso: isos[$event] })"
      />
      <div
        v-else-if="pro && !manual"
        class="readout"
      >
        <span class="readout-label">ISO</span>
        <BlueTooltip
          :text="measuredIsoTip"
          theme="dark"
        >
          <span
            class="readout-value"
            tabindex="0"
          >{{ autoIso }}</span>
        </BlueTooltip>
        <span
          class="readout-tag"
          title="Picked by the camera"
        >A</span>
        <span
          v-if="warningText"
          class="readout-warning"
          role="status"
        >{{ warningText }}</span>
      </div>
      <ProListControl
        v-if="!manual && pro?.exposure_compensation != null && compensations.length"
        label="Exposure comp."
        name="exposure-compensation"
        :items="compensations.map(formatEv)"
        :index="nearestIndex(compensations, pro.exposure_compensation)"
        :disabled="controlsDisabled"
        :compact="compact"
        @update="send({ exposure_compensation: compensations[$event] })"
      >
        <template #flag>
          <BlueButton
            v-if="pro.exposure_compensation !== 0 && compensations.includes(0)"
            variant="icon"
            icon="mdi-backup-restore"
            density="compact"
            theme="dark"
            tooltip="Back to 0 EV"
            class="ml-1"
            :disabled="controlsDisabled"
            @click="send({ exposure_compensation: 0 })"
          />
        </template>
      </ProListControl>
      <!-- Limits are set once per dive, not while flying: folded away, their values in view. -->
      <template v-if="!manual && pro?.max_iso != null">
        <button
          type="button"
          class="readout limits-summary"
          :aria-expanded="panelsOpen.limits"
          @click="setPanel('limits', !panelsOpen.limits)"
        >
          <span class="readout-label">Limits</span>
          <span class="readout-value">{{ limitsSummary }}</span>
          <span aria-hidden="true">{{ panelsOpen.limits ? '▾' : '▸' }}</span>
        </button>
        <template v-if="panelsOpen.limits">
          <ProListControl
            v-if="maxIsos.length"
            label="Max ISO"
            name="iso-limit"
            :items="maxIsos.map(formatMaxIso)"
            :index="maxIsoIndex"
            :disabled="controlsDisabled"
            :compact="compact"
            @update="send({ max_iso: maxIsos[$event] })"
          >
            <template #flag>
              <BlueTooltip
                v-if="pro.max_iso === 'full'"
                :text="FULL_GAIN_NOTE"
                theme="dark"
              >
                <span
                  class="flag flag-extended"
                  tabindex="0"
                >H = full gain</span>
              </BlueTooltip>
            </template>
          </ProListControl>
          <ProListControl
            v-if="program && pro.max_shutter != null && maxShutters.length"
            label="Slowest shutter"
            name="shutter-limit"
            :items="maxShutters.map((shutter) => `${shutter} s`)"
            :index="nearestIndex(maxShutters.map(denominator), denominator(pro.max_shutter))"
            :disabled="controlsDisabled"
            :compact="compact"
            @update="send({ max_shutter: maxShutters[$event] })"
          />
        </template>
      </template>
    </BlueExpansiblePanel>
    <BlueExpansiblePanel
      v-if="capabilities?.focus || capabilities?.zoom || capabilities?.tilt"
      title="Lens & Tilt"
      :expanded="panelsOpen.lens"
      body-class="px-13 pt-4 pb-4"
      theme="dark"
      @update:expanded="setPanel('lens', $event)"
    >
      <BlueSlider
        v-if="capabilities?.focus && pro?.focus != null"
        :model-value="pro.focus"
        :disabled="controlsDisabled"
        name="focus"
        label="Focus"
        :min="capabilities.focus.min"
        :max="capabilities.focus.max"
        :step="0.1"
        :keyboard-step-multiplier-limit="8"
        :format-display="(scaled: number) => `${Math.round(scaled)} %`"
        :scale-fn="(raw: number) => raw / 10"
        :unscale-fn="(scaled: number) => scaled * 10"
        label-min="Near"
        label-max="Far"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="send({ focus: $event as number })"
      />
      <BlueSlider
        v-if="capabilities?.zoom && pro?.zoom != null"
        :model-value="pro.zoom"
        :disabled="controlsDisabled"
        name="zoom"
        label="Zoom"
        :min="capabilities.zoom.min"
        :max="capabilities.zoom.max"
        :step="0.01"
        :keyboard-step-multiplier-limit="1"
        :format-display="(zoom: number) => `${zoom.toFixed(1)}x`"
        :label-min="`${capabilities.zoom.min}x`"
        :label-max="`${capabilities.zoom.max}x`"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="send({ zoom: $event as number })"
      />
      <BlueSlider
        v-if="capabilities?.tilt && pro?.tilt != null"
        :model-value="pro.tilt"
        :disabled="controlsDisabled"
        name="tilt"
        label="Tilt"
        :min="capabilities.tilt.min"
        :max="capabilities.tilt.max"
        :step="0.1"
        :keyboard-step-multiplier-limit="8"
        :format-display="(angle: number) => `${angle.toFixed(1)}°`"
        :label-min="`${capabilities.tilt.min}°`"
        :label-max="`${capabilities.tilt.max}°`"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="send({ tilt: $event as number })"
      />
    </BlueExpansiblePanel>
    <BlueExpansiblePanel
      title="White Balance"
      :expanded="panelsOpen.whiteBalance"
      body-class="px-13 pt-4 pb-4"
      theme="dark"
      @update:expanded="setPanel('whiteBalance', $event)"
    >
      <BlueButtonGroup
        v-if="pro"
        :key="`wb-mode-${pro.white_balance.mode}`"
        label="Mode"
        :disabled="controlsDisabled || wbBusy"
        :button-items="whiteBalanceModeButtonItems"
        theme="dark"
        type="switch"
      />
      <BlueButtonGroup
        v-if="pro?.white_balance.mode === 'auto'"
        :key="`wb-scene-${pro.white_balance.scene}`"
        label="Water"
        :disabled="controlsDisabled || wbBusy"
        :button-items="sceneButtonItems"
        theme="dark"
        type="switch"
      />
      <div
        v-if="pro?.white_balance.mode === 'manual'"
        class="flex flex-col items-end gap-[15px]"
      >
        <BlueSlider
          v-for="channel in RGB_CHANNELS"
          :key="channel"
          :model-value="pro.white_balance[channel] ?? 0"
          :disabled="controlsDisabled || wbBusy"
          :name="channel"
          :label="channel[0].toUpperCase() + channel.slice(1)"
          :min="0"
          :max="255"
          :step="1"
          :keyboard-step-multiplier-limit="2"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="setRgb(channel, $event as number)"
        />
      </div>
      <BlueSlider
        v-if="pro?.white_balance.mode === 'temperature' && kelvinRange"
        :model-value="pro.white_balance.kelvin ?? kelvinRange.min"
        :disabled="controlsDisabled || wbBusy"
        name="kelvin"
        label="Temperature"
        :min="kelvinRange.min"
        :max="kelvinRange.max"
        :step="KELVIN_STEP"
        :keyboard-step-multiplier-limit="10"
        :format-display="(kelvin: number) => `${Math.round(kelvin)} K`"
        :label-min="`${kelvinRange.min} K`"
        :label-max="`${kelvinRange.max} K`"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="setKelvin($event as number)"
      />
      <div
        v-if="pro?.color_temperature != null"
        class="readout"
      >
        <span class="readout-label">Scene light</span>
        <!-- Measured in every white balance mode (it stays live while Kelvin or Manual set
             the gains), so it does not follow the Kelvin control. -->
        <BlueTooltip
          text="The camera's estimate of the light's colour temperature, measured whatever the white balance is set to."
          theme="dark"
        >
          <span
            class="readout-value"
            tabindex="0"
          >≈ {{ pro.color_temperature }} K</span>
        </BlueTooltip>
        <!-- Close to what Auto would pick for this light, but then held fixed. -->
        <BlueButton
          v-if="pro.white_balance.mode === 'temperature' && kelvinRange"
          variant="text"
          density="compact"
          theme="dark"
          class="ml-auto"
          tooltip="Set the temperature to the scene light"
          :disabled="controlsDisabled || wbBusy || pro.white_balance.kelvin === clampKelvin(pro.color_temperature)"
          @click="setKelvin(pro.color_temperature)"
        >
          Match scene
        </BlueButton>
      </div>
      <div class="flex flex-col items-end">
        <!-- One-push measures the scene and holds the result as manual gains. -->
        <BlueButton
          :disabled="controlsDisabled || wbBusy"
          :loading="wbBusy"
          density="regular"
          theme="dark"
          class="max-w-full"
          @click="doOnePush"
        >
          {{ wbBusy ? 'Processing...' : 'One-Push White Balance' }}
        </BlueButton>
      </div>
    </BlueExpansiblePanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onUnmounted, ref, toRef, watch } from 'vue'
import { useElementBounding } from '@vueuse/core'
import {
  BlueButton,
  BlueButtonGroup,
  BlueExpansiblePanel,
  BlueSlider,
  BlueTooltip,
} from '@bluerobotics/bluevue'
import type {
  CameraStateEvent,
  ExposureMode,
  MaxIso,
  OnePushAwbStatus,
  ProCapabilities,
  ProSettings,
  ProState,
  WhiteBalanceScene,
} from '@/bindings/br4kcam_api'
import { backendClient } from '@/utils/backendClient'
import { formatRequestError } from '@/utils/formatRequestError'
import { useCameraState } from '@/utils/useCameraState'
import ProListControl from './ProListControl.vue'

const props = defineProps<{
  selectedCameraUuid: string | null
  disabled: boolean
  onePushAwb?: OnePushAwbStatus | null
}>()

const emit = defineEmits<{
  /** A change the camera or backend refused, worded for the user. */
  (event: 'error', message: string): void
}>()

const RGB_CHANNELS = ['red', 'green', 'blue'] as const

// Words for pilots, the letter for photographers (S, as on most bodies; Tv is Canon's).
const MODE_LABELS: Record<ExposureMode, { name: string; tooltip: string }> = {
  program: { name: 'P Auto', tooltip: 'Program: the camera picks shutter and ISO' },
  shutter: { name: 'S Shutter', tooltip: 'Shutter priority: you set the shutter, the camera picks ISO' },
  manual: { name: 'M Manual', tooltip: 'Manual: you set shutter and ISO' },
}

/** Shutter priority's first shutter, before one was picked there: natural motion at 30 fps. */
const SHUTTER_PRIORITY_DEFAULT = '1/60'
const SHUTTER_PRIORITY_KEY = 'proShutterPriorityShutter'

const FULL_GAIN_NOTE =
  "H: the camera's full gain, past the calibrated ISO 204800 (its ISO is not known)."

// Picked values are shown as camera bodies show them, in nominal third stops: the camera's
// own numbers (1/3968 for a 1/4000 limit) come from its 14.8 us sensor lines.
const THIRD_STOP_SHUTTERS = [
  2, 3, 4, 5, 6, 8, 10, 13, 15, 20, 25, 30, 40, 50, 60, 80, 100, 125, 160, 200, 250, 320, 400,
  500, 640, 800, 1000, 1250, 1600, 2000, 2500, 3200, 4000, 5000, 6400, 8000, 10000, 12800,
  16000, 20000, 25600, 32000,
]
const THIRD_STOP_ISOS = [
  100, 125, 160, 200, 250, 320, 400, 500, 640, 800, 1000, 1250, 1600, 2000, 2500, 3200, 4000,
  5000, 6400, 8000, 10000, 12800, 16000, 20000, 25600, 32000, 40000, 51200, 64000, 80000,
  102400, 128000, 160000, 204800,
]
/** The highest calibrated ISO: picked ISOs past it show as H. */
const TOP_CALIBRATED_ISO = 204800

/**
 * Below this drawn width the panel trades sliders for - / + steppers. Drawn, not laid out:
 * the page keeps its layout width and Cockpit zooms it into its widget (about 0.5 in a
 * first-run window), which shrinks the sliders without narrowing the layout.
 */
const COMPACT_WIDTH = 480

const SCENE_LABELS: Record<WhiteBalanceScene, string> = {
  green_water: 'Green',
  blue_water: 'Blue',
}

const pro = ref<ProState | null>(null)
const capabilities = ref<ProCapabilities | null>(null)
// Only Exposure opens by default: the tab often sits in a narrow panel beside the video.
// Which panels are open is remembered on this browser.
const PANELS_KEY = 'proControlsPanels'
const PANELS_DEFAULT = { exposure: true, limits: false, lens: false, whiteBalance: false }
type Panel = keyof typeof PANELS_DEFAULT
const readPanels = (): typeof PANELS_DEFAULT => {
  try {
    return { ...PANELS_DEFAULT, ...JSON.parse(localStorage.getItem(PANELS_KEY) ?? '{}') }
  } catch {
    return { ...PANELS_DEFAULT }
  }
}
const panelsOpen = ref(readPanels())
const setPanel = (panel: Panel, open: boolean) => {
  panelsOpen.value[panel] = open
  try {
    localStorage.setItem(PANELS_KEY, JSON.stringify(panelsOpen.value))
  } catch {
    // Storage can be unavailable (private mode): the panels just don't persist.
  }
}

const program = computed(() => pro.value?.mode === 'program')
const manual = computed(() => pro.value?.mode === 'manual')

const wbBusy = computed(() => props.onePushAwb != null)
const controlsDisabled = computed(() => props.disabled || pro.value === null)

const root = ref<HTMLElement | null>(null)
const { width: drawnWidth } = useElementBounding(root)
const compact = computed(() => drawnWidth.value > 0 && drawnWidth.value < COMPACT_WIDTH)

const modeButtonItems = computed(() =>
  (capabilities.value?.modes ?? []).map((mode) => ({
    ...MODE_LABELS[mode],
    preSelected: pro.value?.mode === mode,
    onSelected: () => {
      if (pro.value?.mode === mode) return
      if (mode === 'manual') send({ mode, ...nearestManualExposure() })
      // Not the shutter Auto is using: in dim water that is its slowest, blurriest one.
      else if (mode === 'shutter') send({ mode, shutter: shutterPriorityShutter() })
      else send({ mode })
    },
  })),
)

/** The shutter last set in Shutter priority on this browser, or its default. */
const shutterPriorityShutter = (): string => {
  try {
    const kept = localStorage.getItem(SHUTTER_PRIORITY_KEY)
    if (kept && shutters.value.includes(kept)) return kept
  } catch {
    // Storage can be unavailable (private mode): the default stands in.
  }
  return SHUTTER_PRIORITY_DEFAULT
}

const setShutter = (shutter: string) => {
  if (pro.value?.mode === 'shutter') {
    try {
      localStorage.setItem(SHUTTER_PRIORITY_KEY, shutter)
    } catch {
      // Not kept: Shutter priority starts from the default next time.
    }
  }
  send({ shutter })
}

/** The value in `values` closest to `value` in stops. */
const nearestInStops = (values: number[], value: number): number =>
  values.reduce((best, candidate) =>
    Math.abs(Math.log2(candidate / value)) < Math.abs(Math.log2(best / value)) ? candidate : best)

/**
 * The manual shutter and ISO closest to what the camera is using now, so switching to Manual
 * keeps the picture instead of jumping to the last manual values. Nearest in stops.
 */
const nearestManualExposure = (): ProSettings => {
  const live = pro.value
  const settings: ProSettings = {}
  // Calibrated shutters only: they expose what they say, so the picture holds.
  const candidates = calibratedShutters.value.length ? calibratedShutters.value : shutters.value
  if (live?.shutter && candidates.length) {
    const x = nearestInStops(candidates.map(denominator), 1 / exposureSeconds(live.shutter))
    settings.shutter = `1/${x}`
  }
  if (live?.iso && isos.value.length) settings.iso = nearestInStops(isos.value, live.iso)
  return settings
}

/** Seconds of a `"1/x"` or whole-seconds shutter. */
const exposureSeconds = (shutter: string): number =>
  shutter.startsWith('1/') ? 1 / Number(shutter.slice(2)) : Number(shutter)

// The shutter and ISO sliders move over the positions of the values the camera takes, so
// every stop is one step whatever the spacing of the values.
const shutters = computed(() => capabilities.value?.shutter ?? [])
const calibratedShutters = computed(() => capabilities.value?.shutter_calibrated ?? [])

/** Why a shutter may not expose what it says, or null when it is calibrated. */
const uncalibratedReason = (shutter: string): string | null => {
  if (!calibratedShutters.value.length || calibratedShutters.value.includes(shutter)) return null
  return exposureSeconds(shutter) > 1 / 30
    ? 'Longer than a frame at 30 fps: the camera holds it to about 1/30 s unless the frame rate is lowered.'
    : 'Faster than the camera exposes: it exposes no shorter than about 1/33000 s.'
}
const isos = computed(() => capabilities.value?.iso ?? [])
const maxIsos = computed(() => capabilities.value?.max_iso ?? [])
const maxShutters = computed(() => capabilities.value?.max_shutter ?? [])
const compensations = computed(() => capabilities.value?.exposure_compensation ?? [])

const formatMaxIso = (limit: MaxIso): string => (limit === 'full' ? 'H' : `${limit}`)

const maxIsoIndex = computed(() => {
  const limit = pro.value?.max_iso
  if (limit == null) return 0
  if (limit === 'full') return Math.max(0, maxIsos.value.indexOf('full'))
  return nearestIndex(maxIsos.value.map((each) => (each === 'full' ? Infinity : each)), limit)
})

const limitsSummary = computed(() => {
  const parts: string[] = []
  if (pro.value?.max_iso != null) parts.push(`ISO ≤ ${formatMaxIso(pro.value.max_iso)}`)
  if (program.value && pro.value?.max_shutter) parts.push(`slowest ${pro.value.max_shutter} s`)
  return parts.join(' · ')
})

/** Whether a picked ISO is past the calibrated top, by more than rounding to a third stop. */
const pastCalibrated = (iso: number): boolean => iso > TOP_CALIBRATED_ISO * 2 ** (1 / 6)

const autoShutter = computed(() => {
  const shutter = pro.value?.shutter
  if (!shutter) return '—'
  const seconds = exposureSeconds(shutter)
  if (seconds > 1 / 2) return `${shutter} s`
  return `1/${nearestInStops(THIRD_STOP_SHUTTERS, 1 / seconds)} s`
})

const measuredShutterTip = computed(() => {
  const shutter = pro.value?.shutter
  if (!shutter) return 'The camera does not say what it is using.'
  const micros = Math.round(exposureSeconds(shutter) * 1e6)
  return `Measured ${shutter} s (${micros} µs), shown in third stops.`
})

const autoIso = computed(() => {
  const iso = pro.value?.iso
  if (iso == null) return '—'
  return pastCalibrated(iso) ? 'H' : `${nearestInStops(THIRD_STOP_ISOS, iso)}`
})

const measuredIsoTip = computed(() => {
  const iso = pro.value?.iso
  if (iso == null) return 'The camera does not say what it is using.'
  return pastCalibrated(iso)
    ? `Measured ISO ${iso}: past the calibrated ISO 204800, extrapolated.`
    : `Measured ISO ${iso}, shown in third stops.`
})

/**
 * Why the picture is off target, once two reads in a row agree: auto exposure runs off for a
 * moment after every change.
 */
const exposureWarning = ref<ProState['exposure_warning']>(null)
let lastWarning: ProState['exposure_warning'] = null
const noteWarning = (warning: ProState['exposure_warning']) => {
  if (warning === lastWarning) exposureWarning.value = warning
  lastWarning = warning
}

const warningText = computed(() => {
  if (manual.value) return null
  switch (exposureWarning.value) {
    case 'too_dark':
      return program.value ? 'At limits – too dark' : 'ISO at max – too dark'
    case 'too_bright':
      return program.value ? 'Too bright' : 'Too bright – faster shutter'
    default:
      return null
  }
})

const formatEv = (ev: number): string => `${ev > 0 ? '+' : ''}${ev} EV`

/** The `x` of a `"1/x"` shutter. */
const denominator = (shutter: string): number => Number(shutter.split('/')[1])

/** Position of the value in `values` closest to `value` (the camera can hold one not listed). */
const nearestIndex = (values: number[], value: number): number =>
  values.reduce((best, candidate, index) =>
    Math.abs(candidate - value) < Math.abs(values[best] - value) ? index : best, 0)

const sceneButtonItems = computed(() =>
  (capabilities.value?.white_balance_scenes ?? []).map((scene) => ({
    name: SCENE_LABELS[scene],
    preSelected: pro.value?.white_balance.mode === 'auto' && pro.value.white_balance.scene === scene,
    onSelected: () => send({ white_balance: { mode: 'auto', scene } }),
  })),
)

const whiteBalanceModeButtonItems = computed(() => [
  {
    name: 'Auto',
    preSelected: pro.value?.white_balance.mode === 'auto',
    // Back to the scene the camera held in Manual, sent so the tab shows it at once.
    onSelected: () => {
      const current = pro.value?.white_balance
      if (!current || current.mode === 'auto') return
      send({ white_balance: { mode: 'auto', ...(current.scene ? { scene: current.scene } : {}) } })
    },
  },
  {
    name: 'Kelvin',
    tooltip: 'Gains for light of a colour temperature',
    preSelected: pro.value?.white_balance.mode === 'temperature',
    // From the camera's estimate of the scene, so the picture roughly holds.
    onSelected: () => {
      const current = pro.value?.white_balance
      if (!current || current.mode === 'temperature') return
      const estimate = pro.value?.color_temperature
      const kelvin = estimate != null && kelvinRange.value ? clampKelvin(estimate) : undefined
      send({
        white_balance: {
          mode: 'temperature',
          ...(kelvin != null ? { kelvin } : {}),
          ...(current.scene ? { scene: current.scene } : {}),
        },
      })
    },
  },
  {
    name: 'Manual',
    preSelected: pro.value?.white_balance.mode === 'manual',
    // No gains: the camera holds the ones it is using.
    onSelected: () => {
      if (pro.value?.white_balance.mode === 'manual') return
      send({ white_balance: { mode: 'manual' } })
    },
  },
])

/** The Kelvin slider's step: finer than the eye tells apart at daylight. */
const KELVIN_STEP = 100

const kelvinRange = computed(() => capabilities.value?.white_balance_kelvin ?? null)

/** `kelvin` on the slider's steps, within the camera's range. */
const clampKelvin = (kelvin: number): number => {
  const range = kelvinRange.value!
  return Math.min(range.max, Math.max(range.min, Math.round(kelvin / KELVIN_STEP) * KELVIN_STEP))
}

const setKelvin = (value: number) => {
  const current = pro.value?.white_balance
  if (current?.mode !== 'temperature') return
  const kelvin = clampKelvin(value)
  pro.value = { ...pro.value!, white_balance: { ...current, kelvin } }
  send({ white_balance: { mode: 'temperature', kelvin } })
}

const setRgb = (channel: (typeof RGB_CHANNELS)[number], value: number) => {
  const current = pro.value?.white_balance
  if (current?.mode !== 'manual') return
  // Optimistic display keeps the other gains; the request carries only this one.
  pro.value = { ...pro.value!, white_balance: { ...current, [channel]: value } }
  send({ white_balance: { mode: 'manual', [channel]: value } })
}

const path = (uuid: string, route = '') => `/pro/${uuid}${route}`

const load = async (uuid: string, withCapabilities: boolean) => {
  try {
    const [state, caps] = await Promise.all([
      backendClient.request<ProState>('GET', path(uuid)),
      withCapabilities
        ? backendClient.request<ProCapabilities>('GET', path(uuid, '/capabilities'))
        : Promise.resolve(capabilities.value),
    ])
    if (uuid !== props.selectedCameraUuid || inFlight) return
    pro.value = state
    capabilities.value = caps
    noteWarning(state.exposure_warning)
  } catch (error) {
    console.error('Error reading PRO controls:', error)
  }
}

/** `next` over `previous`, keeping manual gains `next` leaves out. */
const mergeSettings = (previous: ProSettings | null, next: ProSettings): ProSettings => {
  const before = previous?.white_balance
  const after = next.white_balance
  const whiteBalance = before?.mode === 'manual' && after?.mode === 'manual'
    ? { ...before, ...Object.fromEntries(Object.entries(after).filter(([, v]) => v != null)) }
    : after ?? before
  return { ...previous, ...next, ...(whiteBalance ? { white_balance: whiteBalance } : {}) } as ProSettings
}

// Latest wins: while a change is in flight, newer ones merge and go out next, so a
// dragged slider sends one request at a time.
let inFlight = false
let queued: ProSettings | null = null

const send = async (settings: ProSettings) => {
  const uuid = props.selectedCameraUuid
  if (!uuid || controlsDisabled.value || !pro.value) return
  // Show the change right away; the answer replaces it with what the camera took.
  const { white_balance: whiteBalance, ...rest } = settings
  pro.value = { ...pro.value, ...rest } as ProState
  if (whiteBalance && whiteBalance.mode !== 'manual') {
    pro.value.white_balance = { ...pro.value.white_balance, ...whiteBalance } as ProState['white_balance']
  }
  if (inFlight) {
    queued = mergeSettings(queued, settings)
    return
  }
  inFlight = true
  let next: ProSettings | null = settings
  while (next) {
    queued = null
    try {
      const state = await backendClient.request<ProState>('PATCH', path(uuid), next)
      if (uuid === props.selectedCameraUuid && !queued) pro.value = state
    } catch (error) {
      console.error('Error applying PRO controls:', error)
      emit('error', `Camera did not take the change: ${formatRequestError(error)}`)
    }
    next = queued
  }
  inFlight = false
  if (uuid !== props.selectedCameraUuid) return
  // A rejected change left the optimistic value on screen; read back what the camera has.
  void load(uuid, false)
}

const doOnePush = () => {
  const uuid = props.selectedCameraUuid
  if (!uuid || controlsDisabled.value || wbBusy.value) return
  backendClient.request('POST', path(uuid, '/white_balance/one_push')).catch((error) => {
    console.error('Error starting one-push white balance:', error)
    emit('error', `Could not set white balance: ${formatRequestError(error)}`)
  })
}

let refreshTimer: ReturnType<typeof setTimeout> | null = null
let refreshCapabilities = false

/** Re-read after changes from anywhere, coalescing bursts of events into one read. */
const scheduleRefresh = (withCapabilities: boolean) => {
  refreshCapabilities ||= withCapabilities
  if (refreshTimer) clearTimeout(refreshTimer)
  refreshTimer = setTimeout(() => {
    refreshTimer = null
    const uuid = props.selectedCameraUuid
    if (uuid) void load(uuid, refreshCapabilities)
    refreshCapabilities = false
  }, 300)
}

useCameraState(toRef(props, 'selectedCameraUuid'), (body: unknown) => {
  if (typeof body !== 'object' || body === null) return
  const data = body as CameraStateEvent
  if (data.camera_uuid !== props.selectedCameraUuid) return
  // Actuator positions stream while they move; the debounce reads once they settle.
  const setupChanged = data.actuators_config != null || data.actuators_configured != null
  if (data.base_parameters != null || data.actuators_state != null || setupChanged) {
    scheduleRefresh(setupChanged)
  }
})

watch(
  () => props.selectedCameraUuid,
  (uuid) => {
    pro.value = null
    capabilities.value = null
    exposureWarning.value = lastWarning = null
    if (uuid) void load(uuid, true)
  },
  { immediate: true },
)

// Where the camera picks shutter or ISO it moves them by itself, with no event to say so:
// re-read what it is using while the tab is open, often enough to keep up with the video.
const liveTimer = setInterval(() => {
  const uuid = props.selectedCameraUuid
  if (uuid && pro.value && pro.value.mode !== 'manual' && !inFlight) void load(uuid, false)
}, 1000)

onUnmounted(() => {
  if (refreshTimer) clearTimeout(refreshTimer)
  clearInterval(liveTimer)
})
</script>

<style scoped>
/* A read-only row lined up with the sliders' 130px label column. */
.readout {
  display: flex;
  align-items: center;
  gap: 8px;
  /* A slider row's height (BlueSlider's track is 30px), so switching modes doesn't move rows. */
  min-height: 30px;
}
.readout-label {
  width: 130px;
  flex-shrink: 0;
}
.readout-value {
  font-variant-numeric: tabular-nums;
}
/* A value the camera picked: dimmed, with its A right beside it. */
.readout-tag {
  padding: 0 5px;
  border: 1px solid currentColor;
  border-radius: 4px;
  font-size: 0.7rem;
  letter-spacing: 0.05em;
  opacity: 0.7;
}
/* The mode's letter (P, S, M) in the same square as a picked value's A. BlueButtonGroup
   takes plain names, so the square is drawn on the name's first letter. */
.mode-switch :deep([data-bbg-label])::first-letter {
  margin-right: 2px;
  padding: 0 5px;
  border: 1px solid currentColor;
  border-radius: 4px;
  font-size: 0.7rem;
  letter-spacing: 0.05em;
}
.readout:not(.limits-summary) .readout-value {
  opacity: 0.75;
}
.readout-warning {
  padding: 0 6px;
  border-radius: 4px;
  background: rgb(255 179 0 / 0.2);
  color: #ffc94d;
  font-size: 0.8rem;
}
.limits-summary {
  width: 100%;
  text-align: start;
  cursor: pointer;
}
.limits-summary .readout-value {
  flex: 1;
  opacity: 0.8;
}
/* One flag style for every note beside a label: uncalibrated, full gain. */
.flag {
  margin-left: 8px;
  font-size: 11px;
  white-space: nowrap;
  opacity: 0.7;
}
.flag-extended {
  color: #ffc94d;
  opacity: 1;
}
</style>
