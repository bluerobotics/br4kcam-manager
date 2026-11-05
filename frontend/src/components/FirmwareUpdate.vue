<template>
  <div
    v-if="newer"
    class="mx-6 mt-4 rounded-[6px] border border-[#4fc3f788] bg-[#1a3a52] px-3 py-2 text-[#b3e5fc]"
  >
    <div class="flex items-center justify-between gap-3 text-sm">
      <span v-if="!confirming">
        Camera firmware {{ newer }} is available (running {{ current }}).
      </span>
      <span v-else>
        The camera will be unavailable for a few minutes. Do not power off the vehicle during the update.
      </span>
      <div class="flex shrink-0 gap-2">
        <BlueButton
          density="compact"
          theme="dark"
          :disabled="disabled"
          @click="confirming ? (confirming = false) : (newer = null)"
        >
          {{ confirming ? 'Cancel' : 'Not now' }}
        </BlueButton>
        <BlueButton
          density="compact"
          variant="filled"
          theme="dark"
          :disabled="disabled"
          @click="confirming ? update() : (confirming = true)"
        >
          {{ confirming ? 'Update now' : 'Update firmware' }}
        </BlueButton>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { BlueButton } from '@bluerobotics/bluevue'
import { ref, watch } from 'vue'
import { backendClient } from '@/utils/backendClient'
import { newerFirmware } from '@/utils/firmwareVersion'

const props = defineProps<{
  selectedCameraUuid: string | null
  disabled: boolean
}>()

type FirmwareUpdates = { current: string, available: string[] }

const current = ref<string | null>(null)
const newer = ref<string | null>(null)
const confirming = ref(false)

const refresh = (): void => {
  const cameraUuid = props.selectedCameraUuid
  newer.value = null
  confirming.value = false
  if (!cameraUuid || props.disabled) return

  backendClient
    .request('POST', '/camera/control', { camera_uuid: cameraUuid, action: 'getFirmwareUpdates' })
    .then((data) => {
      if (props.selectedCameraUuid !== cameraUuid) return
      const updates = data as FirmwareUpdates
      current.value = updates.current
      newer.value = newerFirmware(updates.current, updates.available)
    })
    // No firmware server configured or reachable: nothing to offer.
    .catch((error) => console.debug('Firmware update check failed:', error.message))
}

// Re-check once the camera is usable again (after the update's reboot overlay).
watch(() => [props.selectedCameraUuid, props.disabled], refresh, { immediate: true })

const update = (): void => {
  const version = newer.value
  if (!props.selectedCameraUuid || !version) return
  newer.value = null
  // Progress and errors come through the backend's camera UI overlay.
  backendClient
    .request('POST', '/camera/control', {
      camera_uuid: props.selectedCameraUuid,
      action: 'updateFirmware',
      json: { version },
    })
    .catch((error) => console.error('Failed updating camera firmware:', error.message))
}
</script>
