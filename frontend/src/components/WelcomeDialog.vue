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
      <p class="text-xs opacity-70 mt-3 text-white">
        Please complete the <b>Hardware Setup</b> section below to enable full functionality.
      </p>
    </div>
    <template #actions>
      <div />
      <BlueButton
        variant="filled"
        density="compact"
        @click="goToSetup"
      >
        Go to setup
      </BlueButton>
    </template>
  </StatusDialogShell>
</template>

<script setup lang="ts">
import { BlueButton } from '@bluerobotics/bluevue'
import StatusDialogShell from '@/components/StatusDialogShell.vue'

const props = defineProps<{
  show: boolean
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'go-to-setup'): void
}>()

const close = () => {
  emit('close')
}

const goToSetup = () => {
  emit('go-to-setup')
}
</script>
