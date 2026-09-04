<template>
  <!-- ImageParameters Sliders -->
  <div class="px-6 pt-5">
    <div class="flex flex-col gap-[15px] px-13">
      <BlueSlider
        name="hue"
        label="Hue"
        :model-value="baseParams.hue ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('hue', $event)"
      />
      <BlueSlider
        name="brightness"
        label="Brightness"
        :model-value="baseParams.brightness ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('brightness', $event)"
      />
      <BlueSlider
        name="sharpness"
        label="Sharpness"
        :model-value="baseParams.sharpness ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('sharpness', $event)"
      />
      <BlueSlider
        name="contrast"
        label="Contrast"
        :model-value="baseParams.contrast ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('contrast', $event)"
      />
      <BlueSlider
        name="saturation"
        label="Saturation"
        :model-value="baseParams.saturation ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('saturation', $event)"
      />
      <BlueSlider
        name="gamma"
        label="Gamma"
        :model-value="baseParams.gamma ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('gamma', $event)"
      />
      <BlueSlider
        name="blc_level"
        label="Backlight Compensation"
        :model-value="baseParams.blc_level ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('blc_level', $event)"
      />

      <!-- Restore Image Parameters -->
      <div class="flex justify-end">
        <BlueButton
          theme="dark"
          :loading="processingBaseRestore"
          :disabled="props.disabled || processingBaseRestore"
          @click="doRestoreBase"
        >
          {{ processingBaseRestore ? "Processing..." : "Restore defaults" }}
        </BlueButton>
      </div>
      <div class="flex justify-end">
        <BlueButton
          theme="dark"
          :loading="wbBusy"
          :disabled="props.disabled || wbBusy"
          @click="doWhiteBalance"
        >
          {{ onePushLabel }}
        </BlueButton>
      </div>
    </div>
  </div>

  <div class="mx-6 my-5 border-t border-[#ffffff14]" />

  <div class="px-6">
    <!-- White Balance -->
    <BlueExpansiblePanel
      title="White Balance"
      :expanded="false"
      theme="dark"
    >
      <BlueSelect
        :model-value="baseParams.auto_awb"
        :items="autoWhiteBalanceModeOptions"
        label="White Balance Mode"
        :disabled="props.disabled || wbBusy"
        theme="dark"
        @update:model-value="updateBaseParameter('auto_awb', $event)"
      />
      <BlueSelect
        v-if="baseParams.auto_awb === BaseAutoWhiteBalanceModeValue.Auto"
        :model-value="baseParams.awb_auto_mode"
        :items="autoWhiteBalanceSceneOptions"
        label="White Balance Scene"
        :disabled="props.disabled || wbBusy"
        theme="dark"
        @update:model-value="updateBaseParameter('awb_auto_mode', $event)"
      />
      <div
        v-if="baseParams.auto_awb === BaseAutoWhiteBalanceModeValue.Manual"
        class="flex flex-col gap-[15px]"
      >
        <BlueSlider
          name="awb_red"
          label="White Balance Red"
          :model-value="baseParams.awb_red ?? 0"
          :min="0"
          :max="255"
          :step="1"
          :disabled="props.disabled || wbBusy"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="updateBaseParameter('awb_red', $event)"
        />
        <BlueSlider
          name="awb_green"
          label="White Balance Green"
          :model-value="baseParams.awb_green ?? 0"
          :min="0"
          :max="255"
          :step="1"
          :disabled="props.disabled || wbBusy"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="updateBaseParameter('awb_green', $event)"
        />
        <BlueSlider
          name="awb_blue"
          label="White Balance Blue"
          :model-value="baseParams.awb_blue ?? 0"
          :min="0"
          :max="255"
          :step="1"
          :disabled="props.disabled || wbBusy"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="updateBaseParameter('awb_blue', $event)"
        />
      </div>
      <BlueSlider
        name="awb_style_red"
        label="White Balance Style Red"
        :model-value="baseParams.awb_style_red ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled || wbBusy"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('awb_style_red', $event)"
      />
      <BlueSlider
        name="awb_style_green"
        label="White Balance Style Green"
        :model-value="baseParams.awb_style_green ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled || wbBusy"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('awb_style_green', $event)"
      />
      <BlueSlider
        name="awb_style_blue"
        label="White Balance Style Blue"
        :model-value="baseParams.awb_style_blue ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled || wbBusy"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('awb_style_blue', $event)"
      />
    </BlueExpansiblePanel>
    
    <!-- Exposure & Gain -->
    <BlueExpansiblePanel
      title="Exposure & Gain"
      :expanded="false"
      theme="dark"
    >
      <BlueSwitch
        :model-value="advancedParams.low_farme_rate === AdvancedDisplayLowFramerateValue.Open"
        label="Slow Shutter"
        :disabled="props.disabled"
        name="slow-shutter"
        theme="dark"
        @update:model-value="updateAdvancedParam('low_farme_rate', $event ? AdvancedDisplayLowFramerateValue.Open : AdvancedDisplayLowFramerateValue.Close)"
      />
      <BlueSelect
        :model-value="baseParams.auto_gain_mode"
        :items="autoGainModeOptions"
        label="Automatic Gain Mode"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateBaseParameter('auto_gain_mode', $event)"
      />
      <div
        v-if="baseParams.auto_gain_mode === BaseAutoGainModeValue.Auto"
        class="flex flex-col gap-[15px]"
      >
        <BlueSlider
          name="auto_d_gain_max"
          label="Automatic Maximum D Gain"
          :model-value="baseParams.auto_DGain_max ?? 0"
          :min="0"
          :max="255"
          :step="1"
          :disabled="props.disabled"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="updateBaseParameter('auto_DGain_max', $event)"
        />
        <BlueSlider
          name="auto_a_gain_max"
          label="Automatic Maximum A Gain"
          :model-value="baseParams.auto_AGain_max ?? 0"
          :min="0"
          :max="255"
          :step="1"
          :disabled="props.disabled"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="updateBaseParameter('auto_AGain_max', $event)"
        />
      </div>
      <div
        v-if="baseParams.auto_gain_mode === BaseAutoGainModeValue.Manual"
        class="flex flex-col gap-[15px]"
      >
        <BlueSwitch
          :model-value="baseParams.manual_AGain_enable === BaseManualAGainEnableValue.Open"
          label="Enable Manual A Gain"
          :disabled="props.disabled"
          name="enable-manual-a-gain"
          theme="dark"
          @update:model-value="updateBaseParameter('manual_AGain_enable', $event ? BaseManualAGainEnableValue.Open : BaseManualAGainEnableValue.Close)"
        />
        <BlueSlider
          v-if="baseParams.manual_AGain_enable === BaseManualAGainEnableValue.Open"
          name="manual_a_gain"
          label="Manual A Gain"
          :model-value="baseParams.manual_AGain ?? 0"
          :min="0"
          :max="255"
          :step="1"
          :disabled="props.disabled"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="updateBaseParameter('manual_AGain', $event)"
        />
        <BlueSwitch
          :model-value="baseParams.manual_DGain_enable === BaseManualDGainEnableValue.Open"
          label="Enable Manual D Gain"
          :disabled="props.disabled"
          name="enable-manual-d-gain"
          theme="dark"
          @update:model-value="updateBaseParameter('manual_DGain_enable', $event ? BaseManualDGainEnableValue.Open : BaseManualDGainEnableValue.Close)"
        />
        <BlueSlider
          v-if="baseParams.manual_DGain_enable === BaseManualDGainEnableValue.Open"
          name="manual_d_gain"
          label="Manual D Gain"
          :model-value="baseParams.manual_DGain ?? 0"
          :min="0"
          :max="255"
          :step="1"
          :disabled="props.disabled"
          theme="dark"
          value-weight="regular"
          label-width="130px"
          @update:model-value="updateBaseParameter('manual_DGain', $event)"
        />
      </div>
      <BlueSlider
        name="max_sys_gain"
        label="Max System Gain"
        :model-value="baseParams.max_sys_gain ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateBaseParameter('max_sys_gain', $event)"
      />

      <BlueSelect
        :model-value="baseParams.AE_strategy_mode"
        :items="autoExposureStrategyModeOptions"
        label="Exposure Strategy Mode"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateBaseParameter('AE_strategy_mode', $event)"
      />
      <BlueSelect
        :model-value="baseParams.auto_exposureEx"
        :items="exposureModeOptions"
        label="Exposure Mode"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateBaseParameter('auto_exposureEx', $event)"
      />
      <BlueSelect
        v-if="baseParams.auto_exposureEx === BaseExposureModeValue.Auto"
        :model-value="baseParams.max_exposure"
        :items="maxExposureOptions"
        label="Maximum Exposure Time"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateBaseParameter('max_exposure', $event)"
      />
      <BlueSelect
        v-if="baseParams.auto_exposureEx === BaseExposureModeValue.Manual"
        :model-value="baseParams.exposure_time"
        :items="exposureTimeOptions"
        label="Manual Exposure Time"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateBaseParameter('exposure_time', $event)"
      />
    </BlueExpansiblePanel>

    <!-- Infrared -->
    <BlueExpansiblePanel
      title="Infrared"
      :expanded="false"
      theme="dark"
    >
      <BlueSelect
        :model-value="advancedParams.ircut_level"
        :items="ircutLevelOptions"
        label="IRCUT Level"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('ircut_level', $event)"
      />
      <BlueSelect
        :model-value="advancedParams.ldr_level"
        :items="ldrLevelOptions"
        label="Photosensitive Level"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('ldr_level', $event)"
      />
      <BlueSelect
        :model-value="advancedParams.lamp_type"
        :items="lampTypeOptions"
        label="Lamp Type"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('lamp_type', $event)"
      />
      <BlueSelect
        :model-value="advancedParams.led_control_avail"
        :items="ledControlAvailOptions"
        label="Light Enable Level"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('led_control_avail', $event)"
      />
      <BlueSelect
        :model-value="advancedParams.led_control"
        :items="ledControlOptions"
        label="IR Control"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('led_control', $event)"
      />
      <BlueSlider
        name="sens_day_to_night"
        label="Day to Night Sensitivity"
        :model-value="advancedParams.sens_day_to_night ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('sens_day_to_night', $event)"
      />
      <BlueSlider
        name="sens_night_to_day"
        label="Night to Day Sensitivity"
        :model-value="advancedParams.sens_night_to_day ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('sens_night_to_day', $event)"
      />
      <BlueSlider
        name="infr_day_h"
        label="Infrared Day Start Hour"
        :model-value="advancedParams.infr_day_h ?? 0"
        :min="0"
        :max="23"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('infr_day_h', $event)"
      />
      <BlueSlider
        name="infr_day_m"
        label="Infrared Day Start Minute"
        :model-value="advancedParams.infr_day_m ?? 0"
        :min="0"
        :max="59"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('infr_day_m', $event)"
      />
      <BlueSlider
        name="infr_night_h"
        label="Infrared Night Start Hour"
        :model-value="advancedParams.infr_night_h ?? 0"
        :min="0"
        :max="23"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('infr_night_h', $event)"
      />
      <BlueSlider
        name="infr_night_m"
        label="Infrared Night Start Minute"
        :model-value="advancedParams.infr_night_m ?? 0"
        :min="0"
        :max="59"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('infr_night_m', $event)"
      />
      <BlueSlider
        name="ir_level"
        label="Infrared Lamp Brightness"
        :model-value="advancedParams.ir_level ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('ir_level', $event)"
      />
      <BlueSlider
        name="led_level"
        label="White Light Brightness"
        :model-value="advancedParams.led_level ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('led_level', $event)"
      />
      <BlueSlider
        name="iris_level"
        label="Aperture PWM Duty Cycle"
        :model-value="advancedParams.irisLevel ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('irisLevel', $event)"
      />
    </BlueExpansiblePanel>
    


    <!-- Advanced Parameters -->
    <BlueExpansiblePanel
      title="Advanced"
      :expanded="false"
      theme="dark"
    >
      <BlueSelect
        :model-value="advancedParams.mirror"
        :items="mirrorOptions"
        label="Mirror"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('mirror', $event)"
      />
      <BlueSelect
        :model-value="advancedParams.flip"
        :items="flipOptions"
        label="Flip"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('flip', $event)"
      />
      <BlueSelect
        :model-value="advancedParams.power_freq"
        :items="powerFreqOptions"
        label="Power Frequency"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('power_freq', $event)"
      />
      <BlueSwitch
        :model-value="advancedParams.color_black === AdvancedDisplayColorBlackValue.Auto"
        label="Auto Color Black"
        :disabled="props.disabled"
        name="auto-color-black"
        theme="dark"
        @update:model-value="updateAdvancedParam('color_black', $event ? AdvancedDisplayColorBlackValue.Auto : AdvancedDisplayColorBlackValue.Color)"
      />
      <BlueSelect
        :model-value="advancedParams.infr_detect_mode"
        :items="infrDetectModeOptions"
        label="Video Detection Mode"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('infr_detect_mode', $event)"
      />
      <BlueSelect
        :model-value="advancedParams.lens_correction"
        :items="lensCorrectionOptions"
        label="Lens Correction"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('lens_correction', $event)"
      />
        
      <BlueSelect
        :model-value="advancedParams.auto_iris"
        :items="autoIrisOptions"
        label="Aperture Mode"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('auto_iris', $event)"
      />
      <BlueSelect
        :model-value="advancedParams.noiseReduction"
        :items="noiseReductionOptions"
        label="3D Noise Reduction"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('noiseReduction', $event)"
      />
      <BlueSlider
        name="wdr_level_sensor"
        label="WDR Strength"
        :model-value="advancedParams.wdr_level_sensor ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('wdr_level_sensor', $event)"
      />
      <BlueSlider
        name="wdr_level"
        label="Wide Dynamic Strength"
        :model-value="advancedParams.wdr_level ?? 0"
        :min="0"
        :max="255"
        :step="1"
        :disabled="props.disabled"
        theme="dark"
        value-weight="regular"
        label-width="130px"
        @update:model-value="updateAdvancedParam('wdr_level', $event)"
      />
      <BlueSwitch
        :model-value="advancedParams.wdr_sensor === AdvancedDisplayWDRSensorValue.Open"
        label="WDR Enable"
        :disabled="props.disabled"
        name="wdr-enable"
        theme="dark"
        @update:model-value="updateAdvancedParam('wdr_sensor', $event ? AdvancedDisplayWDRSensorValue.Open : AdvancedDisplayWDRSensorValue.Close)"
      />
      <BlueSwitch
        :model-value="advancedParams.hlc_enable === AdvancedDisplayHlcEnableValue.Open"
        label="HLC Enable"
        :disabled="props.disabled"
        name="hlc-enable"
        theme="dark"
        @update:model-value="updateAdvancedParam('hlc_enable', $event ? AdvancedDisplayHlcEnableValue.Open : AdvancedDisplayHlcEnableValue.Close)"
      />

      <BlueSelect
        :model-value="advancedParams._2DNR_level"
        :items="_2dNrLevelOptions"
        label="2D Noise Reduction"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('_2DNR_level', $event)"
      />
      <BlueSelect
        :model-value="advancedParams.anti_flicker"
        :items="antiFlickerOptions"
        label="Anti Flicker"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('anti_flicker', $event)"
      />
      <!-- Scene mode is read off the advanced parameters: the base parameter's own does not take. -->
      <BlueSelect
        :model-value="advancedParams.scene_mode"
        :items="sceneModeOptions"
        label="Scene Mode"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateAdvancedParam('scene_mode', $event)"
      />

        
      <BlueSelect
        v-tooltip="'note: This restarts the camera'"
        :model-value="baseParams.rotate"
        :items="rotateOptions"
        label="Image Rotation"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateBaseParameter('rotate', $event)"
      />

      <BlueSelect
        :model-value="baseParams.frameTurbo_pro"
        :items="frameTurboOptions"
        label="Frame Turbo"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateBaseParameter('frameTurbo_pro', $event)"
      />

      <BlueSelect
        :model-value="baseParams.antiFog"
        :items="antiFogOptions"
        label="Dehaze"
        :disabled="props.disabled"
        theme="dark"
        @update:model-value="updateBaseParameter('antiFog', $event)"
      />

      <!-- Restore Advanced Image Parameters -->
      <div class="flex justify-end">
        <BlueButton
          theme="dark"
          :loading="processingAdvancedRestore"
          :disabled="props.disabled || processingAdvancedRestore"
          @click="doRestoreAdvanced"
        >
          {{ processingAdvancedRestore ? "Processing..." : "Restore defaults" }}
        </BlueButton>
      </div>
    </BlueExpansiblePanel>
  </div>
</template>

<script setup lang="ts">

import {
    type BaseParameterSetting,
    type CameraControl,
    type AdvancedParameterSetting,
    BaseManualAGainEnableValue,
    BaseManualDGainEnableValue,
    AdvancedDisplayColorBlackValue,
    BaseAntiFogValue,
    BaseFrameTurboValue,
    BaseAutoExposureStrategyModeValue,
    BaseExposureModeValue,
    BaseExposureTimeValue,
    BaseAutoGainModeValue,
    BaseAutoWhiteBalanceModeValue,
    BaseAutoWhiteBalanceSceneValue,
    BaseMaxExposureValue,
    BaseRotateValue,
    AdvancedDisplayFlipValue,
    AdvancedDisplayInfrDetectModeValue,
    AdvancedDisplayLensCorrectionValue,
    AdvancedDisplayMirrorValue,
    AdvancedDisplayPowerFreqValue,
    AdvancedDisplayIRCUTLevelValue,
    AdvancedDisplayAntiflickerValue,
    AdvancedDisplay2dNrLevelValue,
    AdvancedDisplayAutoIrisValue,
    AdvancedDisplayLampTypeValue,
    AdvancedDisplayLDRLevelValue,
    AdvancedDisplayLedControlAvailValue,
    AdvancedDisplayNoiseReductionValue,
    AdvancedDisplayWDRSensorValue,
    AdvancedDisplayHlcEnableValue,
    AdvancedDisplayLowFramerateValue,
    AdvancedDisplayLedControlValue,  
    AdvancedDisplaySceneModeValue
} from '@/bindings/br4kcam'


import {
  BlueButton,
  BlueExpansiblePanel,
  BlueSelect,
  BlueSlider,
  BlueSwitch,
} from '@bluerobotics/bluevue'
import { enumToOptions } from '@/utils/enumUtils'
import { backendClient } from '@/utils/backendClient'
import { useCameraState } from '@/utils/useCameraState'
import { createPendingFields } from '@/utils/pendingFields'
import type { OnePushAwbStatus } from '@/bindings/br4kcam_api'
import { computed, ref, toRef, watch } from 'vue'

const props = defineProps<{
  selectedCameraUuid: string | null
  disabled: boolean
  onePushAwb?: OnePushAwbStatus | null
}>()

const wbBusy = computed(() => props.onePushAwb != null)
const onePushLabel = computed(() => {
  if (props.onePushAwb != null) return 'Processing...'
  return 'One-Push White Balance'
})

const processingBaseRestore = ref(false)
const processingAdvancedRestore = ref(false)

// Advanced parameters
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
  rotate: null
})
const advancedParams = ref<AdvancedParameterSetting>({
  mirror: null,
  flip: null,
  power_freq: null,
  color_black: null,
  infr_detect_mode: null,
  sens_day_to_night: null,
  sens_night_to_day: null,
  infr_day_h: null,
  infr_day_m: null,
  infr_night_h: null,
  infr_night_m: null,
  lens_correction: null,
  wdr_level: null,
  ircut_level: null,
  ldr_level: null,
  led_control_mode: null,
  lamp_type: null,
  led_control_avail: null,
  ir_level: null,
  led_level: null,
  led_control: null,
  auto_iris: null,
  irisLevel: null,
  noiseReduction: null,
  wdr_sensor: null,
  wdr_level_sensor: null,
  hlc_enable: null,
  low_farme_rate: null,
  _2DNR_level: null,
  anti_flicker: null,
  scene_mode: null,
  onceAWB: null,
  set_default: null
})

// Generate options using enum utilities
const antiFogOptions = enumToOptions(BaseAntiFogValue)
const frameTurboOptions = enumToOptions(BaseFrameTurboValue)
const sceneModeOptions = enumToOptions(AdvancedDisplaySceneModeValue)
const autoExposureStrategyModeOptions = enumToOptions(BaseAutoExposureStrategyModeValue)
const exposureModeOptions = enumToOptions(BaseExposureModeValue)
const exposureTimeOptions = enumToOptions(BaseExposureTimeValue).map(option => ({
  ...option,
  name: `1/${option.value} s`
}));
const maxExposureOptions = enumToOptions(BaseMaxExposureValue).map(option => ({
  ...option,
  name: `1/${option.value} s`
}));
const autoWhiteBalanceModeOptions = enumToOptions(BaseAutoWhiteBalanceModeValue)
const autoWhiteBalanceSceneOptions = enumToOptions(BaseAutoWhiteBalanceSceneValue)
const autoGainModeOptions = enumToOptions(BaseAutoGainModeValue)
const rotateOptions = enumToOptions(BaseRotateValue).map(option => ({
  ...option,
  name: `${option.value} º`
}));
const mirrorOptions = enumToOptions(AdvancedDisplayMirrorValue)
const flipOptions = enumToOptions(AdvancedDisplayFlipValue)
const powerFreqOptions = enumToOptions(AdvancedDisplayPowerFreqValue)
const infrDetectModeOptions = enumToOptions(AdvancedDisplayInfrDetectModeValue)
const lensCorrectionOptions = enumToOptions(AdvancedDisplayLensCorrectionValue)
const ircutLevelOptions = enumToOptions(AdvancedDisplayIRCUTLevelValue)
const ldrLevelOptions = enumToOptions(AdvancedDisplayLDRLevelValue)
const lampTypeOptions = enumToOptions(AdvancedDisplayLampTypeValue)
const ledControlAvailOptions = enumToOptions(AdvancedDisplayLedControlAvailValue)
const ledControlOptions = enumToOptions(AdvancedDisplayLedControlValue)
const autoIrisOptions = enumToOptions(AdvancedDisplayAutoIrisValue)
const noiseReductionOptions = enumToOptions(AdvancedDisplayNoiseReductionValue)
const _2dNrLevelOptions = enumToOptions(AdvancedDisplay2dNrLevelValue)
const antiFlickerOptions = enumToOptions(AdvancedDisplayAntiflickerValue)

const imageRequestGeneration = ref(0)
const pendingBase = createPendingFields<keyof BaseParameterSetting, unknown>()
const pendingAdvanced = createPendingFields<keyof AdvancedParameterSetting, unknown>()

const applyCameraStateEvent = (body: unknown) => {
  if (!props.selectedCameraUuid) return
  if (typeof body !== 'object' || body === null) return

  const data = body as Record<string, unknown>
  if (data.camera_uuid !== props.selectedCameraUuid) return

  if (data.base_parameters) {
    baseParams.value = pendingBase.mergeRemote(
      data.base_parameters as BaseParameterSetting,
    )
  }
  if (data.advanced_parameters) {
    advancedParams.value = pendingAdvanced.mergeRemote(
      data.advanced_parameters as AdvancedParameterSetting,
    )
  }
}

useCameraState(toRef(props, 'selectedCameraUuid'), applyCameraStateEvent)

watch(
  () => props.selectedCameraUuid,
  () => {
    imageRequestGeneration.value += 1
    pendingBase.clear()
    pendingAdvanced.clear()
    processingBaseRestore.value = false
    processingAdvancedRestore.value = false
  },
)

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const updateBaseParameter = (param: keyof BaseParameterSetting, value: any) => {
  if (!props.selectedCameraUuid) {
    return
  }

  const cameraUuid = props.selectedCameraUuid
  const generation = imageRequestGeneration.value
  const previous = baseParams.value[param]
  const { token, epoch } = pendingBase.begin(param, previous, value)
  baseParams.value = { ...baseParams.value, [param]: value }

  const payload = {
    camera_uuid: cameraUuid,
    action: "setImageAdjustment",
    json: {
      [param]: value,
    },
  }

  console.log(payload)

  backendClient.request('POST', '/camera/control', payload)
    .then(data => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== imageRequestGeneration.value
      ) {
        return
      }
      const incoming = data as BaseParameterSetting
      pendingBase.settleSuccess(param, token, epoch, () => {
        baseParams.value = pendingBase.mergeRemote(incoming)
      })
    })
    .catch(error => {
      console.error(`Error sending ${String(param)} control with value '${value}':`, error.message)
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== imageRequestGeneration.value
      ) {
        return
      }
      pendingBase.settleFail(
        param,
        token,
        epoch,
        (attempted) => baseParams.value[param] === attempted,
        (prev) => {
          baseParams.value = { ...baseParams.value, [param]: prev as BaseParameterSetting[typeof param] }
        },
      )
    })
}

const getBaseParameters = () => {
  if (!props.selectedCameraUuid) {
    return
  }

  const cameraUuid = props.selectedCameraUuid
  const generation = imageRequestGeneration.value
  const payload = {
    camera_uuid: cameraUuid,
    action: "getImageAdjustment",
  }

  backendClient.request('POST', '/camera/control', payload)
    .then(data => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== imageRequestGeneration.value
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

const doWhiteBalance = async () => {
  if (!props.selectedCameraUuid || props.disabled || wbBusy.value) {
    return
  }

  const payload: CameraControl = {
    camera_uuid: props.selectedCameraUuid,
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

const doRestoreBase = async () => {
  if (!props.selectedCameraUuid) {
    return
  }

  processingBaseRestore.value = true

  const cameraUuid = props.selectedCameraUuid
  const generation = imageRequestGeneration.value
  pendingBase.beginRestore()
  const payload: CameraControl = {
    camera_uuid: cameraUuid,
    action: "setImageAdjustment",
    json: {
      set_default: 1,
    } as BaseParameterSetting,
  }

  backendClient
    .request('POST', '/camera/control', payload)
    .then(data => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== imageRequestGeneration.value
      ) {
        return
      }
      baseParams.value = pendingBase.mergeRemote(data as BaseParameterSetting)
    })
    .catch(error => {
      console.error("Error sending base image restore control:", error.message)
    })
    .finally(() => {
      if (generation !== imageRequestGeneration.value) return
      processingBaseRestore.value = false
    })
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const updateAdvancedParam = (param: keyof AdvancedParameterSetting, value: any) => {
  if (!props.selectedCameraUuid) return

  const cameraUuid = props.selectedCameraUuid
  const generation = imageRequestGeneration.value
  const previous = advancedParams.value[param]
  const { token, epoch } = pendingAdvanced.begin(param, previous, value)
  advancedParams.value = { ...advancedParams.value, [param]: value }

  const payload: CameraControl = {
    camera_uuid: cameraUuid,
    action: "setImageAdjustmentEx",
    json: { [param]: value } as AdvancedParameterSetting
  }

  backendClient.request('POST', '/camera/control', payload)
    .then(data => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== imageRequestGeneration.value
      ) {
        return
      }
      const incoming = data as AdvancedParameterSetting
      pendingAdvanced.settleSuccess(param, token, epoch, () => {
        advancedParams.value = pendingAdvanced.mergeRemote(incoming)
      })
    })
    .catch(error => {
      console.error(`Error updating ${param}:`, error.message)
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== imageRequestGeneration.value
      ) {
        return
      }
      pendingAdvanced.settleFail(
        param,
        token,
        epoch,
        (attempted) => advancedParams.value[param] === attempted,
        (prev) => {
          advancedParams.value = {
            ...advancedParams.value,
            [param]: prev as AdvancedParameterSetting[typeof param],
          }
        },
      )
    })
}

const doRestoreAdvanced = async () => {
  if (!props.selectedCameraUuid) return

  processingAdvancedRestore.value = true

  const cameraUuid = props.selectedCameraUuid
  const generation = imageRequestGeneration.value
  pendingAdvanced.beginRestore()
  const payload: CameraControl = {
    camera_uuid: cameraUuid,
    action: "setImageAdjustmentEx",
    json: { set_default: 1 } as AdvancedParameterSetting
  }

  backendClient.request('POST', '/camera/control', payload)
    .then(data => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== imageRequestGeneration.value
      ) {
        return
      }
      advancedParams.value = pendingAdvanced.mergeRemote(data as AdvancedParameterSetting)
    })
    .catch(error => {
      console.error("Error restoring advanced parameters:", error.message)
    })
    .finally(() => {
      if (generation !== imageRequestGeneration.value) return
      processingAdvancedRestore.value = false
    })
}

</script>
