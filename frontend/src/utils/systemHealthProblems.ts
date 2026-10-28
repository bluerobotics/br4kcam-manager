import type {
  AutopilotHealth,
  CameraConnectivity,
  ParameterDrift,
  SystemHealth,
} from '@/bindings/br4kcam_api'

export type HealthProblemKind =
  | 'mcm'
  | 'autopilot'
  | 'camera'
  | 'camera_stream'
  | 'camera_onvif_auth'
  | 'lua_scripting_disabled'
  | 'lua_script'
  | 'parameter_drift'

export type HealthProblem = {
  kind: HealthProblemKind
  severity: 'error' | 'warning' | 'info'
  title: string
  body: string
  detail?: string | null
  /** Live retry / recovery line under the problem. */
  progress?: string | null
  /** Operator-facing "since HH:MM (N min)" — set by enrichHealthProblems. */
  since?: string | null
  showForget?: boolean
  showGoToSetup?: boolean
  showUpdateLuaScript?: boolean
  /** Opens the per-parameter review where each drift can be accepted. */
  showReview?: boolean
  /** Every drifted parameter, for the review table. */
  drifts?: ParameterDrift[] | null
}

type KindSpec = {
  /**
   * The backend keeps retrying this kind on its own. The rest wait on the user,
   * so telling them a retry is in progress would be a lie.
   */
  selfRecovering: boolean
  recoveryTitle: string
  /** Recovery sentence fragment, joined by `recoveryMessage` without a period. */
  recovered: string
  /** Problem copy for kinds with a single shape; state-dispatched kinds omit it. */
  problem?: Omit<HealthProblem, 'kind' | 'body' | 'detail' | 'since'> & {
    /** Function form when the copy names the selected camera. */
    body: string | ((camera: string) => string)
  }
}

export type HealthProblemsInput = {
  systemHealth: SystemHealth | null
  cameraUuid: string | null
  cameraLabel: string
  cameraConnectivity: CameraConnectivity
  /**
   * True once this camera has had hardware setup applied. Setup-oriented
   * autopilot problems (SCR_ENABLE, lua script, parameter drift) are only
   * meaningful after that — before setup, Welcome alone is the right UX.
   */
  hardwareConfigured?: boolean
  /**
   * True when the selected camera has a fixed-focus lens: it only has tilt, so the
   * script problems (SCR_ENABLE, lua script) do not apply and outage copy names tilt only.
   */
  fixedFocus?: boolean
  /** True when the selected camera is configured but absent from MCM discovery. */
  cameraExpectedMissing?: boolean
  /** MCM video stream failure detail when the stream stayed broken. */
  cameraStreamError?: string | null
  /** MCM ONVIF authentication failure detail when login with expected credentials fails. */
  cameraOnvifAuthError?: string | null
  problemFirstSeen?: Partial<Record<HealthProblemKind, number>>
  nowMs?: number
}

/** Everything keyed purely by problem kind. Declaration order is recovery-message order. */
const KIND_TABLE = {
  mcm: {
    selfRecovering: true,
    recoveryTitle: 'Video service restored',
    recovered: 'BlueOS video service is back',
    problem: {
      severity: 'error',
      title: 'BlueOS video service unavailable',
      body: '4K Cam Manager cannot reach the BlueOS video service, so cameras cannot be discovered or controlled.',
    },
  },
  autopilot: {
    selfRecovering: true,
    recoveryTitle: 'Autopilot connection restored',
    recovered: 'Autopilot connection is working again',
  },
  camera: {
    selfRecovering: true,
    recoveryTitle: 'Camera connection restored',
    recovered: 'Camera connection is working again',
  },
  camera_stream: {
    selfRecovering: true,
    recoveryTitle: 'Camera video stream restored',
    recovered: 'Camera video stream is running again',
    problem: {
      severity: 'warning',
      title: 'Camera video stream not running',
      body: (camera: string) =>
        `${camera} is responding, but its video stream is not running. 4K Cam Manager is restarting it automatically.`,
      progress: 'Restarting the video stream…',
    },
  },
  camera_onvif_auth: {
    selfRecovering: false,
    recoveryTitle: 'Camera ONVIF login restored',
    recovered: 'ONVIF login succeeded and video is available again',
    problem: {
      severity: 'error',
      title: 'Camera ONVIF password does not match',
      body: (camera: string) =>
        `${camera} is responding to 4K Cam Manager, but the BlueOS video service cannot log in over ONVIF with the expected factory credentials (admin / blue), so there is no video. Restore the camera's ONVIF password to admin / blue.`,
      progress: 'Waiting for ONVIF login to succeed…',
    },
  },
  lua_scripting_disabled: {
    selfRecovering: false,
    recoveryTitle: 'Lua scripting enabled',
    recovered: 'Focus and zoom correlation is ready',
    problem: {
      severity: 'warning',
      title: 'Focus and zoom won\'t follow the autopilot',
      body: 'Lua scripting is disabled on the autopilot (SCR_ENABLE). Open hardware setup to re-apply it, then reboot the autopilot.',
      showGoToSetup: true,
    },
  },
  lua_script: {
    selfRecovering: false,
    recoveryTitle: 'Autopilot script updated',
    recovered: 'Autopilot script is up to date',
  },
  parameter_drift: {
    selfRecovering: false,
    recoveryTitle: 'Autopilot parameters restored',
    recovered: 'Autopilot parameters match the saved configuration again',
    problem: {
      severity: 'warning',
      title: 'Autopilot parameters no longer match saved setup',
      body: 'Another ground station, a parameter reset, or a loaded parameter file changed values on the autopilot. Autopilot-driven camera controls may not work until you re-apply the configuration.',
      showGoToSetup: true,
      showReview: true,
    },
  },
} satisfies Record<HealthProblemKind, KindSpec>

const ALL_KINDS = Object.keys(KIND_TABLE) as HealthProblemKind[]

export const SELF_RECOVERING_KINDS: readonly HealthProblemKind[] = ALL_KINDS.filter(
  (kind) => KIND_TABLE[kind].selfRecovering,
)

const AUTOPILOT_PROBLEM_STATES: ReadonlySet<AutopilotHealth> = new Set([
  'endpoint_setup_failed',
  'mavlink_down',
  'autopilot_offline',
  'unresponsive',
])

const SEVERITY_RANK: Record<HealthProblem['severity'], number> = {
  error: 0,
  warning: 1,
  info: 2,
}

/** MCM re-probes every second while down. */
const MCM_PROBE_INTERVAL_MS = 1000

export function collectHealthProblems(input: HealthProblemsInput): HealthProblem[] {
  const problems: HealthProblem[] = []
  const health = input.systemHealth
  const mcmDown = health?.mcm === 'down'

  if (mcmDown) {
    problems.push({
      kind: 'mcm',
      ...KIND_TABLE.mcm.problem,
      detail: health.mcm_detail ?? null,
      progress: mcmProgressLine(input.problemFirstSeen?.mcm, input.nowMs),
    })
  }

  if (health) {
    const autopilot = autopilotProblem(health, input.fixedFocus === true)
    if (autopilot) problems.push(autopilot)

    // SCR_ENABLE / lua script / saved-param drift are fixed by hardware setup.
    // Before any setup, Welcome covers that — don't open System status over it.
    if (input.hardwareConfigured === true) {
      const drift = parameterDriftProblem(health)
      if (drift) problems.push(drift)

      // A fixed-focus lens has no focus or zoom, so it needs no script.
      if (input.fixedFocus !== true) {
        if (health.autopilot === 'online' && health.lua_scripting_disabled) {
          problems.push({
            kind: 'lua_scripting_disabled',
            ...KIND_TABLE.lua_scripting_disabled.problem,
          })
        }

        const script = luaScriptProblem(health, input.cameraUuid)
        if (script) problems.push(script)
      }
    }
  }

  if (input.cameraUuid) {
    if (mcmDown) {
      problems.push({
        kind: 'camera',
        severity: 'info',
        title: 'Camera status unknown',
        body: `Cannot check ${input.cameraLabel} while the BlueOS video service is unavailable.`,
      })
    } else {
      const camera = cameraProblem(
        input.cameraLabel,
        input.cameraConnectivity,
        input.cameraExpectedMissing === true,
      )
      if (camera) problems.push(camera)

      if (input.cameraStreamError && input.cameraConnectivity === 'online') {
        const copy = KIND_TABLE.camera_stream.problem
        problems.push({
          kind: 'camera_stream',
          ...copy,
          body: copy.body(input.cameraLabel),
          detail: input.cameraStreamError,
        })
      }

      if (input.cameraOnvifAuthError && input.cameraConnectivity === 'online') {
        const copy = KIND_TABLE.camera_onvif_auth.problem
        problems.push({
          kind: 'camera_onvif_auth',
          ...copy,
          body: copy.body(input.cameraLabel),
          detail: input.cameraOnvifAuthError,
        })
      }
    }
  }

  return problems
}

export function sortProblemsBySeverity(problems: HealthProblem[]): HealthProblem[] {
  return [...problems].sort(
    (left, right) => SEVERITY_RANK[left.severity] - SEVERITY_RANK[right.severity],
  )
}

/** True when every error/warning problem is a kind the backend retries on its own. */
export function allNotableProblemsSelfRecover(problems: HealthProblem[]): boolean {
  const notable = problems.filter(
    (problem) => problem.severity === 'error' || problem.severity === 'warning',
  )
  if (notable.length === 0) return false
  return notable.every((problem) => SELF_RECOVERING_KINDS.includes(problem.kind))
}

export function problemsSummary(problems: HealthProblem[]): string | null {
  const notable = problems.filter((problem) => problem.severity !== 'info')
  if (notable.length <= 1) return null
  const errors = notable.filter((problem) => problem.severity === 'error').length
  const warnings = notable.filter((problem) => problem.severity === 'warning').length
  const parts: string[] = []
  if (errors > 0) parts.push(`${errors} critical`)
  if (warnings > 0) parts.push(`${warnings} warning${warnings === 1 ? '' : 's'}`)
  return `${notable.length} issues need attention (${parts.join(', ')})`
}

function formatDurationShort(ms: number): string {
  const secs = Math.max(1, Math.round(ms / 1000))
  if (secs < 60) return `${secs} sec`
  const mins = Math.floor(secs / 60)
  const rem = secs % 60
  if (rem === 0) return `${mins} min`
  return `${mins} min ${rem} sec`
}

export function formatProblemSince(firstSeenMs: number, nowMs: number): string {
  const date = new Date(firstSeenMs)
  const time = date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
  const elapsedSec = Math.max(1, Math.round((nowMs - firstSeenMs) / 1000))
  if (elapsedSec < 60) {
    return `since ${time} (${elapsedSec} sec)`
  }
  const elapsedMin = Math.round(elapsedSec / 60)
  return `since ${time} (${elapsedMin} min)`
}

export function recoveryTitle(
  resolvedKinds: readonly HealthProblemKind[],
  forgetSuccess: boolean,
): string {
  if (forgetSuccess) return 'Camera removed from setup'
  const kinds = resolvedKinds.filter((kind) => kind !== 'lua_scripting_disabled')
  if (kinds.length === 0) {
    return resolvedKinds.includes('lua_scripting_disabled')
      ? KIND_TABLE.lua_scripting_disabled.recoveryTitle
      : 'All clear'
  }
  if (kinds.length === 1) return KIND_TABLE[kinds[0]].recoveryTitle
  return 'Issues resolved'
}

export function recoveryMessage(
  resolvedKinds: readonly HealthProblemKind[],
  mcmOutageMsPeak: number,
  forgetSuccess: boolean,
  withCloseHint = true,
): string {
  if (forgetSuccess) {
    return withCloseHint
      ? 'This camera was removed from your saved setup. Click Close to continue.'
      : 'This camera was removed from your saved setup.'
  }

  const parts = ALL_KINDS.filter((kind) => resolvedKinds.includes(kind)).map((kind) =>
    kind === 'mcm' && mcmOutageMsPeak > 0
      ? `${KIND_TABLE.mcm.recovered} (after about ${formatDurationShort(mcmOutageMsPeak)})`
      : KIND_TABLE[kind].recovered,
  )

  if (parts.length === 0) {
    return withCloseHint ? 'All clear. Click Close to continue.' : 'All clear.'
  }
  const body = parts.join('. ')
  return withCloseHint ? `${body}. Click Close to continue.` : `${body}.`
}

function mcmProgressLine(mcmSinceMs: number | undefined, nowMs: number | undefined): string {
  const elapsedMs =
    mcmSinceMs != null && nowMs != null ? Math.max(nowMs - mcmSinceMs, 1000) : 1000
  const elapsed = formatDurationShort(elapsedMs)
  const cadence = MCM_PROBE_INTERVAL_MS / 1000
  return `Retrying every ${cadence} sec (about ${elapsed} so far)…`
}

function autopilotProblem(health: SystemHealth, fixedFocus: boolean): HealthProblem | null {
  const actuatorsAre = fixedFocus ? 'Tilt is' : 'Focus, zoom and tilt are'
  const actuatorsMay = fixedFocus ? 'Tilt may' : 'Focus, zoom and tilt may'

  if (health.autopilot === 'unknown') {
    return {
      kind: 'autopilot',
      severity: 'info',
      title: 'Checking autopilot connection',
      body: '4K Cam Manager is still determining whether the MAVLink link and flight controller are available. This is normal while BlueOS is starting.',
      detail: health.autopilot_detail ?? null,
      progress: 'Waiting for autopilot status from BlueOS…',
    }
  }

  if (!AUTOPILOT_PROBLEM_STATES.has(health.autopilot)) return null

  switch (health.autopilot) {
    case 'endpoint_setup_failed':
      return {
        kind: 'autopilot',
        severity: 'error',
        title: 'Autopilot service unavailable',
        body: `4K Cam Manager cannot configure its MAVLink connection through BlueOS. ${actuatorsAre} unavailable. Check that BlueOS is fully started; if this persists, restart BlueOS.`,
        detail: health.autopilot_detail ?? null,
        progress: 'Waiting for BlueOS to finish starting…',
      }
    case 'mavlink_down': {
      const neverReceived = health.diagnostics.last_frame_age_ms == null
      return {
        kind: 'autopilot',
        severity: 'error',
        title: 'MAVLink connection unavailable',
        body: neverReceived
          ? `4K Cam Manager has not received any MAVLink data from BlueOS yet. ${actuatorsAre} unavailable. Check that BlueOS is fully started, then verify MAVLink endpoints are configured.`
          : `No MAVLink data is reaching 4K Cam Manager. ${actuatorsAre} unavailable. Check BlueOS MAVLink endpoints, then restart BlueOS if needed.`,
        detail: health.autopilot_detail ?? null,
        progress: neverReceived
          ? 'Waiting for the first MAVLink data from BlueOS…'
          : 'Waiting for MAVLink data from BlueOS…',
      }
    }
    case 'autopilot_offline':
      return {
        kind: 'autopilot',
        severity: 'error',
        title: 'Autopilot unavailable',
        body: 'MAVLink is working but the autopilot is not sending heartbeats. Check that the flight controller is powered and connected, then reboot it if needed.',
        detail: health.autopilot_detail ?? null,
        progress: 'Waiting for the flight controller to power on…',
      }
    case 'unresponsive':
      return {
        kind: 'autopilot',
        severity: 'error',
        title: 'Autopilot not responding',
        body: `The autopilot is heartbeating but not answering our requests. ${actuatorsMay} not move. Reboot the autopilot; if it keeps happening, contact support.`,
        detail: health.autopilot_detail ?? null,
        progress: 'Retrying communication with the autopilot…',
      }
    default:
      return null
  }
}

function parameterDriftProblem(health: SystemHealth): HealthProblem | null {
  // Re-applying configuration writes parameters over MAVLink — nothing to do until the
  // autopilot path is usable, and a hard outage already explains why focus/zoom stopped.
  if (health.autopilot !== 'online') return null

  const drifts = health.parameter_drifts
  if (!drifts?.length) return null

  return {
    kind: 'parameter_drift',
    ...KIND_TABLE.parameter_drift.problem,
    drifts,
  }
}

function luaScriptProblem(
  health: SystemHealth,
  cameraUuid: string | null,
): HealthProblem | null {
  // Applying the script reloads scripting over MAVLink, so there is nothing the operator
  // can do about it until the autopilot is back.
  if (health.autopilot !== 'online') return null

  const canUpdate = cameraUuid != null

  switch (health.lua_script) {
    case 'missing':
      return {
        kind: 'lua_script',
        severity: 'warning',
        title: 'Autopilot script is not installed',
        body: canUpdate
          ? 'The script that drives focus and zoom from the autopilot is not on the flight controller, so focus and zoom will not follow the camera. Install it now — the autopilot may reboot.'
          : 'The script that drives focus and zoom from the autopilot is not on the flight controller, so focus and zoom will not follow the camera. A camera must be discovered before the script can be installed.',
        showUpdateLuaScript: canUpdate,
      }
    case 'outdated':
      return {
        kind: 'lua_script',
        severity: 'warning',
        title: 'Autopilot script is out of date',
        body: canUpdate
          ? 'The script installed on the flight controller is not the one this version of 4K Cam Manager expects, so focus and zoom may misbehave. Update it — the autopilot may reboot.'
          : 'The script installed on the flight controller is not the one this version of 4K Cam Manager expects, so focus and zoom may misbehave. A camera must be discovered before the script can be updated.',
        showUpdateLuaScript: canUpdate,
      }
    case 'failing':
      return {
        kind: 'lua_script',
        severity: 'warning',
        title: 'Autopilot script is failing',
        body: canUpdate
          ? 'The right script is installed, but the autopilot reports it is erroring, so focus and zoom will not follow the camera. Re-installing it often clears this — the autopilot may reboot.'
          : 'The right script is installed, but the autopilot reports it is erroring, so focus and zoom will not follow the camera. A camera must be discovered before the script can be re-installed.',
        detail: health.lua_script_detail ?? null,
        showUpdateLuaScript: canUpdate,
      }
    default:
      return null
  }
}

function cameraProblem(
  label: string,
  connectivity: CameraConnectivity,
  expectedMissing: boolean,
): HealthProblem | null {
  switch (connectivity) {
    case 'missing':
      return {
        kind: 'camera',
        severity: 'info',
        title: 'Waiting for camera discovery',
        body: `${label} is configured and the BlueOS video service is still looking for it. This is normal after the video service starts — discovery can take a short while.`,
        showForget: expectedMissing,
        progress: 'Waiting for ONVIF discovery…',
      }
    case 'unreachable':
      return {
        kind: 'camera',
        severity: 'error',
        title: 'Camera unavailable',
        body: `4K Cam Manager can no longer reach ${label}. Check the Ethernet cable between the camera and the vehicle, and confirm the camera has power.`,
        showForget: expectedMissing,
        progress: 'Waiting for the camera to come back online…',
      }
    case 'unresponsive':
      return {
        kind: 'camera',
        severity: 'error',
        title: 'Camera not responding',
        body: `${label} is on the network but is not answering. Power-cycle the camera. If it keeps happening, contact support.`,
        progress: 'Waiting for the camera to respond…',
      }
    default:
      return null
  }
}
