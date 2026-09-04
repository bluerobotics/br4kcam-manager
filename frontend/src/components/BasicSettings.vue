<template>
  <div class="px-6 pt-4 pb-1">
    <div
      v-if="isConfigured === false"
      class="mb-4 rounded-[6px] border border-[#FB8C0055] bg-[#FB8C0022] p-3 text-[#FFB74D]"
    >
      <div class="flex items-center gap-2 text-sm font-medium">
        <BlueIcon
          name="mdi-alert"
          :size="18"
        />
        Hardware setup required
      </div>
      <div class="text-sm mt-1 opacity-90">
        Complete the Hardware setup section below to enable full camera controls.
      </div>
      <div class="mt-3">
        <BlueButton
          density="compact"
          theme="dark"
          @click="scrollToHardwareSetup"
        >
          Go to setup
        </BlueButton>
      </div>
    </div>
    <BlueExpansiblePanel
      title="Image"
      :expanded="panelsOpen.image"
      theme="dark"
      @update:expanded="panelsOpen.image = $event"
    >
      <BlueButtonGroup
        label="Water environment white balance"
        :disabled="isConfigured !== true || cameraControlsDisabled || wbBusy"
        :button-items="WhiteBalanceSceneButtonItems"
        theme="dark"
        type="switch"
      />
      <BlueButtonGroup
        label="White balance mode"
        :disabled="isConfigured !== true || cameraControlsDisabled || wbBusy"
        :button-items="whiteBalanceModeButtonItems"
        theme="dark"
        type="switch"
      />
      <div class="flex flex-col items-end">
        <BlueButton
          :disabled="cameraControlsDisabled || wbBusy"
          :loading="wbBusy"
          density="regular"
          theme="dark"
          class="max-w-full"
          @click="doWhiteBalance"
        >
          {{ onePushLabel }}
        </BlueButton>
      </div>
      <div
        v-if="baseParams.auto_awb === BaseAutoWhiteBalanceModeValue.Manual"
        class="flex flex-col items-end gap-[15px]"
      >
        <BlueSlider
          v-model="baseParams.awb_red"
          :disabled="isConfigured !== true || cameraControlsDisabled || wbBusy"
          name="red"
          label="Red"
          :min="0"
          :max="255"
          :step="1"
          :keyboard-step-multiplier-limit="2"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="updateBaseParameter('awb_red', $event as number)"
        />
        <BlueSlider
          v-model="baseParams.awb_blue"
          :disabled="isConfigured !== true || cameraControlsDisabled || wbBusy"
          name="blue"
          label="Blue"
          :min="0"
          :max="255"
          :step="1"
          :keyboard-step-multiplier-limit="2"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="updateBaseParameter('awb_blue', $event as number)"
        />
      </div>
    </BlueExpansiblePanel>
    <BlueExpansiblePanel
      title="Actuators"
      :expanded="panelsOpen.actuators"
      theme="dark"
      @update:expanded="panelsOpen.actuators = $event"
    >
      <BlueSlider
        v-if="actuatorsState"
        v-model="actuatorsState.focus"
        :disabled="actuatorControlsDisabled"
        name="focus"
        label="Focus"
        :min="0"
        :max="100"
        :step="0.1"
        :keyboard-step-multiplier-limit="8"
        :format-display="formatFocusValue"
        :scale-fn="scaleFocus"
        :unscale-fn="unscaleFocus"
        label-min="Close"
        label-max="Far"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateActuatorsState('focus', $event as number)"
      />
      <BlueSlider
        v-if="actuatorsState"
        v-model="actuatorsState.zoom"
        :disabled="actuatorControlsDisabled"
        name="zoom"
        label="Zoom"
        :min="0"
        :max="100"
        :step="1"
        :format-display="formatZoomValue"
        :scale-fn="scaleZoom"
        :unscale-fn="unscaleZoom"
        :keyboard-step-multiplier-limit="1"
        label-min="1x"
        label-max="2x"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateActuatorsState('zoom', $event as number)"
      />
      <BlueSlider
        v-if="actuatorsState && false"
        v-model="actuatorsState.tilt"
        :disabled="actuatorControlsDisabled"
        name="tilt"
        label="Tilt"
        :min="0"
        :max="100"
        :step="1"
        :format-display="formatTiltValue"
        :scale-fn="scaleTilt"
        :unscale-fn="unscaleTilt"
        :label-min="`${currentFocusAndZoomParams.tilt_mnt_pitch_min}` || ''"
        :label-max="`${currentFocusAndZoomParams.tilt_mnt_pitch_max}` || ''"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateActuatorsState('tilt', $event as number)"
      />
      <BlueExpansiblePanel
        title="more"
        header-align="end"
        body-class="pt-4 pb-8"
        :expanded="panelsOpen.actuatorsMore"
        theme="dark"
        @update:expanded="panelsOpen.actuatorsMore = $event"
      >
        <div>
          <BlueSwitch
            v-model="currentFocusAndZoomParams.enable_focus_and_zoom_correlation"
            :disabled="actuatorControlsDisabled"
            name="focus-zoom-correlation"
            label="Enable focus and zoom correlation"
            theme="dark"
            @update:model-value="updateActuatorsConfig('enable_focus_and_zoom_correlation', $event)"
          />
          <!-- <BlueSlider
            v-model="focusOffsetUI"
            :disabled="isConfigured !== true || cameraControlsDisabled"
            name="focus-offset"
            label="Focus compensation"
            :min="-10"
            :max="10"
            :step="1"
            width="400px"
            theme="dark"
            value-weight="regular"
            label-width="130px"
            class="mt-6"
            @update:model-value="onFocusOffsetChange($event ?? 0)"
          /> -->
        </div>
      </BlueExpansiblePanel>
    </BlueExpansiblePanel>
    <BlueExpansiblePanel
      title="Video"
      :expanded="panelsOpen.video"
      theme="dark"
      @update:expanded="panelsOpen.video = $event"
    >
      <BlueSelect
        v-model="selectedVideoResolution"
        :disabled="isConfigured !== true || cameraControlsDisabled"
        label="Resolution"
        :items="resolutionOptions || [{ name: 'No resolutions available', value: null }]"
        theme="dark"
        @update:model-value="(value: any) => handleVideoChanges('resolution', value)"
      />
      <BlueSelect
        v-model="selectedVideoBitrate"
        :disabled="isConfigured !== true || cameraControlsDisabled"
        label="Bitrate"
        :items="bitrateOptions || [{ name: 'No bitrates available', value: null }]"
        theme="dark"
        @update:model-value="(value: any) => handleVideoChanges('bitrate', value)"
      >
        <template #insetElement>
          <BlueButton
            variant="icon"
            icon="mdi-information-outline"
            tooltip="H.264 bitrate options"
            density="compact"
            theme="dark"
            class="shrink-0 ml-auto mr-2"
            @click="bitrateInfoOpen = true"
          />
        </template>
      </BlueSelect>
      <div
        v-if="hasUnsavedVideoChanges"
        class="flex justify-end mb-[-20px]"
      >
        <BlueButton
          :disabled="isConfigured !== true || cameraControlsDisabled"
          variant="filled"
          density="compact"
          theme="dark"
          @click="saveVideoDataAndRestart"
        >
          Save and restart camera
        </BlueButton>
      </div>
    </BlueExpansiblePanel>
    <BlueExpansiblePanel
      ref="hardwareSetupPanel"
      title="Hardware setup"
      :expanded="panelsOpen.hardware"
      theme="dark"
      @update:expanded="panelsOpen.hardware = $event"
    >
      <div>
        <p class="mb-3">
          Assign Navigator PWM output channels to your camera functions below. The recommended setup is:
        </p>
        <ul class="list-disc pl-5 mb-4 text-base">
          <li><b>Focus</b>: Connect the camera's Focus cable to Navigator's <b>PWM Channel 10</b></li>
          <li><b>Zoom</b>: Connect the camera's Zoom cable to Navigator's <b>PWM Channel 11</b></li>
          <li><b>Script</b>: Navigator's <b>PWM Channel 12</b> is used as an <i>input</i> used by the internal Lua script that enables Focus/Zoom correlation (no physical cable connects here)</li>
          <li><b>Tilt</b>: Connect the camera's Tilt cable to Navigator's <b>PWM Channel 16</b></li>
        </ul>
        <p class="mb-3">
          Click <b>Apply default hardware setup</b> to use the recommended configuration above, or click <b>Advanced setup</b> to customize your channel assignments and parameters.
        </p>
      </div>

      <!-- Default Simple Setup -->
      <div
        v-if="!showAdvancedHardware"
        class="px-3 pt-3"
      >
        <p
          v-if="hardwareSetupDisabledReason"
          class="text-sm opacity-80 mb-3 text-right"
        >
          {{ hardwareSetupDisabledReason }}
        </p>
        <div class="flex flex-row gap-3 mt-5 justify-end">
          <BlueButton
            :disabled="hardwareSetupControlsDisabled"
            density="compact"
            theme="dark"
            @click="showAdvancedHardware = true"
          >
            Advanced setup
          </BlueButton>
          <BlueButton
            variant="filled"
            density="compact"
            :disabled="hardwareSetupControlsDisabled"
            :loading="props.loading"
            theme="dark"
            @click="resetToRecommendedDefaults"
          >
            Apply default hardware setup
          </BlueButton>
        </div>
      </div>

      <!-- Advanced Setup -->
      <div v-else>
        <!-- Focus Group -->
        <BlueExpansiblePanel
          title="Focus"
          expanded
          theme="dark"
        >
          <BlueSelect
            v-model="intendedFocusAndZoomParams.focus_channel"
            :disabled="hardwareSetupControlsDisabled"
            label="PWM Output Channel"
            :items="availableServoChannelOptions"
            :error-messages="channelErrors.focus_channel ? [channelErrors.focus_channel] : []"
            theme="dark"
            @update:model-value="handleChannelChanges('focus_channel', $event)"
          />
          <div class="flex flex-row gap-3">
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Min (µs)
              <BlueInput
                v-model="focusChannelMin"
                name="focus-channel-min"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Trim (µs)
              <BlueInput
                v-model="focusChannelTrim"
                name="focus-channel-trim"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Max (µs)
              <BlueInput
                v-model="focusChannelMax"
                name="focus-channel-max"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
          </div>
          <label class="flex min-w-0 flex-col gap-1 text-xs opacity-70">
            Focus Margin Gain
            <BlueInput
              v-model="focusMarginGain"
              name="focus-margin-gain"
              :disabled="hardwareSetupControlsDisabled"
              type="number"
              width="100%"
              theme="dark"
            />
          </label>
        </BlueExpansiblePanel>

        <!-- Zoom Group -->
        <BlueExpansiblePanel
          title="Zoom"
          expanded
          theme="dark"
        >
          <BlueSelect
            v-model="intendedFocusAndZoomParams.zoom_channel"
            :disabled="hardwareSetupControlsDisabled"
            label="PWM Output Channel"
            :items="availableServoChannelOptions"
            :error-messages="channelErrors.zoom_channel ? [channelErrors.zoom_channel] : []"
            theme="dark"
            @update:model-value="handleChannelChanges('zoom_channel', $event)"
          />
          <div class="flex flex-row gap-3">
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Min (µs)
              <BlueInput
                v-model="zoomChannelMin"
                name="zoom-channel-min"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Trim (µs)
              <BlueInput
                v-model="zoomChannelTrim"
                name="zoom-channel-trim"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Max (µs)
              <BlueInput
                v-model="zoomChannelMax"
                name="zoom-channel-max"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
          </div>
        </BlueExpansiblePanel>

        <!-- Script Group -->
        <BlueExpansiblePanel
          title="Script"
          expanded
          theme="dark"
        >
          <BlueSelect
            v-model="intendedFocusAndZoomParams.script_channel"
            :disabled="hardwareSetupControlsDisabled"
            label="PWM Input Channel"
            :items="availableServoChannelOptions"
            :error-messages="channelErrors.script_channel ? [channelErrors.script_channel] : []"
            theme="dark"
            @update:model-value="handleChannelChanges('script_channel', $event)"
          />
          <div class="flex flex-row gap-3">
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Min (µs)
              <BlueInput
                v-model="scriptChannelMin"
                name="script-channel-min"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Trim (µs)
              <BlueInput
                v-model="scriptChannelTrim"
                name="script-channel-trim"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Max (µs)
              <BlueInput
                v-model="scriptChannelMax"
                name="script-channel-max"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
          </div>
          <div class="flex flex-col gap-[15px]">
            <BlueSelect
              v-model="intendedFocusAndZoomParams.script_function"
              :disabled="hardwareSetupControlsDisabled"
              label="Script Function"
              :items="scriptFunctionOptions"
              theme="dark"
            />
            <BlueSelect
              v-model="intendedFocusAndZoomParams.camera_id"
              :disabled="props.disabled || true"
              label="Camera ID"
              :items="cameraIdOptions"
              theme="dark"
            />
            <BlueSwitch
              v-model="intendedFocusAndZoomParams.enable_focus_and_zoom_correlation"
              :disabled="hardwareSetupControlsDisabled"
              name="focus-zoom-correlation"
              label="Focus/Zoom Correlation"
              theme="dark"
            />
          </div>
        </BlueExpansiblePanel>

        <!-- Tilt Group -->
        <BlueExpansiblePanel
          title="Tilt"
          expanded
          theme="dark"
        >
          <BlueSelect
            v-model="intendedFocusAndZoomParams.tilt_channel"
            :disabled="hardwareSetupControlsDisabled"
            label="PWM Output Channel"
            :items="availableServoChannelOptions"
            :error-messages="channelErrors.tilt_channel ? [channelErrors.tilt_channel] : []"
            theme="dark"
            @update:model-value="handleChannelChanges('tilt_channel', $event)"
          />
          <div class="flex flex-row gap-3">
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Min (µs)
              <BlueInput
                v-model="tiltChannelMin"
                name="tilt-channel-min"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Trim (µs)
              <BlueInput
                v-model="tiltChannelTrim"
                name="tilt-channel-trim"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Max (µs)
              <BlueInput
                v-model="tiltChannelMax"
                name="tilt-channel-max"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
          </div>
          <div class="flex flex-row gap-3">
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Pitch Min (°)
              <BlueInput
                v-model="tiltMntPitchMin"
                name="tilt-mnt-pitch-min"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-xs opacity-70">
              Pitch Max (°)
              <BlueInput
                v-model="tiltMntPitchMax"
                name="tilt-mnt-pitch-max"
                :disabled="hardwareSetupControlsDisabled"
                type="number"
                width="100%"
                theme="dark"
              />
            </label>
          </div>
          <div class="flex flex-col gap-[15px]">
            <BlueSwitch
              v-model="intendedFocusAndZoomParams.tilt_channel_reversed"
              :disabled="hardwareSetupControlsDisabled"
              name="tilt-channel-reversed"
              label="Reverse Direction"
              theme="dark"
            />
            <BlueSelect
              v-model="intendedFocusAndZoomParams.tilt_mnt_type"
              :disabled="hardwareSetupControlsDisabled"
              label="Mount Type"
              :items="mountTypeOptions"
              theme="dark"
            />
          </div>
        </BlueExpansiblePanel>

        <!-- Action Buttons -->
        <div class="flex flex-col items-end gap-3 mt-5">
          <p
            v-if="hardwareSetupDisabledReason"
            class="text-sm opacity-80 mb-0"
          >
            {{ hardwareSetupDisabledReason }}
          </p>
          <div class="flex flex-row gap-3 justify-end">
            <BlueButton
              :disabled="hardwareSetupControlsDisabled"
              density="compact"
              theme="dark"
              @click="showAdvancedHardware = false"
            >
              Back to simple
            </BlueButton>
            <BlueButton
              variant="filled"
              density="compact"
              :disabled="hasChannelErrors || hardwareSetupControlsDisabled"
              :loading="props.loading"
              theme="dark"
              @click="saveHardwareSetup"
            >
              Apply custom hardware setup
            </BlueButton>
          </div>
        </div>
      </div>
    </BlueExpansiblePanel>
  </div>

  <BlueDialog
    v-model="bitrateInfoOpen"
    title="H.264 Bitrate Options"
    width="550px"
    body-class="px-0 pb-2"
  >
    <table class="w-full border-collapse text-[16px]">
      <thead>
        <tr>
          <th class="border-b border-gray-600 pb-1 pl-4 text-left text-[14px]">
            Resolution
          </th>
          <th class="border-b border-gray-600 pb-1 text-center text-[14px]">
            High
          </th>
          <th class="border-b border-gray-600 pb-1 text-center text-[14px]">
            Medium
          </th>
          <th class="border-b border-gray-600 pb-1 pr-4 text-center text-[14px]">
            Low
          </th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="row in h264BitrateTable"
          :key="row.resolution"
          class="border-t-[1px] border-[#ffffff11]"
        >
          <td class="py-1 pl-4 pt-2 text-[16px]">
            {{ row.resolution }}<br>
            <span class="text-[14px] opacity-70">Disk usage</span>
          </td>
          <td class="text-center">
            {{ row.high.bitrate }} kbps<br>
            <span class="opacity-70">{{ row.high.storage }} GB/h</span>
          </td>
          <td class="text-center">
            {{ row.medium.bitrate }} kbps<br>
            <span class="opacity-70">{{ row.medium.storage }} GB/h</span>
          </td>
          <td class="pr-4 text-center">
            {{ row.low.bitrate }} kbps<br>
            <span class="opacity-70">{{ row.low.storage }} GB/h</span>
          </td>
        </tr>
      </tbody>
    </table>
  </BlueDialog>

  <WelcomeDialog
    :show="showWelcomeOverlay"
    @close="showWelcomeDialog = false"
    @go-to-setup="onWelcomeGoToSetup"
  />
</template>

<script setup lang="ts">
import { computed, ref, toRef, watch } from 'vue'
import {
  BlueButton,
  BlueButtonGroup,
  BlueDialog,
  BlueExpansiblePanel,
  BlueIcon,
  BlueInput,
  BlueSelect,
  BlueSlider,
  BlueSwitch,
} from '@bluerobotics/bluevue'
import { type BaseParameterSetting, type VideoParameterSettings, type VideoResolutionValue, BaseAutoWhiteBalanceModeValue, BaseAutoWhiteBalanceSceneValue, type AdvancedParameterSetting, type CameraControl } from '@/bindings/br4kcam'
import { backendClient } from '@/utils/backendClient'
import {
  createActuatorTokenSource,
  ownsActuatorFlight,
  shouldRollbackActuatorUi,
  type ActuatorFlight,
} from '@/utils/actuatorFlight'
import { createPendingFields } from '@/utils/pendingFields'
import { rebootCamera } from '@/utils/rebootCamera'
import { useCameraState } from '@/utils/useCameraState'
import type { ActuatorsConfig, ActuatorsControl, ActuatorsParametersConfig, ActuatorsState, CameraID, MountType, ScriptFunction, ServoChannel } from '@/bindings/autopilot'
import type { CameraStateEvent, OnePushAwbStatus } from '@/bindings/br4kcam_api'
import WelcomeDialog from './WelcomeDialog.vue'
import {
  autopilotDependentControlsBlocked,
  useSystemHealth,
} from '@/utils/useSystemHealth'


const props = defineProps<{
  selectedCameraUuid: string | null
  disabled: boolean
  cameraControlsDisabled: boolean
  loading: boolean
  cockpitMode: boolean
  onePushAwb?: OnePushAwbStatus | null
  backendConnected: boolean
  welcomeOverlayUnblocked: boolean
}>()

const { autopilotState, systemHealth } = useSystemHealth()

const wbBusy = computed(() => props.onePushAwb != null)
const onePushLabel = computed(() => {
  if (props.onePushAwb != null) return 'Processing...'
  return 'One-Push White Balance'
})

interface ServoChannelOption {
  name: string
  value: ServoChannel
}

const servoChannelOptions: ServoChannelOption[] = Array.from({ length: 16 }, (_, i) => ({
  name: `Channel ${i + 1}`,
  value: `SERVO${i + 1}` as ServoChannel,
}))

const baseParams = ref<BaseParameterSetting>({
  hue: null,
  brightness: null,
  sharpness: null,
  contrast: null,
  saturation: null,
  gamma: null,
  blc_level: null,
  max_exposure: null,
  set_default: null,
  antiFog: null,
  frameTurbo_pro: null,
  sceneMode: null,
  AE_strategy_mode: null,
  auto_exposureEx: null,
  exposure_time: null,
  auto_awb: null,
  awb_red: null,
  awb_green: null,
  awb_blue: null,
  awb_auto_mode: null,
  awb_style_red: null,
  awb_style_green: null,
  awb_style_blue: null,
  auto_gain_mode: null,
  auto_DGain_max: null,
  auto_AGain_max: null,
  max_sys_gain: null,
  manual_AGain_enable: null,
  manual_AGain: null,
  manual_DGain_enable: null,
  manual_DGain: null,
  rotate: null,
})

const currentFocusAndZoomParams = ref<ActuatorsParametersConfig>({
  camera_id: null,
  focus_channel: null,
  focus_channel_min: null,
  focus_channel_trim: null,
  focus_channel_max: null,
  focus_margin_gain: null,
  script_function: null,
  script_channel: null,
  script_channel_min: null,
  script_channel_trim: null,
  script_channel_max: null,
  enable_focus_and_zoom_correlation: null,
  zoom_channel: null,
  zoom_channel_min: null,
  zoom_channel_trim: null,
  zoom_channel_max: null,
  tilt_channel: null,
  tilt_channel_min: null,
  tilt_channel_trim: null,
  tilt_channel_max: null,
  tilt_channel_reversed: null,
  tilt_mnt_type: null,
  tilt_mnt_pitch_min: null,
  tilt_mnt_pitch_max: null,
})

const selectedVideoResolution = ref<VideoResolutionValue | null>(null)
const selectedVideoBitrate = ref<number | null>(null)
const hasUserEditedVideo = ref<boolean>(false)
const selectedVideoParameters = ref<VideoParameterSettings>({})
const downloadedVideoParameters = ref<VideoParameterSettings>({})
const actuatorsState = ref<ActuatorsState>({
  focus: 0,
  zoom: 0,
  tilt: 0,
})
type ActuatorKey = keyof ActuatorsState

/** Match SERVO feedback within half a UI step (focus 0.1, zoom/tilt 1). */
const actuatorMatchEpsilon: Record<ActuatorKey, number> = {
  focus: 0.05,
  zoom: 0.5,
  tilt: 0.5,
}

const approxEqual = (a: number, b: number, key: ActuatorKey = 'zoom'): boolean =>
  Math.abs(a - b) <= actuatorMatchEpsilon[key]

// Tracks the last user-requested value per actuator. While a key is pending, we do not let
// pushed state updates overwrite the UI with intermediate/stale values.
const desiredActuatorsState = ref<Record<ActuatorKey, number | null>>({
  focus: null,
  zoom: null,
  tilt: null,
})
/** Drop desired latch if SERVO never enters epsilon (stuck / unreachable). */
const DESIRED_LATCH_TIMEOUT_MS = 5_000
const desiredLatchTimers: Partial<Record<ActuatorKey, number>> = {}
/** Value to restore on failed POST when no newer command is pending. */
const actuatorsSetRollback = ref<Record<ActuatorKey, number | null>>({
  focus: null,
  zoom: null,
  tilt: null,
})

const clearDesiredLatch = (key: ActuatorKey): void => {
  desiredActuatorsState.value[key] = null
  const timer = desiredLatchTimers[key]
  if (timer != null) {
    clearTimeout(timer)
    delete desiredLatchTimers[key]
  }
}

const armDesiredLatch = (key: ActuatorKey, value: number): void => {
  desiredActuatorsState.value[key] = value
  const timer = desiredLatchTimers[key]
  if (timer != null) clearTimeout(timer)
  desiredLatchTimers[key] = window.setTimeout(() => {
    delete desiredLatchTimers[key]
    if (
      desiredActuatorsState.value[key] !== null &&
      approxEqual(desiredActuatorsState.value[key]!, value, key)
    ) {
      desiredActuatorsState.value[key] = null
    }
  }, DESIRED_LATCH_TIMEOUT_MS)
}

// Coalesce actuator set requests so only one request per actuator can be in-flight, always
// sending the most recent value (prevents request pile-up).
const actuatorTokens = createActuatorTokenSource()
const actuatorsSetInFlight = ref<Record<ActuatorKey, ActuatorFlight | null>>({
  focus: null,
  zoom: null,
  tilt: null,
})
const actuatorsSetQueued = ref<Record<ActuatorKey, number | null>>({
  focus: null,
  zoom: null,
  tilt: null,
})
/** Bumped on camera switch so in-flight actuator POSTs ignore stale `.finally` cleanup. */
const actuatorsRequestGeneration = ref(0)
const pendingBase = createPendingFields<keyof BaseParameterSetting, unknown>()
const correlationLatch = ref<{ token: number; value: boolean | null } | null>(null)
let nextCorrelationToken = 1
const isConfigured = ref<boolean | null>(null)
/** Sticky expand state — set once when configuration is first known, never flapped by connectivity. */
const panelsOpen = ref({
  image: true,
  actuators: true,
  actuatorsMore: true,
  video: true,
  hardware: false,
})

/**
 * Setup-first while unconfigured, day-to-day panels once configured.
 *
 * Cockpit runs in a short iframe, so the video panel stays collapsed there.
 */
const applyPanelLayout = (configured: boolean): void => {
  panelsOpen.value = {
    image: configured,
    actuators: configured,
    actuatorsMore: configured && !props.cockpitMode,
    video: configured && !props.cockpitMode,
    hardware: !configured,
  }
}

watch(isConfigured, (value, previous) => {
  if (previous !== null || value === null) return
  applyPanelLayout(value === true)
})

const autopilotBlocksControls = computed(() =>
  autopilotDependentControlsBlocked(systemHealth.value?.autopilot),
)

const actuatorControlsDisabled = computed(
  () => isConfigured.value !== true || props.disabled || autopilotBlocksControls.value,
)
const hardwareSetupControlsDisabled = computed(
  () => props.disabled || autopilotBlocksControls.value,
)
const hardwareSetupDisabledReason = computed(() => {
  if (!hardwareSetupControlsDisabled.value) return null
  if (!props.backendConnected) return 'Connect to the backend to apply hardware setup.'
  if (autopilotState.value === 'syncing') {
    return 'Waiting for the autopilot to finish syncing parameters…'
  }
  if (autopilotState.value !== 'online') {
    return 'Autopilot must be online to apply hardware setup.'
  }
  return null
})
const showWelcomeDialog = ref<boolean>(true)
const showWelcomeOverlay = computed(
  () =>
    props.welcomeOverlayUnblocked
    && isConfigured.value === false
    && showWelcomeDialog.value
    && autopilotState.value !== 'syncing',
)
const hardwareSetupPanel = ref<InstanceType<typeof BlueExpansiblePanel> | null>(null)

const scrollToHardwareSetup = (): void => {
  panelsOpen.value = { ...panelsOpen.value, hardware: true }
  hardwareSetupPanel.value?.$el.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

const onWelcomeGoToSetup = (): void => {
  scrollToHardwareSetup()
  showWelcomeDialog.value = false
}
const showAdvancedHardware = ref(false)
const bitrateInfoOpen = ref(false)
const intendedFocusAndZoomParams = ref<ActuatorsParametersConfig>({
  camera_id: null,
  focus_channel: null,
  focus_channel_min: null,
  focus_channel_trim: null,
  focus_channel_max: null,
  focus_margin_gain: null,
  script_function: null,
  script_channel: null,
  script_channel_min: null,
  script_channel_trim: null,
  script_channel_max: null,
  enable_focus_and_zoom_correlation: null,
  zoom_channel: null,
  zoom_channel_min: null,
  zoom_channel_trim: null,
  zoom_channel_max: null,
  tilt_channel: null,
  tilt_channel_min: null,
  tilt_channel_trim: null,
  tilt_channel_max: null,
  tilt_channel_reversed: null,
  tilt_mnt_type: null,
  tilt_mnt_pitch_min: null,
  tilt_mnt_pitch_max: null,
})

type NumericParamKey = {
  [K in keyof ActuatorsParametersConfig]: ActuatorsParametersConfig[K] extends number | null ? K : never
}[keyof ActuatorsParametersConfig]

// BlueInput types its emit as widely as its own field allows, so a numeric setting reaches the
// config through a narrowing bridge rather than fifteen inline handlers.
const numberField = (key: NumericParamKey) =>
  computed({
    get: (): number | null => intendedFocusAndZoomParams.value[key],
    set: (value: string | number | null) => {
      const parsed = Number(value)
      intendedFocusAndZoomParams.value[key] = Number.isFinite(parsed) ? parsed : null
    },
  })

const focusChannelMin = numberField('focus_channel_min')
const focusChannelTrim = numberField('focus_channel_trim')
const focusChannelMax = numberField('focus_channel_max')
const focusMarginGain = numberField('focus_margin_gain')
const zoomChannelMin = numberField('zoom_channel_min')
const zoomChannelTrim = numberField('zoom_channel_trim')
const zoomChannelMax = numberField('zoom_channel_max')
const scriptChannelMin = numberField('script_channel_min')
const scriptChannelTrim = numberField('script_channel_trim')
const scriptChannelMax = numberField('script_channel_max')
const tiltChannelMin = numberField('tilt_channel_min')
const tiltChannelTrim = numberField('tilt_channel_trim')
const tiltChannelMax = numberField('tilt_channel_max')
const tiltMntPitchMin = numberField('tilt_mnt_pitch_min')
const tiltMntPitchMax = numberField('tilt_mnt_pitch_max')

const defaultFocusAndZoomParams = ref<ActuatorsParametersConfig>({
  camera_id: null,
  focus_channel: null,
  focus_channel_min: null,
  focus_channel_trim: null,
  focus_channel_max: null,
  focus_margin_gain: null,
  script_function: null,
  script_channel: null,
  script_channel_min: null,
  script_channel_trim: null,
  script_channel_max: null,
  enable_focus_and_zoom_correlation: null,
  zoom_channel: null,
  zoom_channel_min: null,
  zoom_channel_trim: null,
  zoom_channel_max: null,
  tilt_channel: null,
  tilt_channel_min: null,
  tilt_channel_trim: null,
  tilt_channel_max: null,
  tilt_channel_reversed: null,
  tilt_mnt_type: null,
  tilt_mnt_pitch_min: null,
  tilt_mnt_pitch_max: null,
})
const hasUnsavedVideoChanges = ref<boolean>(false)

watch(
  () => props.selectedCameraUuid,
  () => {
    hasUserEditedVideo.value = false
    hasUnsavedVideoChanges.value = false
    actuatorsRequestGeneration.value += 1
    pendingBase.clear()
    correlationLatch.value = null
    isConfigured.value = null
    showAdvancedHardware.value = false
    const emptyParams: ActuatorsParametersConfig = {
      camera_id: null,
      focus_channel: null,
      focus_channel_min: null,
      focus_channel_trim: null,
      focus_channel_max: null,
      focus_margin_gain: null,
      script_function: null,
      script_channel: null,
      script_channel_min: null,
      script_channel_trim: null,
      script_channel_max: null,
      enable_focus_and_zoom_correlation: null,
      zoom_channel: null,
      zoom_channel_min: null,
      zoom_channel_trim: null,
      zoom_channel_max: null,
      tilt_channel: null,
      tilt_channel_min: null,
      tilt_channel_trim: null,
      tilt_channel_max: null,
      tilt_channel_reversed: null,
      tilt_mnt_type: null,
      tilt_mnt_pitch_min: null,
      tilt_mnt_pitch_max: null,
    }
    intendedFocusAndZoomParams.value = { ...emptyParams }
    currentFocusAndZoomParams.value = { ...emptyParams }
    defaultFocusAndZoomParams.value = { ...emptyParams }
    ;(['focus', 'zoom', 'tilt'] as const).forEach(clearDesiredLatch)
    actuatorsSetRollback.value = { focus: null, zoom: null, tilt: null }
    actuatorsSetQueued.value = { focus: null, zoom: null, tilt: null }
    actuatorsSetInFlight.value = { focus: null, zoom: null, tilt: null }
  },
)

const resolutionOptions = ref([
  { name: '3840x2160', value: { width: 3840, height: 2160 } },
  { name: '1920x1080', value: { width: 1920, height: 1080 } },
])

const resolutionsToBitrate: Record<string, number[]> = {
  '3840x2160': [16384, 8192, 4096],
  '1920x1080': [8192, 4096, 2048],
}

const h264BitrateTable = [
  { resolution: '3840x2160', high: { bitrate: 16384, storage: 7.2 }, medium: { bitrate: 8192, storage: 3.6 }, low: { bitrate: 4096, storage: 1.8 } },
  { resolution: '1920x1080', high: { bitrate: 8192, storage: 3.6 }, medium: { bitrate: 4096, storage: 1.8 }, low: { bitrate: 2048, storage: 0.9 } }
]

const WhiteBalanceSceneButtonItems = computed(() => [
  { 
    name: 'Green',
    preSelected: baseParams.value.awb_auto_mode === BaseAutoWhiteBalanceSceneValue.Scene1,
    onSelected: () => (updateBaseParameter('awb_auto_mode', BaseAutoWhiteBalanceSceneValue.Scene1))
  },
  { 
    name: 'Blue',
    preSelected: baseParams.value.awb_auto_mode === BaseAutoWhiteBalanceSceneValue.Scene2,
    onSelected: () => (updateBaseParameter('awb_auto_mode', BaseAutoWhiteBalanceSceneValue.Scene2))
  }
])

const whiteBalanceModeButtonItems = computed(() => [
  { 
    name: 'Auto',
    preSelected: baseParams.value.auto_awb === BaseAutoWhiteBalanceModeValue.Auto,
    onSelected: () => (updateBaseParameter('auto_awb', BaseAutoWhiteBalanceModeValue.Auto))
  },
  { 
    name: 'Manual',
    preSelected: baseParams.value.auto_awb === BaseAutoWhiteBalanceModeValue.Manual,
    onSelected: () => (updateBaseParameter('auto_awb', BaseAutoWhiteBalanceModeValue.Manual))
  }
])


const channelErrors = computed(() => {
  const errors: Record<keyof Pick<ActuatorsParametersConfig, 'focus_channel' | 'zoom_channel' | 'tilt_channel' | 'script_channel'>, string | null> = {
    focus_channel: null,
    zoom_channel: null,
    tilt_channel: null,
    script_channel: null,
  }

  const channels = [
    'focus_channel',
    'zoom_channel',
    'tilt_channel',
    'script_channel',
  ] as const

  // Check required
  for (const key of channels) {
    if (intendedFocusAndZoomParams.value[key] == null) {
      errors[key] = 'Required'
    }
  }

  // Check duplicates (only if all are selected)
  const selected = channels.map(k => intendedFocusAndZoomParams.value[k]).filter(c => c !== null)
  if (new Set(selected).size !== selected.length) {
    // Mark duplicates
    const seen = new Set<ServoChannel>()
    for (const key of channels) {
      const val = intendedFocusAndZoomParams.value[key]
      if (val === null) continue
      if (seen.has(val)) {
        errors[key] = 'Duplicate channel'
      } else {
        seen.add(val)
      }
    }
  }

  return errors
})

const hasChannelErrors = computed(() => 
  Object.values(channelErrors.value).some(err => err !== null)
)

const bitrateOptions = computed(() => {
  const res = selectedVideoResolution.value
  if (!res) return null

  const key = `${res.width}x${res.height}`
  const allowed = resolutionsToBitrate[key]
  if (!allowed) return null

  return allowed.map((bitrate) => ({
    name: `${bitrate} kbps`,
    value: bitrate,
  }))
})

const cameraIdOptions = [
  { name: 'CAM1', value: 'CAM1' },
  { name: 'CAM2', value: 'CAM2' },
] satisfies { name: string; value: CameraID }[];

const scriptFunctionOptions = Array.from({ length: 16 }, (_, i) => ({
  name: `SCRIPT${i + 1}`,
  value: `SCRIPT${i + 1}` as ScriptFunction,
}));

const mountTypeOptions = [
  { name: 'Servo', value: 'Servo' },
  { name: 'Brushless PWM', value: 'BrushlessPWM' },
] satisfies { name: string; value: MountType }[];


const scaleFocus = (raw: number): number => raw / 10
const unscaleFocus = (scaled: number): number => scaled * 10

const formatFocusValue = (scaled: number): string => {
  return `${scaled.toFixed(2)}`
}

const scaleZoom = (raw: number): number => 1.0 + (raw / 100) * 1.0
const unscaleZoom = (scaled: number): number => ((scaled - 1.0) / 1.0) * 100
const formatZoomValue = (zoomLevel: number): string => {
  return `${zoomLevel.toFixed(1)}x`
}

const scaleTilt = (raw: number): number => {
  const minAngle = currentFocusAndZoomParams.value.tilt_mnt_pitch_min ?? -90
  const maxAngle = currentFocusAndZoomParams.value.tilt_mnt_pitch_max ?? 90
  return minAngle + (raw / 100) * (maxAngle - minAngle)
}

const unscaleTilt = (scaled: number): number => {
  const minAngle = currentFocusAndZoomParams.value.tilt_mnt_pitch_min ?? -90
  const maxAngle = currentFocusAndZoomParams.value.tilt_mnt_pitch_max ?? 90
  if (maxAngle === minAngle) return 0
  return 100 * (scaled - minAngle) / (maxAngle - minAngle)
}

const formatTiltValue = (angle: number): string => {
  return `${angle.toFixed(1)}°`
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const updateBaseParameter = (param: keyof BaseParameterSetting, value: any) => {
  if (!props.selectedCameraUuid || props.disabled) {
    return
  }

  const cameraUuid = props.selectedCameraUuid
  const generation = actuatorsRequestGeneration.value
  const previous = baseParams.value[param]
  const { token, epoch } = pendingBase.begin(param, previous, value)
  baseParams.value = { ...baseParams.value, [param]: value }

  const payload = {
    camera_uuid: cameraUuid,
    action: 'setImageAdjustment',
    json: {
      [param]: value,
    },
  }

  backendClient
    .request('POST', '/camera/control', payload)
    .then((data) => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== actuatorsRequestGeneration.value
      ) {
        return
      }
      const incoming = data as BaseParameterSetting
      pendingBase.settleSuccess(param, token, epoch, () => {
        baseParams.value = pendingBase.mergeRemote(incoming)
      })
    })
    .catch((error) => {
      const message = `Error sending ${String(param)} control with value '${value}'`
      console.log(message, error.message)
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== actuatorsRequestGeneration.value
      ) {
        return
      }
      pendingBase.settleFail(
        param,
        token,
        epoch,
        (attempted) => baseParams.value[param] === attempted,
        (prev) => {
          baseParams.value = {
            ...baseParams.value,
            [param]: prev as BaseParameterSetting[typeof param],
          }
        },
      )
    })
}

const applyConfigParameters = (newParams: ActuatorsParametersConfig): void => {
  const latch = correlationLatch.value
  currentFocusAndZoomParams.value = latch
    ? {
        ...newParams,
        enable_focus_and_zoom_correlation: latch.value,
      }
    : { ...newParams }
}

const applyActuatorsConfig = (data: ActuatorsConfig) => {
  const newParams = data?.parameters
  if (newParams) {
    if (intendedFocusAndZoomParams.value.camera_id === null) {
      intendedFocusAndZoomParams.value = { ...newParams }
    }
    applyConfigParameters(newParams)
  } else {
    console.warn("Received null 'parameters' from response:", data)
  }
}

const applyActuatorsDefaultConfig = (data: ActuatorsConfig) => {
  const newParams = data?.parameters
  if (newParams) {
    defaultFocusAndZoomParams.value = { ...newParams }
    if (intendedFocusAndZoomParams.value.camera_id === null) {
      intendedFocusAndZoomParams.value = { ...newParams }
    }
  }
}

const applyActuatorsState = (state: ActuatorsState) => {
  ;(['focus', 'zoom', 'tilt'] as const).forEach((key) => {
    const desired = desiredActuatorsState.value[key]
    const received = state[key]

    if (desired !== null) {
      if (received !== null && received !== undefined && approxEqual(received, desired, key)) {
        clearDesiredLatch(key)
        actuatorsState.value[key] = received
      }
      return
    }

    if (received !== null && received !== undefined) {
      actuatorsState.value[key] = received
    }
  })
}

const applyCameraStateEvent = (body: unknown) => {
  if (!props.selectedCameraUuid) return
  if (typeof body !== 'object' || body === null) return

  const data = body as CameraStateEvent
  if (data.camera_uuid !== props.selectedCameraUuid) return

  if (data.actuators_config) {
    applyActuatorsConfig(data.actuators_config as ActuatorsConfig)
  }
  if (data.actuators_default_config) {
    applyActuatorsDefaultConfig(data.actuators_default_config as ActuatorsConfig)
  }
  if (typeof data.actuators_configured === 'boolean') {
    isConfigured.value = data.actuators_configured
  }
  if (data.actuators_state) {
    applyActuatorsState(data.actuators_state as ActuatorsState)
  }
  if (data.video_parameters && !hasUserEditedVideo.value) {
    update_video_parameter_values(data.video_parameters as VideoParameterSettings)
  }
  if (data.base_parameters) {
    baseParams.value = pendingBase.mergeRemote(
      data.base_parameters as BaseParameterSetting,
    )
  }
}

useCameraState(toRef(props, 'selectedCameraUuid'), applyCameraStateEvent)

const isHardwareSetupComplete = computed<boolean>(() => {
  return (
    intendedFocusAndZoomParams.value.focus_channel !== null &&
    intendedFocusAndZoomParams.value.zoom_channel !== null &&
    intendedFocusAndZoomParams.value.tilt_channel !== null &&
    intendedFocusAndZoomParams.value.script_channel !== null
  )
})

const availableServoChannelOptions = computed(() => {
  const selectedChannels = new Set([
    intendedFocusAndZoomParams.value.focus_channel,
    intendedFocusAndZoomParams.value.zoom_channel,
    intendedFocusAndZoomParams.value.tilt_channel,
    intendedFocusAndZoomParams.value.script_channel
  ].filter(channel => channel !== null))

  return servoChannelOptions.map(option => ({
    ...option,
    disabled: selectedChannels.has(option.value) &&
      option.value !== intendedFocusAndZoomParams.value.focus_channel &&
      option.value !== intendedFocusAndZoomParams.value.zoom_channel &&
      option.value !== intendedFocusAndZoomParams.value.tilt_channel &&
      option.value !== intendedFocusAndZoomParams.value.script_channel
  }))
})

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const updateActuatorsConfig = (param: keyof ActuatorsParametersConfig, value: any) => {
  if (!props.selectedCameraUuid || props.disabled) {
    return
  }

  const cameraUuid = props.selectedCameraUuid
  const generation = actuatorsRequestGeneration.value
  const previousCorrelation =
    param === 'enable_focus_and_zoom_correlation'
      ? currentFocusAndZoomParams.value.enable_focus_and_zoom_correlation
      : null
  let correlationToken: number | null = null

  if (param === 'enable_focus_and_zoom_correlation') {
    if (correlationLatch.value !== null) return
    correlationToken = nextCorrelationToken++
    correlationLatch.value = { token: correlationToken, value }
    currentFocusAndZoomParams.value = {
      ...currentFocusAndZoomParams.value,
      enable_focus_and_zoom_correlation: value,
    }
  }

  const payload: ActuatorsControl = {
    camera_uuid: cameraUuid,
    action: "setActuatorsConfig",
    json: { "parameters": { [param]: value } as ActuatorsParametersConfig} as ActuatorsConfig
  }

  backendClient
    .request('POST', '/autopilot/control', payload)
    .then((data) => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== actuatorsRequestGeneration.value
      ) {
        return
      }
      const newParams = (data as ActuatorsConfig)?.parameters
      if (newParams) {
        applyConfigParameters(newParams)
      } else {
        console.warn("Received null 'parameters' from response:", data)
      }
    })
    .catch((error) => {
      if (
        param === 'enable_focus_and_zoom_correlation' &&
        props.selectedCameraUuid === cameraUuid &&
        generation === actuatorsRequestGeneration.value &&
        correlationToken !== null &&
        correlationLatch.value?.token === correlationToken
      ) {
        correlationLatch.value = null
        currentFocusAndZoomParams.value = {
          ...currentFocusAndZoomParams.value,
          enable_focus_and_zoom_correlation: previousCorrelation,
        }
      }
      const message = `Error sending ${String(param)} control with value '${value}'`
      console.log(message, error.message)
    })
    .finally(() => {
      if (
        correlationToken !== null &&
        generation === actuatorsRequestGeneration.value &&
        correlationLatch.value?.token === correlationToken
      ) {
        correlationLatch.value = null
      }
    })
}

 
const sendQueuedActuatorState = (param: ActuatorKey, allowWhileDisabled = false): void => {
  if (!props.selectedCameraUuid) return
  if (props.disabled && !allowWhileDisabled) return
  if (actuatorsSetInFlight.value[param] !== null) return

  const value = actuatorsSetQueued.value[param]
  if (value === null) return

  const cameraUuid = props.selectedCameraUuid
  const generation = actuatorsRequestGeneration.value
  const token = actuatorTokens.next()
  actuatorsSetQueued.value[param] = null
  actuatorsSetInFlight.value[param] = { token, value }

  const payload: ActuatorsControl = {
    camera_uuid: cameraUuid,
    action: "setActuatorsState",
    json: { [param]: value } as ActuatorsState,
  }

  backendClient
    .request('POST', '/autopilot/control', payload)
    .catch((error) => {
      const message = `Error updating ${param}`
      console.log(message, error.message)
      if (generation !== actuatorsRequestGeneration.value) return
      if (!ownsActuatorFlight(actuatorsSetInFlight.value[param], token)) return

      const desiredBeforeClear = desiredActuatorsState.value[param]
      // Failed command will never match SERVO — drop the latch for this value.
      if (
        desiredBeforeClear !== null &&
        approxEqual(desiredBeforeClear, value, param)
      ) {
        clearDesiredLatch(param)
      }
      // Roll back optimistic UI if this flight still owns the slot and nothing newer is pending.
      if (
        shouldRollbackActuatorUi({
          ownsFlight: true,
          queued: actuatorsSetQueued.value[param],
          desiredBeforeClear,
          ui: actuatorsState.value[param],
          attempted: value,
          rollback: actuatorsSetRollback.value[param],
          valuesMatch: (a, b) => approxEqual(a, b, param),
        })
      ) {
        actuatorsState.value[param] = actuatorsSetRollback.value[param]
      }
    })
    .finally(() => {
      // Camera switched while this request was in flight — leave the new camera alone.
      if (generation !== actuatorsRequestGeneration.value) {
        return
      }
      if (!ownsActuatorFlight(actuatorsSetInFlight.value[param], token)) {
        return
      }

      actuatorsSetInFlight.value[param] = null

      // If a newer value was queued while this request was in-flight, send it now
      // even if the UI is disabled (loading/reboot) so the drain cannot strand.
      if (actuatorsSetQueued.value[param] !== null) {
        sendQueuedActuatorState(param, true)
      } else {
        actuatorsSetRollback.value[param] = null
      }
    })
}

const updateActuatorsState = (param: keyof ActuatorsState, value: number) => {
  if (!props.selectedCameraUuid || props.disabled) return

  const key = param as ActuatorKey

  // Capture pre-gesture value once so a failed POST can roll back.
  if (
    actuatorsSetQueued.value[key] === null &&
    actuatorsSetInFlight.value[key] === null
  ) {
    actuatorsSetRollback.value[key] = actuatorsState.value[key]
  }

  // Optimistic UI update: do not wait for feedback to reflect user input.
  actuatorsState.value[key] = value
  armDesiredLatch(key, value)

  // Coalesce requests per actuator to prevent backlog.
  const existingQueued = actuatorsSetQueued.value[key]
  if (existingQueued === null || !approxEqual(existingQueued, value, key)) {
    actuatorsSetQueued.value[key] = value
  }
  sendQueuedActuatorState(key)
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const handleChannelChanges = (param: keyof ActuatorsParametersConfig, value: any): void => {
  if (!props.selectedCameraUuid || props.disabled) return

  // Optional: prevent duplicates (though UI should disable them)
  const isAlreadySelected = Object.entries(intendedFocusAndZoomParams.value).some(
    ([key, val]) => key !== param && val === value
  )

  if (isAlreadySelected && value !== null) {
    console.warn(`Channel ${value} is already in use`)
    return
  }

  intendedFocusAndZoomParams.value[param] = value
}

const getBaseParameters = () => {
  if (!props.selectedCameraUuid || props.disabled) {
    return
  }

  const cameraUuid = props.selectedCameraUuid
  const generation = actuatorsRequestGeneration.value
  const payload = {
    camera_uuid: cameraUuid,
    action: "getImageAdjustment",
  }

  backendClient.request('POST', '/camera/control', payload)
    .then(data => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== actuatorsRequestGeneration.value
      ) {
        return
      }
      baseParams.value = pendingBase.mergeRemote(
        data as BaseParameterSetting,
      )
      console.log(data)
    })
    .catch(error => {
      console.error(`Error sending getImageAdjustment request:`, error.message)
    })
}

const updateVideoParameters = async (
  partial: Partial<VideoParameterSettings>
): Promise<void> => {
  if (!props.selectedCameraUuid || props.disabled) return

  const cameraUuid = props.selectedCameraUuid
  const generation = actuatorsRequestGeneration.value
  const payload = {
    camera_uuid: cameraUuid,
    action: 'setVencConf',
    json: partial as VideoParameterSettings,
  }

  try {
    const data = await backendClient.request('POST', '/camera/control', payload)
    if (
      props.selectedCameraUuid !== cameraUuid ||
      generation !== actuatorsRequestGeneration.value
    ) {
      return
    }
    const settings = data as VideoParameterSettings
    update_video_parameter_values(settings)
  } catch (error) {
    const message = `Error sending partial video params '${JSON.stringify(partial)}'`
    console.log(message, error instanceof Error ? error.message : error)
    throw error
  }
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const handleVideoChanges = (what: 'resolution' | 'bitrate', value: any): void => {
  if (!props.selectedCameraUuid || props.disabled) return

  hasUserEditedVideo.value = true

  if (what === 'resolution' && value) {
    selectedVideoResolution.value = value as VideoResolutionValue
    const key = `${value.width}x${value.height}`
    const allowed = resolutionsToBitrate[key]
    if (allowed?.length) {
      selectedVideoBitrate.value = allowed[0]
    }
  } else if (what === 'bitrate') {
    selectedVideoBitrate.value = value as number
  }    

  // Compute unsaved state
  const currentResolution = selectedVideoParameters.value.pic_width && selectedVideoParameters.value.pic_height
    ? { width: selectedVideoParameters.value.pic_width, height: selectedVideoParameters.value.pic_height }
    : null
  const currentBitrate = selectedVideoParameters.value.bitrate ?? null

  const hasUnsaved = 
    (selectedVideoResolution.value?.width !== currentResolution?.width ||
     selectedVideoResolution.value?.height !== currentResolution?.height) ||
    (selectedVideoBitrate.value !== currentBitrate)

  hasUnsavedVideoChanges.value = hasUnsaved
}


const update_video_parameter_values = (settings: VideoParameterSettings) => {
  downloadedVideoParameters.value = { ...settings }
  selectedVideoParameters.value = { ...settings }
  selectedVideoParameters.value.pixel_list = undefined

  // Only update UI selectors if user hasn't made changes
  if (!hasUserEditedVideo.value) {
    const width = settings.pic_width
    const height = settings.pic_height

    if (width && height) {
      const match = resolutionOptions.value.find(o => o.value.width === width && o.value.height === height)
      if (match) {
        selectedVideoResolution.value = match.value
      } else {
        // Inject new resolution if not in list
        const injected = { width, height }
        resolutionOptions.value.push({ name: `${width}x${height}`, value: injected })
        selectedVideoResolution.value = injected
      }
    } else {
      selectedVideoResolution.value = null
    }

    selectedVideoBitrate.value = settings.bitrate ?? null
  }

  // Always update bitrate options based on current resolution
  if (selectedVideoResolution.value) {
    const key = `${selectedVideoResolution.value.width}x${selectedVideoResolution.value.height}`
    const allowed = resolutionsToBitrate[key]
    if (allowed && !allowed.includes(selectedVideoBitrate.value ?? -1)) {
      // If current bitrate is invalid for new res, reset to first option (but only if user hasn't edited)
      if (!hasUserEditedVideo.value) {
        selectedVideoBitrate.value = allowed[0]
      }
    }
  }
}

const doWhiteBalance = async () => {
  if (!props.selectedCameraUuid || props.disabled || wbBusy.value) {
    return
  }

  const cameraUuid = props.selectedCameraUuid
  const payload: CameraControl = {
    camera_uuid: cameraUuid,
    action: "setImageAdjustmentEx",
    json: {
      onceAWB: 1,
    } as AdvancedParameterSetting,
  }

  backendClient.request('POST', '/camera/control', payload)
    .catch(error => {
      console.error("Error sending onceAWB control:", error.message)
    })
}

const doRestart = (cameraUuid?: string) => {
  const uuid = cameraUuid ?? props.selectedCameraUuid
  if (!uuid) {
    return
  }
  // Explicit captured UUID must still reboot even if the current selection is disabled.
  if (cameraUuid == null && props.disabled) {
    return
  }

  rebootCamera(uuid)
    .then((data) => {
      console.log("Got an answer from the restarting request", data)
    })
    .catch((error) => {
      console.error('Failed to reboot camera', error)
    })
}

const saveVideoDataAndRestart = async (): Promise<void> => {
  if (!props.selectedCameraUuid || props.disabled) return

  const cameraUuid = props.selectedCameraUuid
  const curr = selectedVideoParameters.value
  const newWidth = selectedVideoResolution.value?.width ?? null
  const newHeight = selectedVideoResolution.value?.height ?? null
  const newBitrate = selectedVideoBitrate.value

  const videoPartial: Partial<VideoParameterSettings> = {}
  if (newWidth !== null && newWidth !== curr.pic_width) videoPartial.pic_width = newWidth
  if (newHeight !== null && newHeight !== curr.pic_height) videoPartial.pic_height = newHeight
  if (newBitrate !== null && newBitrate !== curr.bitrate) videoPartial.bitrate = newBitrate

  if (Object.keys(videoPartial).length > 0) {
    try {
      await updateVideoParameters(videoPartial)
      // Always reboot the camera that accepted the venc change, even if the
      // user switched selection afterward.
      doRestart(cameraUuid)
      if (props.selectedCameraUuid === cameraUuid) {
        Object.assign(selectedVideoParameters.value, videoPartial)
      }
    } catch {
      return
    }
  }

  hasUserEditedVideo.value = false
  hasUnsavedVideoChanges.value = false
}

const updateLuaScript = (): void => {
  if (!props.selectedCameraUuid || props.disabled) return

  backendClient
    .request('POST', '/autopilot/control', {
      camera_uuid: props.selectedCameraUuid,
      action: 'exportLuaScript',
    })
    .then((data) => {
      console.log('Lua script download initiated:', data)
    })
    .catch((error) => {
      console.error('Failed to update Lua script', error)
    })
}

const applyRecommendedCameraSettings = (): void => {
  if (!props.selectedCameraUuid || props.disabled) return

  const payload = {
    camera_uuid: props.selectedCameraUuid,
    action: 'setRecommendedCameraSettings',
  }

  backendClient
    .request('POST', '/camera/control', payload)
    .then((data) => {
      console.log('Recommended camera settings applied:', data)
    })
    .catch((error) => {
      console.error('Failed to apply recommended camera settings', error)
    })
}

const saveHardwareSetup = async (): Promise<void> => {
  if (!props.selectedCameraUuid || props.disabled || !defaultFocusAndZoomParams.value.camera_id || !intendedFocusAndZoomParams.value) return

  if (!isHardwareSetupComplete.value) {
    console.error('All channel selections are required')
    return
  }

  const cameraUuid = props.selectedCameraUuid
  let payloadParams: ActuatorsParametersConfig
  payloadParams = { ...defaultFocusAndZoomParams.value }
  if (showAdvancedHardware.value) {
    payloadParams = { ...intendedFocusAndZoomParams.value }
  }

  const payload: ActuatorsControl = {
    camera_uuid: cameraUuid,
    action: "setActuatorsConfig",
    json: { parameters: payloadParams } as ActuatorsConfig
  }

  console.log('Saving hardware setup:', payload)

  const generation = actuatorsRequestGeneration.value

  backendClient
    .request('POST', '/autopilot/control', payload)
    .then((data) => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== actuatorsRequestGeneration.value
      ) {
        return
      }
      console.log("Got an answer from the setActuatorsConfig request", data)

      const newParams = (data as ActuatorsConfig)?.parameters
      if (newParams) {
        applyConfigParameters(newParams)
        intendedFocusAndZoomParams.value = { ...newParams }
      }
      showAdvancedHardware.value = false
      applyPanelLayout(true)
    })
    .catch((error) => {
      const message = 'Error saving hardware setup'
      console.log(message, error.message)
    })
}

const resetToRecommendedDefaults = async (): Promise<void> => {
  if (!props.selectedCameraUuid || props.disabled) return

  const cameraUuid = props.selectedCameraUuid
  const generation = actuatorsRequestGeneration.value
  const payload = {
    camera_uuid: cameraUuid,
    action: 'resetActuatorsConfig',
  }

  backendClient
    .request('POST', '/autopilot/control', payload)
    .then((data) => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== actuatorsRequestGeneration.value
      ) {
        return
      }
      console.log("Got an answer from the setActuatorsConfig request", data)

      const newParams = (data as ActuatorsConfig)?.parameters
      if (newParams) {
        applyConfigParameters(newParams)
        intendedFocusAndZoomParams.value = { ...newParams }
      }
      showAdvancedHardware.value = false
      applyPanelLayout(true)
    })
    .catch((error) => {
      const message = 'Failed to apply default hardware setup'
      console.error(message, error)
    })
}

defineExpose({
  updateLuaScript,
  applyRecommendedCameraSettings,
  rebootCamera: doRestart,
  scrollToHardwareSetup,
})

watch(
  () => selectedVideoResolution.value,
  (newRes) => {
    if (!newRes) return
    // Only sync the local bitrate picker; never POST from a resolution change
    // that may have come from camera/state (that races SAVE AND RESTART).
    const key = `${newRes.width}x${newRes.height}`
    const allowed = resolutionsToBitrate[key]
    if (!allowed || allowed.length === 0) {
      selectedVideoBitrate.value = null
      return
    }
    if (!selectedVideoBitrate.value || !allowed.includes(selectedVideoBitrate.value)) {
      selectedVideoBitrate.value = allowed[0]
    }
  }
)

</script>
