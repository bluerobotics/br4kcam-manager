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
    <textarea
      v-if="copyFallbackText"
      :value="copyFallbackText"
      readonly
      class="mt-2 w-full text-xs font-mono opacity-80 text-white"
      rows="4"
      aria-label="Diagnostics text for manual copy"
      @focus="($event.target as HTMLTextAreaElement).select()"
    />
    <template #actions>
      <BlueButton
        density="compact"
        theme="dark"
        @click="copyDiagnostics"
      >
        Copy diagnostics
      </BlueButton>
      <BlueButton
        variant="filled"
        density="compact"
        @click="close"
      >
        Close
      </BlueButton>
    </template>
  </StatusDialogShell>

  <CopyFeedbackToast
    :message="copyFeedback"
    @dismiss="clearCopyFeedbackMessage"
  />
</template>

<script setup lang="ts">
import { BlueButton } from '@bluerobotics/bluevue'
import type { CameraConnectivity, SystemHealth } from '@/bindings/br4kcam_api'
import CopyFeedbackToast from '@/components/CopyFeedbackToast.vue'
import StatusDialogShell from '@/components/StatusDialogShell.vue'
import { useCopyDiagnostics } from '@/utils/useCopyDiagnostics'

const props = defineProps<{
  message: string | null
  systemHealth: SystemHealth | null
  cameraConnectivity?: CameraConnectivity | null
}>()

const emit = defineEmits<{
  (e: 'close'): void
}>()

const {
  copyFeedback,
  copyFallbackText,
  clearCopyFeedbackMessage,
  clearCopyState,
  copyDiagnostics: copyDiagnosticsPayload,
} = useCopyDiagnostics()

const copyDiagnostics = async (): Promise<void> => {
  await copyDiagnosticsPayload({
    system_health: props.systemHealth,
    camera_connectivity: props.cameraConnectivity ?? null,
    problem_titles: props.message ? [props.message] : [],
    user_agent: navigator.userAgent,
  })
}

const close = () => {
  clearCopyState()
  emit('close')
}
</script>
