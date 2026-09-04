<template>
  <StatusDialogShell
    :show="props.show"
    :title="dialogTitle"
    width="480px"
  >
    <template v-if="viewStatusCopy">
      <p
        class="text-sm text-white text-center mb-4 opacity-90"
        aria-live="polite"
      >
        {{ viewStatusCopy.body }}
      </p>
      <div class="rounded-md border border-white/10 p-3">
        <div class="flex items-center gap-2 text-sm font-medium text-white">
          <BlueSpinner
            color="white"
            :size="14"
            class="shrink-0"
          />
          <span>{{ viewStatusCopy.title }}</span>
        </div>
        <p class="text-xs mt-2 text-[#9ec9ef]">
          {{ viewStatusCopy.progress }}
        </p>
      </div>
    </template>
    <template v-else-if="viewAwaitingClose">
      <p class="text-sm text-white text-center">
        {{ viewRecoveryMessage }}
      </p>
    </template>
    <template v-else>
      <p
        v-if="problemsSummaryLine"
        class="text-sm text-white text-center mb-4 opacity-90"
        aria-live="polite"
      >
        {{ problemsSummaryLine }}
      </p>
      <p
        v-if="anyProblemSelfRecovers"
        class="text-sm text-white text-center mb-4 opacity-90"
        aria-live="polite"
      >
        Waiting while 4K Cam Manager keeps retrying in the background.
      </p>
      <div
        v-for="(problem, index) in viewProblems"
        :key="`${problem.kind}-${index}`"
        class="mb-4 last:mb-0 rounded-md border p-3"
        :class="SEVERITY_STYLES[problem.severity].class"
        aria-live="polite"
      >
        <div class="flex items-start gap-2 text-sm font-medium text-white">
          <BlueIcon
            :name="SEVERITY_STYLES[problem.severity].icon"
            :color="SEVERITY_STYLES[problem.severity].color"
            :size="16"
            class="mt-0.5 shrink-0"
          />
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <BlueSpinner
                v-if="problem.progress"
                color="white"
                :size="14"
                class="shrink-0"
              />
              <span>{{ problem.title }}</span>
            </div>
            <p
              v-if="problem.since"
              class="text-xs mt-1 opacity-70 font-normal"
            >
              {{ problem.since }}
            </p>
          </div>
        </div>
        <p class="text-sm text-white mt-2 opacity-90">
          {{ problem.body }}
        </p>
        <details
          v-if="problem.detail"
          class="mt-2"
        >
          <summary class="text-xs opacity-70 cursor-pointer text-white">
            Technical details
          </summary>
          <p class="text-xs font-mono opacity-70 mt-1 text-white break-words">
            {{ problem.detail }}
          </p>
        </details>
        <p
          v-if="problem.progress"
          class="text-xs mt-2 text-[#9ec9ef]"
        >
          {{ problem.progress }}
        </p>
        <div
          v-if="
            problem.showForget
              || problem.showGoToSetup
              || problem.showUpdateLuaScript
          "
          class="mt-3"
        >
          <BlueButton
            v-if="problem.showUpdateLuaScript"
            density="compact"
            theme="dark"
            class="mr-2"
            :disabled="actionInProgress != null"
            @click="runUpdateLuaScript"
          >
            Update autopilot script
          </BlueButton>
          <BlueButton
            v-if="problem.showGoToSetup"
            density="compact"
            theme="dark"
            class="mr-2"
            @click="goToSetup"
          >
            Open hardware setup
          </BlueButton>
          <BlueButton
            v-if="problem.showForget"
            density="compact"
            theme="dark"
            class="mr-2"
            :disabled="actionInProgress != null"
            @click="confirmForgetOpen = true"
          >
            Remove from setup
          </BlueButton>
        </div>
      </div>
    </template>
    <p
      v-if="actionError"
      class="text-xs mt-3 text-red-300"
      role="alert"
    >
      {{ actionError }}
    </p>
    <p
      v-else-if="actionInProgress"
      class="text-xs mt-3 text-[#9ec9ef] flex items-center gap-2"
      aria-live="polite"
    >
      <BlueSpinner
        color="#9ec9ef"
        :size="14"
      />
      {{ actionProgressLabel }}
    </p>
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
      <!-- Kept in the row even with nothing in it, so the footer's space-between still holds the
           trailing action against the right edge. -->
      <div class="flex items-center gap-2 min-w-0">
        <BlueButton
          v-if="!viewBusyCopy"
          density="compact"
          theme="dark"
          class="shrink-0"
          :disabled="actionInProgress != null"
          @click="copyDiagnostics"
        >
          Copy diagnostics
        </BlueButton>
      </div>
      <template v-if="!viewConnectionCopy">
        <BlueButton
          v-if="viewAwaitingClose && !viewBusyCopy"
          variant="filled"
          density="compact"
          theme="dark"
          @click="close"
        >
          Close
        </BlueButton>
        <BlueButton
          v-else
          density="compact"
          theme="dark"
          @click="minimize"
        >
          Minimize
        </BlueButton>
      </template>
    </template>
  </StatusDialogShell>

  <StatusDialogShell
    :show="confirmForgetOpen"
    title="Remove from setup?"
    :persistent="false"
    :logo="false"
    @dismiss="confirmForgetOpen = false"
  >
    <p class="text-sm text-white text-center">
      This deletes the saved hardware setup for this camera. The camera itself is unchanged — you can set it up again later.
    </p>
    <template #actions>
      <BlueButton
        variant="text"
        density="compact"
        theme="dark"
        @click="confirmForgetOpen = false"
      >
        Cancel
      </BlueButton>
      <BlueButton
        variant="filled"
        density="compact"
        theme="dark"
        color="#B71C1C"
        @click="runForget"
      >
        Remove
      </BlueButton>
    </template>
  </StatusDialogShell>

  <CopyFeedbackToast
    :message="copyFeedback"
    @dismiss="clearCopyFeedbackMessage"
  />
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'

import type { CameraConnectivity, SystemHealth } from '@/bindings/br4kcam_api'
import { BlueButton, BlueIcon, BlueSpinner } from '@bluerobotics/bluevue'
import CopyFeedbackToast from '@/components/CopyFeedbackToast.vue'
import StatusDialogShell from '@/components/StatusDialogShell.vue'
import { backendClient, type ConnectionState } from '@/utils/backendClient'
import { useCopyDiagnostics } from '@/utils/useCopyDiagnostics'
import { formatRequestError } from '@/utils/formatRequestError'
import type { HealthProblem } from '@/utils/systemHealthProblems'
import {
  allNotableProblemsSelfRecover,
  problemsSummary,
} from '@/utils/systemHealthProblems'

const SEVERITY_STYLES: Record<
  HealthProblem['severity'],
  { class: string; icon: string; color: string }
> = {
  error: {
    class: 'border-red-400/40 bg-red-950/20',
    icon: 'mdi-alert-circle',
    color: '#ef5350',
  },
  warning: {
    class: 'border-amber-400/40 bg-amber-950/20',
    icon: 'mdi-alert',
    color: '#ffb74d',
  },
  info: {
    class: 'border-white/10',
    icon: 'mdi-information',
    color: '#9ec9ef',
  },
}

/** Single in-progress state rendered as a spinner block: connecting, or a camera action. */
type StatusCopy = {
  title: string
  body: string
  progress: string
}

/** A deliberate long-running camera action the backend is running right now. */
type BusyState = {
  message: string
  rebooting: boolean
}

type LeaveSnapshot = {
  awaitingClose: boolean
  recoveryTitle: string
  recoveryMessage: string
  problems: HealthProblem[]
  connectionCopy: StatusCopy | null
  busyCopy: StatusCopy | null
}

type ActionKind = 'forget' | 'lua_script'

const props = defineProps<{
  show: boolean
  awaitingClose: boolean
  recoveryTitle?: string | null
  recoveryMessage?: string | null
  problems: HealthProblem[]
  systemHealth: SystemHealth | null
  cameraConnectivity?: CameraConnectivity | null
  cameraUuid?: string | null
  connectionState: ConnectionState
  everConnected: boolean
  busy?: BusyState | null
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'minimize'): void
  (e: 'forgotten', cameraUuid: string): void
  (e: 'go-to-setup'): void
}>()

const {
  copyFeedback,
  copyFallbackText,
  clearCopyFeedbackMessage,
  clearCopyState,
  copyDiagnostics: copyDiagnosticsPayload,
} = useCopyDiagnostics()

const confirmForgetOpen = ref(false)
const actionInProgress = ref<ActionKind | null>(null)
const actionError = ref<string | null>(null)

/** Freeze visible copy for the dialog's close after the parent clears props. */
const leaveSnapshot = ref<LeaveSnapshot | null>(null)
/** Last copy while `show` was true. Hide-time props are already cleared. */
const lastShown = ref<LeaveSnapshot | null>(null)

const connectionCopy = computed((): StatusCopy | null => {
  if (props.connectionState === 'connected') return null
  const connecting = props.connectionState === 'connecting'
  if (!props.everConnected) {
    if (connecting) {
      return {
        title: 'Connecting to 4K Cam Manager',
        body: 'Establishing a connection. Controls stay paused until the backend is ready.',
        progress: 'Connecting…',
      }
    }
    return {
      title: 'Unable to reach 4K Cam Manager',
      body: 'Retrying automatically. Controls stay paused until the backend is ready.',
      progress: 'Retrying…',
    }
  }
  if (connecting) {
    return {
      title: 'Reconnecting to 4K Cam Manager',
      body: 'Restoring the connection. Controls are paused until the backend is back.',
      progress: 'Reconnecting…',
    }
  }
  return {
    title: '4K Cam Manager unavailable',
    body: 'Connection lost. Reconnecting automatically — controls are paused until the backend is back.',
    progress: 'Waiting to retry…',
  }
})

const busyCopy = computed((): StatusCopy | null => {
  const busy = props.busy
  if (!busy) return null
  if (busy.rebooting) {
    return {
      title: busy.message,
      body: 'The camera restarts and stays unavailable until it finishes. You can minimize this and keep working with other cameras.',
      progress: 'Waiting for the camera to come back…',
    }
  }
  return {
    title: busy.message,
    body: 'Controls for this camera stay paused until it finishes. You can minimize this and keep working with other cameras.',
    progress: 'Working…',
  }
})

const liveSnapshot = computed((): LeaveSnapshot => ({
  awaitingClose: props.awaitingClose,
  recoveryTitle: props.recoveryTitle ?? 'All clear',
  recoveryMessage: props.recoveryMessage ?? 'All clear. Click Close to continue.',
  problems: props.problems,
  connectionCopy: connectionCopy.value,
  busyCopy: busyCopy.value,
}))

const viewConnectionCopy = computed(() => {
  if (props.show) return connectionCopy.value
  return leaveSnapshot.value?.connectionCopy ?? connectionCopy.value
})

const viewBusyCopy = computed(() => {
  if (props.show) return busyCopy.value
  return leaveSnapshot.value?.busyCopy ?? busyCopy.value
})

/** Connection takes the dialog over a camera action: nothing can run while the backend is away. */
const viewStatusCopy = computed(() => viewConnectionCopy.value ?? viewBusyCopy.value)

const actionProgressLabel = computed(() => {
  switch (actionInProgress.value) {
    case 'forget':
      return 'Removing camera from setup…'
    case 'lua_script':
      return 'Updating autopilot script…'
    default:
      return ''
  }
})

watch(
  liveSnapshot,
  (live) => {
    if (props.show) lastShown.value = live
  },
  { immediate: true },
)

watch(
  () => props.show,
  (show) => {
    if (show) {
      leaveSnapshot.value = null
      actionError.value = null
      actionInProgress.value = null
    } else {
      leaveSnapshot.value = lastShown.value
      clearCopyState()
    }
  },
)

const viewAwaitingClose = computed(() => {
  if (props.show) return props.awaitingClose
  return leaveSnapshot.value?.awaitingClose ?? props.awaitingClose
})
const viewRecoveryTitle = computed(() => {
  if (props.show) return props.recoveryTitle ?? 'All clear'
  return leaveSnapshot.value?.recoveryTitle ?? props.recoveryTitle ?? 'All clear'
})
const viewRecoveryMessage = computed(() => {
  if (props.show) return props.recoveryMessage ?? 'All clear. Click Close to continue.'
  return leaveSnapshot.value?.recoveryMessage
    ?? props.recoveryMessage
    ?? 'All clear. Click Close to continue.'
})
const viewProblems = computed(() => {
  if (props.show) return props.problems
  return leaveSnapshot.value?.problems ?? props.problems
})
const problemsSummaryLine = computed(() => problemsSummary(viewProblems.value))
const anyProblemSelfRecovers = computed(() =>
  allNotableProblemsSelfRecover(viewProblems.value),
)
const dialogTitle = computed(() => {
  if (viewStatusCopy.value) return viewStatusCopy.value.title
  if (viewAwaitingClose.value) return viewRecoveryTitle.value
  return 'System status'
})

const copyDiagnostics = async (): Promise<void> => {
  await copyDiagnosticsPayload({
    system_health: props.systemHealth,
    camera_connectivity: props.cameraConnectivity ?? null,
    problem_titles: viewProblems.value.map((problem) => problem.title),
    user_agent: navigator.userAgent,
  })
}

const close = (): void => {
  clearCopyState()
  emit('close')
}

const minimize = (): void => {
  clearCopyState()
  emit('minimize')
}

const runForget = async (): Promise<void> => {
  const cameraUuid = props.cameraUuid
  if (!cameraUuid || actionInProgress.value != null) return

  confirmForgetOpen.value = false
  actionError.value = null
  actionInProgress.value = 'forget'
  try {
    await backendClient.request('POST', '/autopilot/control', {
      camera_uuid: cameraUuid,
      action: 'forgetActuatorsConfig',
    })
    emit('forgotten', cameraUuid)
  } catch (error) {
    actionError.value = `Failed to remove camera from setup: ${formatRequestError(error)}`
  } finally {
    actionInProgress.value = null
  }
}

const runUpdateLuaScript = async (): Promise<void> => {
  const cameraUuid = props.cameraUuid
  if (!cameraUuid || actionInProgress.value != null) return

  actionError.value = null
  actionInProgress.value = 'lua_script'
  try {
    await backendClient.request('POST', '/autopilot/control', {
      camera_uuid: cameraUuid,
      action: 'exportLuaScript',
    })
  } catch (error) {
    actionError.value = `Failed to update autopilot script: ${formatRequestError(error)}`
  } finally {
    actionInProgress.value = null
  }
}

const goToSetup = (): void => {
  clearCopyState()
  emit('go-to-setup')
}
</script>