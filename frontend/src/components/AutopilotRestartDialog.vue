<template>
  <StatusDialogShell
    :show="show"
    title="Restart the autopilot?"
    :persistent="false"
    :logo="false"
    :width="hasPreview ? '520px' : undefined"
    @dismiss="emit('cancel')"
  >
    <p class="text-sm text-white text-center">
      This may restart the autopilot. Actuator control is unavailable for about a minute while it comes back.
    </p>
    <template v-if="preview">
      <div
        v-if="preview.changes.length > 0"
        class="mt-4 text-white"
      >
        <p class="text-sm font-medium">
          Settings that will change
        </p>
        <ul class="list-disc pl-5 mt-1 text-xs opacity-80 break-words">
          <li
            v-for="change in preview.changes"
            :key="change"
          >
            {{ change }}
          </li>
        </ul>
      </div>
      <div
        v-if="preview.released.length > 0"
        class="mt-4 rounded-[6px] border border-[#FB8C0055] bg-[#FB8C0022] p-3 text-[#FFB74D]"
      >
        <p class="text-sm font-medium">
          Will be released
        </p>
        <p class="text-xs mt-1 opacity-90">
          A fixed-focus lens has no focus or zoom, so the autopilot parameters the motorized setup
          used are turned off, and its script is removed or rewritten without this camera.
          Anything another camera still uses, or that was changed since, is left alone.
        </p>
        <ul class="list-disc pl-5 mt-2 text-xs break-words">
          <li
            v-for="released in preview.released"
            :key="released"
          >
            {{ released }}
          </li>
        </ul>
      </div>
    </template>
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
import { computed } from 'vue'
import { BlueButton } from '@bluerobotics/bluevue'

import type { SetupPreview } from '@/bindings/autopilot'
import StatusDialogShell from './StatusDialogShell.vue'

const props = defineProps<{
  show: boolean
  /** What the apply will change, when known. */
  preview?: SetupPreview | null
}>()

const emit = defineEmits<{ confirm: []; cancel: [] }>()

const hasPreview = computed(
  () => props.preview != null && (props.preview.changes.length > 0 || props.preview.released.length > 0),
)
</script>
