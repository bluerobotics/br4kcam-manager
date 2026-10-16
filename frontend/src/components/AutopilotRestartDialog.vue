<template>
  <StatusDialogShell
    :show="show"
    title="Restart the autopilot?"
    :persistent="false"
    :logo="false"
    @dismiss="emit('cancel')"
  >
    <p class="text-sm text-white text-center">
      This may restart the autopilot. Actuator control is unavailable for about a minute while it comes back.
    </p>
    <template #actions>
      <BlueButton
        variant="text"
        density="compact"
        theme="dark"
        @click="emit('cancel')"
      >
        Cancel
      </BlueButton>
      <BlueButton
        variant="filled"
        density="compact"
        theme="dark"
        @click="emit('confirm')"
      >
        Continue
      </BlueButton>
    </template>
  </StatusDialogShell>
</template>

<script setup lang="ts">
import { BlueButton } from '@bluerobotics/bluevue'

import StatusDialogShell from './StatusDialogShell.vue'

defineProps<{ show: boolean }>()

const emit = defineEmits<{ confirm: []; cancel: [] }>()
</script>
