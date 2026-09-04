<template>
  <StatusDialogShell
    :show="!!props.message"
    title="Operation Failed"
    :persistent="false"
    @dismiss="close"
  >
    <div class="text-center">
      <p class="text-sm text-white">
        {{ props.message }}
      </p>
      <p class="text-xs opacity-70 mt-3 text-white">
        Please try again. If the problem persists, contact support
      </p>
    </div>
    <template #actions>
      <div />
      <BlueButton
        variant="filled"
        density="compact"
        @click="close"
      >
        Close
      </BlueButton>
    </template>
  </StatusDialogShell>
</template>

<script setup lang="ts">
import { BlueButton } from '@bluerobotics/bluevue'
import StatusDialogShell from '@/components/StatusDialogShell.vue'

const props = defineProps<{
  message: string | null
}>()

const emit = defineEmits<{
  (e: 'close'): void
}>()

const close = () => {
  emit('close')
}
</script>
