<template>
  <div
    v-if="visible"
    class="mx-6 mt-4 rounded-[6px] border border-[#4fc3f788] bg-[#1a3a52] px-3 py-2 text-[#b3e5fc]"
  >
    <div class="flex items-center justify-between gap-3 text-sm">
      <span>
        This camera and browser support H.265, which gives better image quality at the same bitrate.
      </span>
      <div class="flex shrink-0 gap-2">
        <BlueButton
          density="compact"
          theme="dark"
          :disabled="disabled"
          @click="dismiss"
        >
          Not now
        </BlueButton>
        <BlueButton
          density="compact"
          variant="filled"
          theme="dark"
          :disabled="disabled"
          @click="switchToH265"
        >
          Switch to H.265
        </BlueButton>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { BlueButton } from '@bluerobotics/bluevue'
import { computed, ref, watch } from 'vue'
import { backendClient } from '@/utils/backendClient'

const props = defineProps<{
  selectedCameraUuid: string | null
  disabled: boolean
}>()

type H265Support = { supported: boolean, reason: string | null, enabled: boolean }

const DISMISSED_KEY = 'h265-suggestion-dismissed'

// Cockpit plays the stream through WebRTC in this same browser, so it must decode H.265.
const browserSupportsH265 = (() => {
  try {
    return RTCRtpReceiver.getCapabilities('video')?.codecs
      .some((codec) => codec.mimeType.toLowerCase() === 'video/h265') ?? false
  } catch {
    return false
  }
})()

const support = ref<H265Support | null>(null)
const dismissed = ref<string[]>(readDismissed())

function readDismissed(): string[] {
  try {
    return JSON.parse(localStorage.getItem(DISMISSED_KEY) ?? '[]')
  } catch {
    return []
  }
}

const visible = computed(() =>
  props.selectedCameraUuid !== null
  && support.value?.supported === true
  && !support.value.enabled
  && !dismissed.value.includes(props.selectedCameraUuid),
)

const refresh = (): void => {
  const cameraUuid = props.selectedCameraUuid
  support.value = null
  if (!browserSupportsH265 || !cameraUuid || props.disabled) return

  backendClient
    .request('POST', '/camera/control', { camera_uuid: cameraUuid, action: 'getH265Support' })
    .then((data) => {
      if (props.selectedCameraUuid !== cameraUuid) return
      support.value = data as H265Support
      if (support.value.reason) console.debug(`H.265 not suggested: ${support.value.reason}`)
    })
    .catch((error) => console.debug('H.265 support check failed:', error.message))
}

// Re-check once the camera is usable again (after reboots or the switch overlay).
watch(() => [props.selectedCameraUuid, props.disabled], refresh, { immediate: true })

const dismiss = (): void => {
  if (!props.selectedCameraUuid) return
  dismissed.value = [...dismissed.value, props.selectedCameraUuid]
  try {
    localStorage.setItem(DISMISSED_KEY, JSON.stringify(dismissed.value))
  } catch {
    // Per-browser convenience only.
  }
}

const switchToH265 = (): void => {
  if (!props.selectedCameraUuid) return
  support.value = null
  // Progress and errors come through the backend's camera UI overlay.
  backendClient
    .request('POST', '/camera/control', { camera_uuid: props.selectedCameraUuid, action: 'setH265' })
    .catch((error) => console.error('Failed switching to H.265:', error.message))
    .finally(refresh)
}
</script>
