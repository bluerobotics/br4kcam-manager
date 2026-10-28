<template>
  <StatusDialogShell
    :show="props.show"
    title="Hardware Setup Required"
    :persistent="false"
    @dismiss="close"
  >
    <div class="text-center">
      <p class="text-sm text-white">
        Your camera actuators are not configured yet.
      </p>
      <p class="text-sm mt-3 text-white">
        Which lens does this camera have?
      </p>
      <p class="text-xs opacity-70 mt-3 text-white">
        A <b>motorized</b> lens has focus and zoom servos. A <b>fixed-focus</b> lens has neither, so only tilt is set up.
        You can change this later in the <b>Hardware Setup</b> section below.
      </p>
    </div>
    <template #actions>
      <BlueButton
        variant="text"
        density="compact"
        @click="goToSetup('FixedFocus')"
      >
        Fixed focus
      </BlueButton>
      <BlueButton
        variant="filled"
        density="compact"
        @click="goToSetup('Motorized')"
      >
        Motorized focus &amp; zoom
      </BlueButton>
    </template>
  </StatusDialogShell>
</template>

<script setup lang="ts">
import { BlueButton } from '@bluerobotics/bluevue'
import StatusDialogShell from '@/components/StatusDialogShell.vue'
import type { LensType } from '@/bindings/autopilot'

const props = defineProps<{
  show: boolean
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'go-to-setup', lens: LensType): void
}>()

const close = () => {
  emit('close')
}

const goToSetup = (lens: LensType) => {
  emit('go-to-setup', lens)
}
</script>
