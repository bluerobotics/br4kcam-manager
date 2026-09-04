<template>
  <BlueTabs
    v-model="selectedChannel"
    :tabs="channelTabs"
    stretch
    theme="dark"
  />
  <div class="flex flex-col gap-[15px] px-6 pt-5">
    <div class="flex flex-col gap-[15px] px-13">
      <BlueSelect
        v-model="selectedVideoParameters.encode_profile"
        :items="encodeProfileOptions"
        :disabled="controlsDisabled"
        label="Encode Profile"
        theme="dark"
      />
      <BlueSelect
        v-model="selectedVideoParameters.encode_type"
        :items="encodeTypeOptions"
        :disabled="controlsDisabled"
        label="Encode Type"
        theme="dark"
      />
      <BlueSelect
        v-model="selectedResolutionKey"
        :items="resolutionOptions"
        :disabled="controlsDisabled"
        label="Resolution"
        theme="dark"
      />
      <BlueSelect
        v-model="selectedVideoParameters.rc_mode"
        :items="rcModeOptions"
        :disabled="controlsDisabled"
        label="Bitrate Type"
        theme="dark"
      />
      <BlueInput
        v-model="adjustedBitrate"
        name="stream-bitrate"
        width="240px"
        :disabled="controlsDisabled"
        label="Bitrate"
        suffix="kbps"
        type="number"
        :min="1024"
        :max="40960"
        :step="1024"
        theme="dark"
      />
      <BlueInput
        v-model="frameRate"
        name="stream-frame-rate"
        width="240px"
        :disabled="controlsDisabled"
        label="Frame Rate"
        type="number"
        :min="1"
        :max="selectedVideoParameters.max_framerate"
        theme="dark"
      />
      <BlueInput
        v-model="gop"
        name="stream-gop"
        width="240px"
        :disabled="controlsDisabled"
        label="I-Frame Interval (GOP)"
        type="number"
        :min="1"
        :max="100"
        theme="dark"
      />
    </div>

    <div class="my-3 border-t border-[#ffffff14]" />

    <div class="flex justify-end px-13">
      <BlueButton
        variant="filled"
        theme="dark"
        :loading="processingUpdate"
        :disabled="controlsDisabled"
        @click="updateVideoParameters"
      >
        {{
          processingUpdate
            ? "Processing..."
            : needs_restart
              ? "Apply and Restart"
              : "Apply"
        }}
      </BlueButton>
    </div>
  </div>
</template>

<script setup lang="ts">
import {
  BlueButton,
  BlueInput,
  BlueSelect,
  BlueTabs,
  type BlueTab,
} from '@bluerobotics/bluevue'
import { backendClient } from '@/utils/backendClient'
import { rebootCamera } from '@/utils/rebootCamera'
import { useCameraState } from '@/utils/useCameraState'
import { computed, ref, toRef, watch } from "vue"
import { enumToOptions } from "@/utils/enumUtils"
import {
  VideoChannelValue,
  VideoEncodeTypeValue,
  VideoEncodingProfileValue,
  VideoRcModeValue,
  type VideoParameterSettings,
  type VideoResolutionValue,
} from "@/bindings/br4kcam"

const props = defineProps<{
  selectedCameraUuid: string | null
  disabled: boolean
}>()

const processingUpdate = ref<boolean>(false)

const channelOptions = enumToOptions(VideoChannelValue)
const encodeProfileOptions = enumToOptions(VideoEncodingProfileValue)
const encodeTypeOptions = enumToOptions(VideoEncodeTypeValue)
const rcModeOptions = enumToOptions(VideoRcModeValue)

const controlsDisabled = computed(() => props.disabled || processingUpdate.value)

const channelTabs = computed<BlueTab[]>(() =>
  channelOptions
    .filter((option) => option.value < 1)
    .map((option) => ({
      value: option.value,
      label: option.name,
      disabled: controlsDisabled.value,
    })),
)

const resolutionOptions = computed(() =>
  downloadedVideoParameters.value.pixel_list?.map((res: VideoResolutionValue) => ({
    name: `${res.width}x${res.height}`,
  })) ?? [],
)

const selectedVideoParameters = ref<VideoParameterSettings>({
  channel: VideoChannelValue.MainStream,
})
const downloadedVideoParameters = ref<VideoParameterSettings>({})
const selectedVideoResolution = ref<VideoResolutionValue | null>(null)
const needs_restart = computed(() => {
  const selected = selectedVideoParameters.value
  const downloaded = downloadedVideoParameters.value
  return (
    selected.encode_profile !== downloaded.encode_profile ||
    selected.encode_type !== downloaded.encode_type ||
    selected.pic_width !== downloaded.pic_width ||
    selected.pic_height !== downloaded.pic_height
  )
})
const hasUserEditedVideo = ref(false)
const streamsRequestGeneration = ref(0)
let suppressUserEditFlag = false
let awaitingHydrate = false
/** Ignore camera/state venc pushes until reboot overlay clears. */
let awaitingRestartHydrate = false
/** Fallback so awaitingRestartHydrate cannot stick if disabled never flips. */
let restartHydrateTimeout: number | null = null
const RESTART_HYDRATE_TIMEOUT_MS = 30_000

const clearRestartHydrateLatch = (): void => {
  awaitingRestartHydrate = false
  if (restartHydrateTimeout !== null) {
    clearTimeout(restartHydrateTimeout)
    restartHydrateTimeout = null
  }
}

const armRestartHydrateLatch = (): void => {
  clearRestartHydrateLatch()
  awaitingRestartHydrate = true
  const generation = streamsRequestGeneration.value
  restartHydrateTimeout = window.setTimeout(() => {
    restartHydrateTimeout = null
    if (
      awaitingRestartHydrate &&
      generation === streamsRequestGeneration.value
    ) {
      awaitingRestartHydrate = false
    }
  }, RESTART_HYDRATE_TIMEOUT_MS)
}

watch(
  () => props.selectedCameraUuid,
  async (newValue) => {
    streamsRequestGeneration.value += 1
    hasUserEditedVideo.value = false
    processingUpdate.value = false
    clearRestartHydrateLatch()
    // Hold dirty latch until the first successful hydrate for this camera.
    suppressUserEditFlag = true
    awaitingHydrate = true
    if (!newValue) {
      suppressUserEditFlag = false
      awaitingHydrate = false
      return
    }
    // Always fetch so awaitingHydrate cannot latch forever if state never arrives.
    getVideoParameters(true)
  },
  { immediate: true },
)

watch(
  () => props.disabled,
  (disabled, wasDisabled) => {
    // Overlay cleared — re-fetch and clear latch only after a successful hydrate.
    if (wasDisabled && !disabled && awaitingRestartHydrate) {
      getVideoParameters(true)
    }
  },
)

watch(
  () => selectedVideoResolution.value,
  async (newValue) => {
    if (newValue) {
      selectedVideoParameters.value.pic_width = newValue.width
      selectedVideoParameters.value.pic_height = newValue.height
    }
  }
)

watch(
  () => selectedVideoParameters.value.channel,
  async (newValue, oldValue) => {
    if (newValue !== oldValue) {
      getVideoParameters(true)
    }
  }
)

watch(
  [selectedVideoParameters, selectedVideoResolution],
  () => {
    // Allow dirty during hydrate so a successful fetch cannot clobber in-progress edits.
    if (!suppressUserEditFlag) {
      hasUserEditedVideo.value = true
    }
  },
  { deep: true },
)

const applyCameraStateEvent = (body: unknown) => {
  if (!props.selectedCameraUuid) return
  if (typeof body !== 'object' || body === null) return

  const data = body as Record<string, unknown>
  if (data.camera_uuid !== props.selectedCameraUuid) return
  if (!data.video_parameters) return
  if (hasUserEditedVideo.value) return
  if (awaitingRestartHydrate) {
    const settings = data.video_parameters as VideoParameterSettings
    const currentChannel = selectedVideoParameters.value.channel ?? VideoChannelValue.MainStream
    if (settings.channel !== currentChannel) return
    // First matching post-restart video snapshot — accept and clear the latch.
    update_video_parameter_values(settings)
    clearRestartHydrateLatch()
    return
  }

  const settings = data.video_parameters as VideoParameterSettings
  const currentChannel = selectedVideoParameters.value.channel ?? VideoChannelValue.MainStream
  if (settings.channel !== currentChannel) return

  update_video_parameter_values(settings)
}

useCameraState(toRef(props, 'selectedCameraUuid'), applyCameraStateEvent)

const selectedChannel = computed({
  get: (): number => selectedVideoParameters.value.channel ?? VideoChannelValue.MainStream,
  set: (value: string | number) => {
    selectedVideoParameters.value.channel = Number(value) as VideoChannelValue
  },
})

// The select matches its options by value, so the resolution travels as the same "WxH" string it
// is labelled with. As an object it would never match: every hydrate builds a fresh one.
const selectedResolutionKey = computed({
  get: (): string | null => {
    const resolution = selectedVideoResolution.value
    return resolution ? `${resolution.width}x${resolution.height}` : null
  },
  set: (key: string | number | null) => {
    const [width, height] = String(key ?? '').split('x').map(Number)
    if (!Number.isFinite(width) || !Number.isFinite(height)) return
    selectedVideoResolution.value = { width, height } as VideoResolutionValue
  },
})

const adjustedBitrate = computed({
  get: (): number | null => selectedVideoParameters.value.bitrate ?? null,
  set: (newValue: string | number | null) => {
    const value = Number(newValue)
    if (!Number.isFinite(value)) return
    selectedVideoParameters.value.bitrate = Math.round(value / 1024) * 1024
  }
})

const frameRate = computed({
  get: (): number | null => selectedVideoParameters.value.frame_rate ?? null,
  set: (newValue: string | number | null) => {
    const value = Number(newValue)
    if (!Number.isFinite(value)) return
    selectedVideoParameters.value.frame_rate = value
  },
})

const gop = computed({
  get: (): number | null => selectedVideoParameters.value.gop ?? null,
  set: (newValue: string | number | null) => {
    const value = Number(newValue)
    if (!Number.isFinite(value)) return
    selectedVideoParameters.value.gop = value
  },
})

const updateVideoParameters = () => {
  if (!props.selectedCameraUuid) {
    return
  }

  const cameraUuid = props.selectedCameraUuid
  const generation = streamsRequestGeneration.value
  processingUpdate.value = true

  console.debug(selectedVideoParameters.value)

  const video_parameter_settings = selectedVideoParameters.value
  const shouldRestart = needs_restart.value
  let handedOffRestart = false

  const payload = {
    camera_uuid: cameraUuid,
    action: "setVencConf",
    json: video_parameter_settings,
  }

  backendClient
    .request('POST', '/camera/control', payload)
    .then((data) => {
      if (
        props.selectedCameraUuid === cameraUuid &&
        generation === streamsRequestGeneration.value
      ) {
        // Clear dirty latch / sync downloaded* even when we also reboot.
        update_video_parameter_values(data as VideoParameterSettings)
      }
      if (shouldRestart) {
        // Always reboot the camera that accepted the venc change, even if the
        // user switched selection afterward.
        handedOffRestart = true
        doRestart(cameraUuid)
        if (
          props.selectedCameraUuid === cameraUuid &&
          generation === streamsRequestGeneration.value
        ) {
          armRestartHydrateLatch()
        }
      }
    })
    .catch((error) =>
      console.error(
        `Error sending ${video_parameter_settings}':`,
        error.message
      )
    )
    .finally(() => {
      if (generation !== streamsRequestGeneration.value) {
        return
      }
      // Restart path owns the spinner via doRestart after a successful POST.
      if (!handedOffRestart) {
        processingUpdate.value = false
      }
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

  const generation = streamsRequestGeneration.value
  const manageSpinner = props.selectedCameraUuid === uuid
  console.log("Restarting...")

  if (manageSpinner) {
    processingUpdate.value = true
  }

  rebootCamera(uuid)
    .then((data) => {
      if (
        props.selectedCameraUuid !== uuid ||
        generation !== streamsRequestGeneration.value
      ) {
        return
      }
      console.log("Got an answer from the restarting request", data)
    })
    .catch((error) => {
      console.error(
        `Error sending restart':`,
        error.message
      )
      if (generation === streamsRequestGeneration.value) {
        clearRestartHydrateLatch()
      }
    })
    .finally(() => {
      if (!manageSpinner) return
      if (generation !== streamsRequestGeneration.value) return
      processingUpdate.value = false
    })
}

const getVideoParameters = (update: boolean) => {
  if (!props.selectedCameraUuid) {
    return
  }

  const cameraUuid = props.selectedCameraUuid
  const generation = streamsRequestGeneration.value
  const video_parameter_settings = {
    channel: selectedVideoParameters.value.channel ?? VideoChannelValue.MainStream,
  }

  const payload = {
    camera_uuid: cameraUuid,
    action: "getVencConf",
    json: video_parameter_settings,
  }

  backendClient
    .request('POST', '/camera/control', payload)
    .then((data) => {
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== streamsRequestGeneration.value
      ) {
        return
      }
      const settings: VideoParameterSettings =
        data as VideoParameterSettings

      if (update) {
        if (!hasUserEditedVideo.value) {
          update_video_parameter_values(settings)
        }
        if (awaitingRestartHydrate && !hasUserEditedVideo.value) {
          clearRestartHydrateLatch()
        }
        // End initial hydrate window even when dirty skipped the overwrite.
        if (awaitingHydrate) {
          awaitingHydrate = false
          suppressUserEditFlag = false
        }
      }
    })
    .catch((error) => {
      console.error(`Error sending getVencConf request:`, error.message)
      if (
        props.selectedCameraUuid !== cameraUuid ||
        generation !== streamsRequestGeneration.value
      ) {
        return
      }
      // Don't latch the form dirty forever when the initial fetch fails.
      hasUserEditedVideo.value = false
      suppressUserEditFlag = false
      awaitingHydrate = false
    })
}

const update_video_parameter_values = (settings: VideoParameterSettings) => {
  suppressUserEditFlag = true
  downloadedVideoParameters.value = { ...settings }

  selectedVideoParameters.value = { ...settings }
  selectedVideoParameters.value.pixel_list = undefined

  selectedVideoResolution.value = {
    width: settings.pic_width ?? selectedVideoResolution.value?.width ?? 0,
    height: settings.pic_height ?? selectedVideoResolution.value?.height ?? 0,
  } as VideoResolutionValue
  hasUserEditedVideo.value = false
  awaitingHydrate = false
  queueMicrotask(() => {
    suppressUserEditFlag = false
  })
}
</script>
