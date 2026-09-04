<template>
  <div
    class="text-white p-0 pb-[10px] rounded-[8px] bluevue-elevation-5 no-user-select"
    :class="[
      theme === 'dark' ? 'bg-[#363636]' : 'bg-[#F5F5F5]',
      isCockpitMode ? 'transparent-card mx-4 mt-4 mb-14 overflow-x-hidden' : 'mt-6 mx-auto max-w-[800px]',
    ]"
  >
    <BlueHeader
      theme="dark"
      leading-width="400px"
      :background="isCockpitMode ? '#2C2C2C88' : '#15151577'"
    >
      <template #leading>
        <BlueHeaderMenu
          :items="toolsMenuItems"
          :icon-size="30"
          tooltip="Camera tools"
          theme="dark"
        />
        <BlueHeaderSelector
          v-model="selectedCameraUUID"
          :items="cameraSelectItems"
          label="Camera"
          placeholder="No camera"
          theme="dark"
        />
      </template>
      <BlueTooltip
        v-if="showBusyChip"
        :text="busyChipTooltip"
        placement="bottom"
        theme="dark"
      >
        <BlueSpinner
          color="#9ec9ef"
          :size="16"
          class="cursor-pointer health-focusable"
          role="button"
          tabindex="0"
          aria-label="Open system status"
          @click="onBusyChipOpen"
          @keydown.enter="onBusyChipOpen"
          @keydown.space.prevent="onBusyChipOpen"
        />
      </BlueTooltip>
      <BlueTooltip
        v-if="showHealthChip"
        :text="healthChipTooltip"
        placement="bottom"
        theme="dark"
      >
        <BlueIcon
          name="mdi-alert"
          color="#FFB74D"
          :size="18"
          class="cursor-pointer health-focusable"
          role="button"
          tabindex="0"
          aria-label="Open system status"
          @click="onDegradedBannerOpen"
          @keydown.enter="onDegradedBannerOpen"
          @keydown.space.prevent="onDegradedBannerOpen"
        />
      </BlueTooltip>
      <BlueTooltip
        :text="connectionTooltip"
        placement="bottom"
        theme="dark"
        class="connection-tooltip"
      >
        <span class="connection-status-wrap">
          <BlueIcon
            :name="connectionIcon"
            :color="connectionColor"
            :size="18"
            class="connection-status-icon"
            :class="{ 'connection-status-icon--spinning': connectionState === 'connecting' }"
          />
        </span>
      </BlueTooltip>
      <BlueButtonGroup
        :button-items="configButtons"
        :theme="theme"
        type="switch"
        width="156px"
      />
    </BlueHeader>
    <div class="health-banners-sticky">
      <div
        v-if="showStaleBundleBanner"
        class="mx-6 mt-4 rounded-[6px] border border-[#4fc3f788] bg-[#1a3a52] px-3 py-2 text-[#b3e5fc]"
      >
        <div class="flex items-center justify-between gap-3 text-sm">
          <span>
            The backend was updated while this page was open. Reload to load the matching UI.
          </span>
          <BlueButton
            density="compact"
            theme="dark"
            class="shrink-0"
            @click="reloadPage"
          >
            Reload
          </BlueButton>
        </div>
      </div>
      <div
        v-if="showDegradedBanner"
        class="mx-6 mt-4 cursor-pointer rounded-[6px] border border-[#c9a22788] bg-[#5c4a12] px-3 py-2 text-[#ffe082] health-focusable"
        role="button"
        tabindex="0"
        @click="onDegradedBannerOpen"
        @keydown.enter="onDegradedBannerOpen"
        @keydown.space.prevent="onDegradedBannerOpen"
      >
        <div class="text-sm font-medium">
          {{ degradedBannerTitle }}
        </div>
      </div>
    </div>
    <div
      class="transition-all duration-300 ease-in-out"
      :class="isCockpitMode ? 'min-w-0' : 'min-w-[650px]'"
    >
      <div
        v-if="discoveryEmpty"
        class="px-6 py-8 text-center text-sm opacity-70"
      >
        No cameras discovered yet. Connect a 4K Cam and it will appear here.
      </div>
      <div v-if="configMode === 'basic'">
        <BasicSettings
          ref="cameraControls"
          :selected-camera-uuid="selectedCameraUUID"
          :disabled="baseControlsDisabled"
          :camera-controls-disabled="cameraBackedControlsDisabled"
          :loading="uiLoading"
          :cockpit-mode="isCockpitMode"
          :one-push-awb="onePushAwb"
          :backend-connected="backendConnected"
          :welcome-overlay-unblocked="welcomeOverlayUnblocked"
        />
      </div>
      <div v-if="configMode === 'advanced'">
        <BlueTabs
          v-model="tab"
          :tabs="advancedTabs"
          stretch
          theme="dark"
        />

        <ImageTab
          v-if="tab === 'image'"
          :selected-camera-uuid="selectedCameraUUID"
          :disabled="cameraBackedControlsDisabled"
          :one-push-awb="onePushAwb"
        />
        <StreamsTab
          v-else-if="tab === 'streams'"
          :selected-camera-uuid="selectedCameraUUID"
          :disabled="cameraBackedControlsDisabled"
        />
      </div>
      <HealthDiagnostics
        :system-health="systemHealth"
        :camera-connectivity="cameraConnectivity"
        :problem-titles="healthProblems.map((problem) => problem.title)"
      />
    </div>
  </div>
  <SystemStatusDialog
    :show="showSystemStatusDialog"
    :awaiting-close="healthDialogAwaitingClose"
    :recovery-title="healthDialogRecoveryTitle"
    :recovery-message="healthDialogRecoveryMessage"
    :problems="healthProblems"
    :system-health="systemHealth"
    :camera-connectivity="cameraConnectivity"
    :camera-uuid="selectedCameraUUID"
    :connection-state="dialogConnectionState"
    :ever-connected="connectionPhaseEverConnected"
    :busy="busyDialogState"
    @close="onHealthDialogClose"
    @minimize="onSystemStatusDialogMinimize"
    @forgotten="onHealthCameraForgotten"
    @go-to-setup="onHealthGoToSetup"
  />
  <ErrorDialog
    :message="errorDialogMessage"
    @close="dismissErrorDialog"
  />
  <WarningToast
    :message="warningToastMessage"
    :icon="warningToastIcon"
  />
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRouteQuery } from '@vueuse/router'

import type { Camera } from '@/bindings/mcm_client'
import type {
  CameraConnectivity,
  CameraStateEvent,
  CameraUiState,
  OnePushAwbStatus,
} from '@/bindings/br4kcam_api'
import HealthDiagnostics from '@/components/HealthDiagnostics.vue'
import BasicSettings from '@/components/BasicSettings.vue'
import {
  BlueButton,
  BlueButtonGroup,
  BlueHeader,
  BlueHeaderMenu,
  BlueHeaderSelector,
  type BlueHeaderSelectorItem,
  BlueIcon,
  type BlueMenuItem,
  BlueSpinner,
  type BlueTab,
  BlueTabs,
  BlueTooltip,
} from '@bluerobotics/bluevue'
import ImageTab from '@/components/ImageTab.vue'
import StreamsTab from '@/components/StreamsTab.vue'
import SystemStatusDialog from '@/components/SystemStatusDialog.vue'
import WarningToast from '@/components/WarningToast.vue'
import {
  closeHealthDialog,
  degradedBannerCopy,
  enrichHealthProblems,
  evaluateHealthFlags,
  healthDialogStateOnDisconnect,
  healthDialogView,
  initialHealthDialogState,
  minimizeHealthDialog,
  noteActiveProblems,
  noteForgetSuccess,
  recoveryWhileMinimizedToast,
  reduceHealthDialogOnProblems,
  reopenHealthDialog,
  type HealthDialogState,
} from '@/utils/healthDialogState'
import { backendClient, type ConnectionState, type ConnectionStats } from '@/utils/backendClient'
import { formatRequestError } from '@/utils/formatRequestError'
import { useSystemHealth } from '@/utils/useSystemHealth'

type CameraOption = {
  uuid: string
  label: string
  missing: boolean
}

/** Minimum time the connecting/reconnecting dialog stays up, so it never just flashes. */
const MIN_CONNECTION_PHASE_MS = 1000

const tab = ref<string | number>('image')
const advancedTabs: BlueTab[] = [
  { value: 'image', label: 'Image' },
  { value: 'streams', label: 'Streams' },
  { value: 'configs', label: 'Configs', disabled: true },
]
const cameras = ref<Camera[]>([])
const selectedCameraUUID = ref<string | null>(null)
/** Last known labels so a selected camera stays visible across discovery flaps. */
const cameraLabelByUuid = ref<Record<string, string>>({})
const {
  systemHealth,
  discoveryEmpty,
  expectedMissing,
} = useSystemHealth()
const healthDialog = ref<HealthDialogState>(initialHealthDialogState())
const healthProblemsNowMs = ref(Date.now())
const cameraConnectivity = ref<CameraConnectivity>('unknown')
const cameraStreamError = ref<string | null>(null)
const cameraOnvifAuthError = ref<string | null>(null)
/** Per-camera actuators_configured from camera/state — gates setup health problems. */
const actuatorsConfiguredByUuid = ref<Record<string, boolean>>({})
const connectionState = ref<ConnectionState>('connecting')
let everConnected = false
/** Connection state the dialog renders, held past reconnect so it cannot flash by. */
const connectionPhase = ref<ConnectionState | null>('connecting')
/** `everConnected` as of when the current connection phase began, so its copy is stable. */
const connectionPhaseEverConnected = ref(false)
let connectionPhaseSince = Date.now()
let connectionPhaseTimer: ReturnType<typeof setTimeout> | null = null
const connectionStats = ref<ConnectionStats | null>(null)
const disconnectedSince = ref<Date | null>(null)

const sinceFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: 'medium',
  timeStyle: 'short',
})

const formatSince = (value: string | Date): string => {
  const date = value instanceof Date ? value : new Date(value)
  return sinceFormatter.format(date)
}

const formatKbps = (kbps: number): string => {
  if (kbps < 10) {
    return kbps.toFixed(1)
  }
  return Math.round(kbps).toString()
}

const connectionStatusLine = computed(() => {
  if (connectionState.value === 'connecting') {
    return 'Connecting…'
  }
  if (connectionState.value === 'connected' && connectionStats.value) {
    return `Connected since ${formatSince(connectionStats.value.since)}`
  }
  if (connectionState.value === 'disconnected' && disconnectedSince.value) {
    return `Disconnected since ${formatSince(disconnectedSince.value)}`
  }
  return 'Disconnected'
})

// A tooltip carries one string, so the bandwidth rows are held in their columns by a tab stop
// rather than by a grid. The rule that preserves the tabs is beside .connection-tooltip below.
const connectionTooltip = computed(() => {
  const lines = [connectionStatusLine.value]
  const stats = connectionStats.value
  if (connectionState.value === 'connected' && stats) {
    const clients = stats.clients_connected
    lines.push(`${clients} ${clients === 1 ? 'client' : 'clients'} connected`)
    lines.push(
      `This\t↑ ${formatKbps(stats.this_upload_kbps)} ↓ ${formatKbps(stats.this_download_kbps)} kbps`,
    )
    lines.push(
      `All\t↑ ${formatKbps(stats.total_upload_kbps)} ↓ ${formatKbps(stats.total_download_kbps)} kbps`,
    )
  }
  return lines.join('\n')
})

const connectionIcon = computed(() => {
  switch (connectionState.value) {
    case 'connected':
      return 'mdi-lan-connect'
    case 'connecting':
      return 'mdi-sync'
    case 'disconnected':
    default:
      return 'mdi-lan-disconnect'
  }
})

const connectionColor = computed(() => {
  switch (connectionState.value) {
    case 'connected':
      return '#66bb6a'
    case 'connecting':
      return '#ffb74d'
    case 'disconnected':
    default:
      return '#ef5350'
  }
})

const backendConnected = computed(() => connectionState.value === 'connected')

const cameraOptions = computed((): CameraOption[] => {
  const options: CameraOption[] = cameras.value.map((camera) => ({
    uuid: camera.uuid,
    label: camera.hostname,
    missing: false,
  }))
  const listed = new Set(cameras.value.map((camera) => camera.uuid))
  for (const ghost of expectedMissing.value) {
    if (!listed.has(ghost.uuid)) {
      options.push({
        uuid: ghost.uuid,
        label:
          ghost.last_hostname
          ?? cameraLabelByUuid.value[ghost.uuid]
          ?? `Camera ${ghost.uuid.slice(0, 8)}`,
        missing: true,
      })
      listed.add(ghost.uuid)
    }
  }
  // Keep the current selection visible across brief list/health races.
  const selected = selectedCameraUUID.value
  if (selected && !listed.has(selected)) {
    options.push({
      uuid: selected,
      label: cameraLabelByUuid.value[selected] ?? `Camera ${selected.slice(0, 8)}`,
      missing: !cameras.value.some((camera) => camera.uuid === selected),
    })
  }
  return options
})

// The select carries one line per option, so a camera the backend expects but discovery has not
// listed yet says so in its own name.
const cameraSelectItems = computed<BlueHeaderSelectorItem[]>(() =>
  cameraOptions.value.map((option) => ({
    name: option.label,
    value: option.uuid,
    icon: option.missing ? 'mdi-magnify-scan' : undefined,
    subtitle: option.missing ? 'Waiting for discovery' : option.uuid,
  })),
)

const selectedCameraLabel = computed(() => {
  if (!selectedCameraUUID.value) return ''
  const option = cameraOptions.value.find((item) => item.uuid === selectedCameraUUID.value)
  if (option) return option.label
  const camera = cameras.value.find((item) => item.uuid === selectedCameraUUID.value)
  return camera?.hostname ?? `Camera ${selectedCameraUUID.value.slice(0, 8)}`
})

const cameraOnline = computed(
  () => cameraConnectivity.value === 'online' || cameraConnectivity.value === 'unknown',
)

const baseControlsDisabled = computed(
  () =>
    !backendConnected.value
    || selectedCameraUUID.value == null
    || uiLoading.value
    || uiRebooting.value,
)

const cameraBackedControlsDisabled = computed(
  () => baseControlsDisabled.value || !cameraOnline.value,
)

const healthInputBase = computed(() => ({
  systemHealth: systemHealth.value,
  cameraUuid: selectedCameraUUID.value,
  cameraLabel: selectedCameraLabel.value,
  cameraConnectivity: cameraConnectivity.value,
  cameraStreamError: cameraStreamError.value,
  cameraOnvifAuthError: cameraOnvifAuthError.value,
  cameraExpectedMissing: expectedMissing.value.some(
    (camera) => camera.uuid === selectedCameraUUID.value,
  ),
  // Setup-oriented problems only after hardware setup; Welcome owns first-run UX.
  hardwareConfigured:
    selectedCameraUUID.value != null
    && actuatorsConfiguredByUuid.value[selectedCameraUUID.value] === true,
}))

const healthFlags = computed(() =>
  evaluateHealthFlags({
    ...healthInputBase.value,
    problemFirstSeen: healthDialog.value.problemFirstSeen,
    nowMs: healthProblemsNowMs.value,
  }),
)
const healthProblems = computed(() =>
  enrichHealthProblems(
    healthFlags.value.problems,
    healthDialog.value.problemFirstSeen,
    healthProblemsNowMs.value,
  ),
)
const healthView = computed(() =>
  healthDialogView(healthDialog.value, healthFlags.value.degraded),
)
const showHealthDialog = computed(() => healthView.value.showDialog)
/** A deliberate long-running action shows in the dialog until the user minimizes it. */
const busyDialogState = computed(() =>
  uiLoading.value && !busyMinimized.value
    ? { message: uiLoadingMessage.value, rebooting: uiRebooting.value }
    : null,
)
const showSystemStatusDialog = computed(
  () => connectionPhase.value != null || busyDialogState.value != null || showHealthDialog.value,
)
const dialogConnectionState = computed(() => connectionPhase.value ?? connectionState.value)
const welcomeOverlayUnblocked = computed(
  () =>
    backendConnected.value
    && !uiRebooting.value
    && !showSystemStatusDialog.value
    && errorDialogMessage.value == null,
)
const showDegradedBanner = computed(() => healthView.value.showDegradedBanner)
const showHealthChip = computed(() => healthFlags.value.degraded)
const showBusyChip = computed(() => uiLoading.value && busyMinimized.value)
const healthDialogAwaitingClose = computed(
  () => connectionState.value === 'connected' && healthView.value.awaitingClose,
)
const healthDialogRecoveryTitle = computed(() => healthView.value.recoveryTitle)
const healthDialogRecoveryMessage = computed(() => healthView.value.recoveryMessage)
const degradedBanner = computed(() => degradedBannerCopy(healthProblems.value))
const degradedBannerTitle = computed(() => degradedBanner.value.title)
const degradedBannerTooltip = computed(() => {
  const hint = showDegradedBanner.value
    ? 'Click to reopen system status.'
    : 'System status is open.'
  return `${degradedBanner.value.body} ${hint}`
})
const healthChipTooltip = computed(
  () => `${degradedBannerTitle.value} · ${degradedBannerTooltip.value}`,
)
const busyChipTooltip = computed(
  () => `${uiLoadingMessage.value} · Still running. Click to reopen system status.`,
)

const desiredCameraUuid = useRouteQuery<string | null>('uuid', null)

// Auto-pick over the options, not the MCM list, so a configured camera that discovery has
// not listed yet is still selected: it answers its own HTTP API regardless. Never replaces
// an existing selection, so a camera leaving discovery does not steal the user's choice.
watch(
  cameraOptions,
  (options) => {
    if (selectedCameraUUID.value || options.length === 0) return
    const desired = desiredCameraUuid.value
      ? options.find((option) => option.uuid === desiredCameraUuid.value)
      : null
    selectedCameraUUID.value = (desired ?? options[0]).uuid
  },
  { immediate: true },
)

const theme = ref<'light' | 'dark'>('dark')
const configMode = ref<'basic' | 'advanced'>('basic')
const cameraControls = ref<InstanceType<typeof BasicSettings> | null>(null)
const uiLoading = ref(false)
const uiLoadingMessage = ref('Applying settings…')
const uiRebooting = ref(false)
/** Local, per-episode: a new action always shows the dialog again. */
const busyMinimized = ref(false)
const onePushAwb = ref<OnePushAwbStatus | null>(null)
const errorDialogMessage = ref<string | null>(null)
const WARNING_TOAST_ICON = 'mdi-alert-circle-outline'
const RECOVERY_TOAST_ICON = 'mdi-check-circle-outline'
const warningToastMessage = ref<string | null>(null)
const warningToastIcon = ref(WARNING_TOAST_ICON)
const showStaleBundleBanner = ref(false)
const isCockpitMode = useRouteQuery<string, boolean>('cockpit_mode', 'false', {
  transform: {
    get: (v: string) => v === 'true',
    set: (v: boolean) => String(v),
  },
})

const configButtons = [
  {
    name: 'Basic',
    tooltip: 'Basic setup for the 4K Cam',
    onSelected: () => (configMode.value = 'basic'),
    preSelected: true,
  },
  {
    name: 'Advanced',
    tooltip: 'Advanced camera settings',
    onSelected: () => (configMode.value = 'advanced'),
  },
]

const applyCameraUi = (ui: CameraUiState) => {
  uiLoading.value = ui.loading
  // Keep the last message while fading out — clearing it to the default
  // ("Applying settings…") mid-transition looks like a glitch.
  if (ui.loading_message) {
    uiLoadingMessage.value = ui.loading_message
  }
  uiRebooting.value = ui.rebooting
  onePushAwb.value = ui.one_push_awb ?? null
  errorDialogMessage.value = ui.error_dialog ?? null
  warningToastIcon.value = WARNING_TOAST_ICON
  warningToastMessage.value = ui.warning_toast ?? null
  // A backend too old to send connectivity must not gray out every camera control:
  // treat an absent field as 'unknown', which keeps them usable.
  cameraConnectivity.value = ui.connectivity ?? 'unknown'
  cameraStreamError.value = ui.stream_error ?? null
  cameraOnvifAuthError.value = ui.onvif_auth_error ?? null
}

const uiByCamera = new Map<string, CameraUiState>()

const applyCameraState = (body: unknown) => {
  if (typeof body !== 'object' || body === null) return
  const data = body as CameraStateEvent
  if (typeof data.actuators_configured === 'boolean') {
    actuatorsConfiguredByUuid.value = {
      ...actuatorsConfiguredByUuid.value,
      [data.camera_uuid]: data.actuators_configured,
    }
  }
  if (data.ui) {
    uiByCamera.set(data.camera_uuid, data.ui)
    if (data.camera_uuid === selectedCameraUUID.value) {
      applyCameraUi(data.ui)
    }
  }
}

watch(selectedCameraUUID, (uuid, previousUuid) => {
  if (previousUuid) {
    backendClient.unsubscribeCamera(previousUuid)
  }

  if (!uuid) {
    uiLoading.value = false
    uiRebooting.value = false
    onePushAwb.value = null
    errorDialogMessage.value = null
    warningToastMessage.value = null
    cameraConnectivity.value = 'unknown'
    cameraStreamError.value = null
    cameraOnvifAuthError.value = null
    return
  }

  backendClient.subscribeCamera(uuid)

  const ui = uiByCamera.get(uuid)
  if (ui) {
    // Don't resurrect stale loading/rebooting overlays after unsubscribe.
    applyCameraUi({
      ...ui,
      loading: false,
      loading_message: undefined,
      rebooting: false,
    })
  } else {
    uiLoading.value = false
    uiRebooting.value = false
    onePushAwb.value = null
    errorDialogMessage.value = null
    warningToastMessage.value = null
    cameraConnectivity.value = 'unknown'
    cameraStreamError.value = null
    cameraOnvifAuthError.value = null
  }
})

// Remounted Basic/Advanced tabs start empty; re-subscribe so the backend re-pushes cache.
watch([configMode, tab], () => {
  if (selectedCameraUUID.value) {
    backendClient.refreshCameraSubscription()
  }
})

const dismissErrorDialog = () => {
  if (selectedCameraUUID.value) {
    backendClient.dismissUi(selectedCameraUUID.value, 'error_dialog')
  }
  errorDialogMessage.value = null
}

const reloadPage = (): void => {
  window.location.reload()
}

const applyCameraList = (data: unknown) => {
  try {
    const camerasData = validateCameras(data)
    cameras.value = camerasData

    const labels = { ...cameraLabelByUuid.value }
    for (const camera of camerasData) {
      labels[camera.uuid] = camera.hostname
    }
    for (const ghost of expectedMissing.value) {
      if (ghost.last_hostname) {
        labels[ghost.uuid] = ghost.last_hostname
      }
    }
    cameraLabelByUuid.value = labels
  } catch (error) {
    console.error('Error processing cameras:', error)
  }
}

const onCameraForgotten = (cameraUuid: string): void => {
  const labels = { ...cameraLabelByUuid.value }
  delete labels[cameraUuid]
  cameraLabelByUuid.value = labels

  if (selectedCameraUUID.value === cameraUuid) {
    selectedCameraUUID.value = null
  }

  if (!selectedCameraUUID.value && cameras.value.length > 0) {
    selectedCameraUUID.value = cameras.value[0].uuid
  }
}

const onSystemStatusDialogMinimize = (): void => {
  // One click must clear the dialog even when an action and a health problem overlap,
  // which happens when the reboot was started from inside this dialog.
  if (busyDialogState.value) {
    busyMinimized.value = true
  }
  if (showHealthDialog.value) {
    healthDialog.value = minimizeHealthDialog(healthDialog.value)
  }
}

const onHealthDialogClose = (): void => {
  healthDialog.value = closeHealthDialog()
}

const onDegradedBannerOpen = (): void => {
  healthDialog.value = reopenHealthDialog(healthDialog.value)
}

const onBusyChipOpen = (): void => {
  busyMinimized.value = false
}

const onHealthCameraForgotten = (cameraUuid: string): void => {
  healthDialog.value = noteForgetSuccess(healthDialog.value)
  onCameraForgotten(cameraUuid)
}

const onHealthGoToSetup = async (): Promise<void> => {
  healthDialog.value = minimizeHealthDialog(healthDialog.value)
  configMode.value = 'basic'
  await nextTick()
  cameraControls.value?.scrollToHardwareSetup()
}

const validateCameras = (data: unknown): Camera[] => {
  if (typeof data !== 'object' || data === null) {
    throw new Error('Expected a map of { uuid: camera }')
  }

  const cameras: Camera[] = []
  for (const [uuid, cameraData] of Object.entries(data)) {
    if (isCamera(cameraData)) {
      cameras.push({ ...cameraData, uuid })
    }
  }
  return cameras
}

const isCamera = (data: unknown): data is Omit<Camera, 'uuid'> => {
  if (typeof data !== 'object' || data === null) return false

  const camera = data as Record<string, unknown>

  const isStreamsValid =
    typeof camera.streams === 'object' &&
    camera.streams !== null &&
    Object.values(camera.streams).every((stream) => typeof stream === 'string')

  return typeof camera.hostname === 'string' && isStreamsValid
}

const updateLuaScript = (): void => {
  if (!selectedCameraUUID.value) return

  if (cameraControls.value) {
    cameraControls.value.updateLuaScript()
    return
  }

  runAutopilotControl('exportLuaScript', 'Failed to update Lua script')
}

const applyRecommendedCameraSettings = (): void => {
  if (cameraControls.value) {
    cameraControls.value.applyRecommendedCameraSettings()
    return
  }

  runCameraControl('setRecommendedCameraSettings', 'Failed to apply recommended camera settings')
}

const rebootCamera = (): void => {
  if (!selectedCameraUUID.value) return

  if (cameraControls.value) {
    cameraControls.value.rebootCamera()
    return
  }

  runCameraControl('restart', 'Failed to reboot camera')
}

const toolsMenuItems = computed<BlueMenuItem[]>(() => [
  {
    title: 'Update Lua script',
    icon: 'mdi-script-text-outline',
    disabled: !backendConnected.value,
    action: updateLuaScript,
  },
  {
    title: 'Apply recommended camera settings',
    icon: 'mdi-auto-fix',
    disabled: !backendConnected.value || cameraBackedControlsDisabled.value,
    action: applyRecommendedCameraSettings,
  },
  {
    title: 'Reboot camera',
    icon: 'mdi-restart',
    disabled: !backendConnected.value || cameraBackedControlsDisabled.value,
    action: rebootCamera,
  },
])

const runAutopilotControl = (action: string, errorMessage: string): void => {
  const cameraUuid = selectedCameraUUID.value
  if (!cameraUuid || uiLoading.value || uiRebooting.value) return

  backendClient
    .request('POST', '/autopilot/control', {
      camera_uuid: cameraUuid,
      action,
    })
    .then((data) => {
      if (selectedCameraUUID.value !== cameraUuid) return
      console.log(data)
    })
    .catch((error) => {
      if (selectedCameraUUID.value !== cameraUuid) return
      warningToastMessage.value = `${errorMessage}: ${formatRequestError(error)}`
    })
}

const runCameraControl = (action: string, errorMessage: string): void => {
  const cameraUuid = selectedCameraUUID.value
  if (!cameraUuid || uiLoading.value || uiRebooting.value) return

  backendClient
    .request('POST', '/camera/control', {
      camera_uuid: cameraUuid,
      action,
    })
    .then((data) => {
      if (selectedCameraUUID.value !== cameraUuid) return
      console.log(data)
    })
    .catch((error) => {
      if (selectedCameraUUID.value !== cameraUuid) return
      warningToastMessage.value = `${errorMessage}: ${formatRequestError(error)}`
    })
}

const applyConnectionStats = (body: unknown) => {
  if (typeof body !== 'object' || body === null) return
  connectionStats.value = body as ConnectionStats
}

const unsubscribeCameraList = backendClient.onEvent('camera/list', applyCameraList)
const unsubscribeCameraState = backendClient.onEvent('camera/state', applyCameraState)
const unsubscribeConnectionStats = backendClient.onEvent('connection/stats', applyConnectionStats)
const unsubscribeTransportError = backendClient.onTransportError((message) => {
  // Local-only toast; do not call dismissUi (that would clear shared backend warnings).
  warningToastMessage.value = message
})
const unsubscribeBackendVersionChanged = backendClient.onBackendVersionChanged(() => {
  showStaleBundleBanner.value = true
})
const unsubscribeConnectionState = backendClient.onConnectionState((state, previousState) => {
  if (state === 'disconnected' && previousState !== 'disconnected') {
    disconnectedSince.value = new Date()
    connectionStats.value = null
    uiByCamera.clear()
    actuatorsConfiguredByUuid.value = {}
    uiLoading.value = false
    uiRebooting.value = false
    errorDialogMessage.value = null
    warningToastMessage.value = null
    cameraConnectivity.value = 'unknown'
    cameraStreamError.value = null
    cameraOnvifAuthError.value = null
    healthDialog.value = healthDialogStateOnDisconnect(healthDialog.value)
  }
  if (state === 'connected') {
    disconnectedSince.value = null
    everConnected = true
    const remaining = MIN_CONNECTION_PHASE_MS - (Date.now() - connectionPhaseSince)
    if (remaining <= 0) {
      connectionPhase.value = null
    } else if (connectionPhaseTimer == null) {
      connectionPhaseTimer = setTimeout(() => {
        connectionPhase.value = null
        connectionPhaseTimer = null
      }, remaining)
    }
  } else {
    if (connectionPhaseTimer != null) {
      clearTimeout(connectionPhaseTimer)
      connectionPhaseTimer = null
    }
    if (connectionPhase.value == null) {
      connectionPhaseSince = Date.now()
      connectionPhaseEverConnected.value = everConnected
    }
    connectionPhase.value = state
  }
  connectionState.value = state
})

watch(
  healthInputBase,
  () => {
    const flags = healthFlags.value

    const before = healthDialog.value
    let next = before
    next = noteActiveProblems(next, flags.problems, Date.now())
    next = reduceHealthDialogOnProblems(next, flags.degraded)
    if (
      before.mode === 'minimized'
      && before.episodeDegraded
      && !flags.degraded
      && next.mode === 'hidden'
    ) {
      warningToastIcon.value = RECOVERY_TOAST_ICON
      warningToastMessage.value = recoveryWhileMinimizedToast(before)
    }
    if (next !== before) {
      healthDialog.value = next
    }
  },
)

watch(uiLoading, (loading) => {
  if (!loading) {
    busyMinimized.value = false
  }
})

const needsHealthProblemsNowTick = computed(() => healthFlags.value.degraded)

let healthProblemsNowInterval: ReturnType<typeof setInterval> | null = null

const stopHealthProblemsNowTick = (): void => {
  if (healthProblemsNowInterval == null) return
  clearInterval(healthProblemsNowInterval)
  healthProblemsNowInterval = null
}

const startHealthProblemsNowTick = (): void => {
  if (healthProblemsNowInterval != null) return
  healthProblemsNowMs.value = Date.now()
  healthProblemsNowInterval = setInterval(() => {
    healthProblemsNowMs.value = Date.now()
  }, 5000)
}

watch(needsHealthProblemsNowTick, (needs) => {
  if (needs) {
    startHealthProblemsNowTick()
  } else {
    stopHealthProblemsNowTick()
  }
}, { immediate: true })

onMounted(() => {
  backendClient.connect().catch((error) => {
    console.error('Error connecting to backend:', error)
  })
})

watch(warningToastMessage, (message, _previous, onCleanup) => {
  if (!message) {
    warningToastIcon.value = WARNING_TOAST_ICON
    return
  }

  const cameraUuid = selectedCameraUUID.value
  const cachedToast =
    cameraUuid != null ? uiByCamera.get(cameraUuid)?.warning_toast ?? null : null
  // Only auto-dismissUi when the displayed text is the backend-owned toast.
  const backendOwned = cachedToast != null && cachedToast === message
  const timeout = setTimeout(() => {
    if (backendOwned && cameraUuid) {
      backendClient.dismissUi(cameraUuid, 'warning_toast')
    }
    warningToastMessage.value = null
  }, 5000)

  onCleanup(() => clearTimeout(timeout))
})

onUnmounted(() => {
  stopHealthProblemsNowTick()
  if (connectionPhaseTimer != null) {
    clearTimeout(connectionPhaseTimer)
    connectionPhaseTimer = null
  }
  if (selectedCameraUUID.value) {
    backendClient.unsubscribeCamera(selectedCameraUUID.value)
  }
  unsubscribeCameraList()
  unsubscribeCameraState()
  unsubscribeConnectionStats()
  unsubscribeTransportError()
  unsubscribeBackendVersionChanged()
  unsubscribeConnectionState()
})


</script>
<style scoped>
.connection-status-wrap {
  display: inline-flex;
  align-items: center;
  padding: 0 12px;
  cursor: default;
}

.connection-status-icon {
  opacity: 0.55;
}

.connection-status-icon--spinning {
  animation: connection-status-spin 1.5s linear infinite;
}

@keyframes connection-status-spin {
  from {
    transform: rotate(0deg);
  }

  to {
    transform: rotate(360deg);
  }
}

/* pre keeps the newlines and the tab that connectionTooltip aligns its bandwidth rows on, which
   BlueTooltip's own wrapping would otherwise collapse back into one paragraph. */
.connection-tooltip :deep([role='tooltip']) {
  max-width: none;
  tab-size: 6;
  white-space: pre;
}

/* Cockpit's own glass menu, copied rather than read: it hands widget iframes nothing but datalake
   variables, and a vehicle-hosted widget is a different origin from Cockpit Standalone, so the
   user's live setting is out of reach. The fill is darker than Cockpit's own #63636354, which
   leaves the panel washed out over a bright scene. */
.transparent-card {
  background-color: #10101085;
  backdrop-filter: blur(25px);
  border: 1px solid rgba(255, 255, 255, 0.08);
  box-shadow: 0px 4px 4px 0px #00000033, 0px 8px 12px 6px #00000016;
}

.health-banners-sticky {
  position: sticky;
  top: 0;
  z-index: 3;
}

.health-focusable:focus-visible {
  outline: 2px solid #9ec9ef;
  outline-offset: 2px;
}
</style>
