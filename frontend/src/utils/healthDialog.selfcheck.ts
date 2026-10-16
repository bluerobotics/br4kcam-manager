import type { CameraConnectivity, SystemHealth } from '@/bindings/br4kcam_api'
import {
  closeHealthDialog,
  degradedBannerCopy,
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
} from './healthDialogState'
import {
  allNotableProblemsSelfRecover,
  collectHealthProblems,
  formatProblemSince,
  recoveryMessage,
  recoveryTitle,
  SELF_RECOVERING_KINDS,
  type HealthProblem,
  type HealthProblemKind,
  type HealthProblemsInput,
} from './systemHealthProblems'

type ProblemCase = {
  name: string
  input: HealthProblemsInput
  /** Full ordered problem titles, when the whole set matters (ranking/suppression). */
  titles?: string[]
  kinds?: HealthProblemKind[]
  degraded?: boolean
  check?: (problems: HealthProblem[]) => boolean
}

/** `[resolved kinds, title, message without the Close hint]`. */
type RecoveryCase = [HealthProblemKind[], string, string]

const DIAGNOSTICS = {
  mavlink_reconnects: 0,
  mavlink_frames_lagged: 0,
  state_events_lagged: 0,
  mcm_consecutive_failures: 0,
  script_reloads: 0,
  backend_version: 'test',
}

const MCM_DOWN: SystemHealth = health({
  mcm: 'down',
  mcm_detail: 'connection refused',
  cameras_discovered: 0,
  diagnostics: { ...DIAGNOSTICS, mcm_consecutive_failures: 3 },
})

const CAMERA = { cameraUuid: 'cam', cameraLabel: '4K Cam', hardwareConfigured: true }
const ONVIF_ERROR = 'ONVIF authentication failed: wrong password'

const MCM_TITLE = 'BlueOS video service unavailable'
const CAMERA_UNKNOWN_TITLE = 'Camera status unknown'
const CAMERA_UNREACHABLE_TITLE = 'Camera unavailable'
const ONVIF_TITLE = 'Camera ONVIF password does not match'
const STREAM_TITLE = 'Camera video stream not running'

/** Every recovery copy row uses the same peak so one round trip covers the mcm duration too. */
const MCM_PEAK_MS = 5000

export function runHealthDialogSelfCheck(): void {
  for (const { name, input, titles, kinds, degraded, check } of problemCases()) {
    const problems = collectHealthProblems(input)
    if (titles) assertList(problems.map((problem) => problem.title), titles, `${name}: titles`)
    if (kinds) assertList(problems.map((problem) => problem.kind), kinds, `${name}: kinds`)
    if (degraded != null) {
      assert(evaluateHealthFlags(input).degraded === degraded, `${name}: degraded must be ${degraded}`)
    }
    if (check) assert(check(problems), `${name}: check failed`)
  }

  for (const [kinds, title, message] of recoveryCases()) {
    const label = kinds.join('+') || 'none'
    assert(recoveryTitle(kinds, false) === title, `recovery title ${label}: ${recoveryTitle(kinds, false)}`)
    const line = recoveryMessage(kinds, MCM_PEAK_MS, false)
    assert(line === `${message} Click Close to continue.`, `recovery message ${label}: ${line}`)
  }

  for (const [name, input, expected] of selfRecoveryCases()) {
    const all = allNotableProblemsSelfRecover(collectHealthProblems(input))
    assert(all === expected, `${name}: allNotableProblemsSelfRecover must be ${expected}`)
  }
  assert(!SELF_RECOVERING_KINDS.includes('lua_script'), 'lua_script must not be self-recovering')

  for (const [name, run] of stateCases()) {
    try {
      run()
    } catch (error) {
      throw new Error(`healthDialog: ${name} -> ${(error as Error).message}`)
    }
  }

  console.log('healthDialog self-check ok')
}

function health(overrides: Partial<SystemHealth> = {}): SystemHealth {
  return {
    mcm: 'online',
    cameras_discovered: 1,
    expected_missing: [],
    autopilot: 'online',
    lua_scripting_disabled: false,
    lua_script: 'ok',
    diagnostics: DIAGNOSTICS,
    ...overrides,
  }
}

function baseInput(overrides: Partial<HealthProblemsInput> = {}): HealthProblemsInput {
  return {
    systemHealth: MCM_DOWN,
    cameraUuid: null,
    cameraLabel: '',
    cameraConnectivity: 'unknown',
    ...overrides,
  }
}

/** Healthy backend with the selected camera in a given connectivity state. */
function cameraInput(
  cameraConnectivity: CameraConnectivity,
  overrides: Partial<HealthProblemsInput> = {},
): HealthProblemsInput {
  return baseInput({ ...CAMERA, systemHealth: health(), cameraConnectivity, ...overrides })
}

function problemCases(): ProblemCase[] {
  const driftHealth = health({
    parameter_drifts: [
      { name: 'SERVO1_FUNCTION', expected: 33, actual: 0, adoptable: false },
      { name: 'SERVO2_FUNCTION', expected: 34, actual: 0, adoptable: false },
    ],
  })
  const luaScript = (problems: HealthProblem[]) =>
    problems.find((problem) => problem.kind === 'lua_script')
  const drift = (problems: HealthProblem[]) =>
    problems.find((problem) => problem.kind === 'parameter_drift')

  return [
    { name: 'mcm down alone', input: baseInput(), titles: [MCM_TITLE], degraded: true },
    { name: 'mcm down hides camera cable blame',
      input: baseInput({ ...CAMERA, cameraConnectivity: 'unreachable' }),
      titles: [MCM_TITLE, CAMERA_UNKNOWN_TITLE] },
    { name: 'mcm down suppresses onvif and stream blame',
      input: baseInput({
        ...CAMERA,
        cameraConnectivity: 'online',
        cameraOnvifAuthError: ONVIF_ERROR,
        cameraStreamError: 'pipeline error',
      }),
      titles: [MCM_TITLE, CAMERA_UNKNOWN_TITLE] },
    { name: 'autopilot syncing is not a modal problem',
      input: baseInput({ systemHealth: health({ autopilot: 'syncing' }) }),
      titles: [],
      degraded: false },
    { name: 'mavlink_down without frame age',
      input: baseInput({
        systemHealth: health({ autopilot: 'mavlink_down', autopilot_detail: 'MAVLink component unavailable' }),
      }),
      kinds: ['autopilot'],
      degraded: true },
    { name: 'autopilot unknown during boot',
      input: baseInput({ systemHealth: health({ autopilot: 'unknown' }) }),
      kinds: ['autopilot'],
      degraded: false },
    ...(['missing', 'outdated', 'failing'] as const).flatMap((lua_script): ProblemCase[] => [
      { name: `lua_script=${lua_script} with camera offers the fix`,
        input: baseInput({ ...CAMERA, systemHealth: health({ lua_script }) }),
        check: (problems) => luaScript(problems)?.showUpdateLuaScript === true },
      { name: `lua_script=${lua_script} without camera hides the fix`,
        input: baseInput({ hardwareConfigured: true, systemHealth: health({ lua_script }) }),
        check: (problems) =>
          !luaScript(problems)?.showUpdateLuaScript
          && luaScript(problems)?.body.includes('discovered') === true },
      { name: `lua_script=${lua_script} hidden while autopilot is down`,
        input: baseInput({ ...CAMERA, systemHealth: health({ lua_script, autopilot: 'mavlink_down' }) }),
        check: (problems) => luaScript(problems) == null },
    ]),
    { name: 'lua_script unknown yields no problem',
      input: baseInput({ ...CAMERA, systemHealth: health({ lua_script: 'unknown' }) }),
      check: (problems) => luaScript(problems) == null },
    { name: 'lua_scripting_disabled after hardware setup',
      input: baseInput({
        ...CAMERA,
        systemHealth: health({ lua_scripting_disabled: true }),
      }),
      kinds: ['lua_scripting_disabled'],
      degraded: true },
    { name: 'lua_scripting_disabled ignored before hardware setup',
      input: baseInput({
        ...CAMERA,
        hardwareConfigured: false,
        systemHealth: health({ lua_scripting_disabled: true }),
      }),
      titles: [],
      degraded: false },
    { name: 'lua_script ignored before hardware setup',
      input: baseInput({
        ...CAMERA,
        hardwareConfigured: false,
        systemHealth: health({ lua_script: 'missing' }),
      }),
      titles: [],
      degraded: false },
    { name: 'parameter drift ignored before hardware setup',
      input: baseInput({
        ...CAMERA,
        hardwareConfigured: false,
        systemHealth: driftHealth,
      }),
      titles: [],
      degraded: false },
    { name: 'onvif auth while camera online',
      input: cameraInput('online', { cameraOnvifAuthError: ONVIF_ERROR }),
      titles: [ONVIF_TITLE],
      kinds: ['camera_onvif_auth'] },
    { name: 'onvif auth hidden while camera unreachable',
      input: cameraInput('unreachable', { cameraOnvifAuthError: ONVIF_ERROR }),
      titles: [CAMERA_UNREACHABLE_TITLE] },
    { name: 'stream error while camera online',
      input: cameraInput('online', { cameraStreamError: 'pipeline error' }),
      titles: [STREAM_TITLE],
      kinds: ['camera_stream'] },
    { name: 'stream error hidden while camera unreachable',
      input: cameraInput('unreachable', { cameraStreamError: 'MCM stream state: stopped' }),
      titles: [CAMERA_UNREACHABLE_TITLE] },
    { name: 'parameter drift',
      input: baseInput({ systemHealth: driftHealth, hardwareConfigured: true }),
      degraded: true,
      check: (problems) => {
        const problem = drift(problems)
        return problem?.showGoToSetup === true && problem.showReview === true
          && problem.severity === 'warning' && problem.detail == null
          && problem.drifts?.[0]?.name === 'SERVO1_FUNCTION'
          && problem.body.includes('may not work')
      } },
    { name: 'parameter drift lists every difference for review',
      input: baseInput({
        hardwareConfigured: true,
        systemHealth: health({
          parameter_drifts: Array.from({ length: 5 }, (_, index) => ({
            name: `SERVO${index + 1}_FUNCTION`,
            expected: 33 + index,
            actual: 0,
            adoptable: false,
          })),
        }),
      }),
      check: (problems) => drift(problems)?.drifts?.length === 5 },
    { name: 'parameter drift hidden while autopilot is down',
      input: baseInput({
        hardwareConfigured: true,
        systemHealth: health({ ...driftHealth, autopilot: 'mavlink_down' }),
      }),
      check: (problems) => drift(problems) == null },
    { name: 'mcm progress uses the first-seen clock',
      input: baseInput({ problemFirstSeen: { mcm: 1_000_000 }, nowMs: 1_045_000 }),
      check: (problems) => problems[0]?.progress === 'Retrying every 1 sec (about 45 sec so far)…' },
  ]
}

function recoveryCases(): RecoveryCase[] {
  return [
    [[], 'All clear', 'All clear.'],
    [['mcm'], 'Video service restored', 'BlueOS video service is back (after about 5 sec).'],
    [['autopilot'], 'Autopilot connection restored', 'Autopilot connection is working again.'],
    [['camera'], 'Camera connection restored', 'Camera connection is working again.'],
    [['camera_stream'], 'Camera video stream restored', 'Camera video stream is running again.'],
    [['camera_onvif_auth'], 'Camera ONVIF login restored', 'ONVIF login succeeded and video is available again.'],
    [['lua_script'], 'Autopilot script updated', 'Autopilot script is up to date.'],
    [['lua_scripting_disabled'], 'Lua scripting enabled', 'Focus and zoom correlation is ready.'],
    [['parameter_drift'], 'Autopilot parameters restored', 'Autopilot parameters match the saved configuration again.'],
    [['mcm', 'camera'], 'Issues resolved', 'BlueOS video service is back (after about 5 sec). Camera connection is working again.'],
  ]
}

/** `[name, input, every notable problem self-recovers]`. */
function selfRecoveryCases(): [string, HealthProblemsInput, boolean][] {
  return [
    ['lone mcm outage', baseInput(), true],
    ['lone lua_script', baseInput({ ...CAMERA, systemHealth: health({ lua_script: 'missing' }) }), false],
    ['mcm + lua_script', baseInput({ ...CAMERA, systemHealth: health({ ...MCM_DOWN, lua_script: 'missing' }) }), false],
    ['mcm + camera unreachable', baseInput({ ...CAMERA, cameraConnectivity: 'unreachable' }), true],
  ]
}

function stateCases(): [name: string, run: () => void][] {
  const degradedOpen = () => reduceHealthDialogOnProblems(initialHealthDialogState(), true)
  const mcmProblems = collectHealthProblems(baseInput())

  return [
    ['auto-open on degraded problems, idempotent while open', () => {
      const state = degradedOpen()
      assert(state.mode === 'open', 'must auto-open on degraded problems')
      assert(reduceHealthDialogOnProblems(state, true) === state, 'reduce while open must be idempotent')
    }],
    ['minimize shows the banner only, reopen shows the dialog', () => {
      const minimized = minimizeHealthDialog(degradedOpen())
      const banner = healthDialogView(minimized, true)
      assert(!banner.showDialog && banner.showDegradedBanner, 'minimize must show banner only')
      const reopened = healthDialogView(reopenHealthDialog(minimized), true)
      assert(reopened.showDialog && !reopened.showDegradedBanner, 'reopen must show dialog')
    }],
    ['recovery while open stays sticky until Close', () => {
      let state = noteActiveProblems(degradedOpen(), mcmProblems, 5000)
      state = reduceHealthDialogOnProblems(noteActiveProblems(state, mcmProblems, 10_000), false)
      const view = healthDialogView(state, false)
      assert(view.showDialog && view.awaitingClose, 'recovery must keep dialog open awaiting close')
      const closed = closeHealthDialog()
      assert(closed.mode === 'hidden' && closed.mcmOutageMsPeak === 0, 'close must reset')
    }],
    ['recovery while minimized clears through the toast', () => {
      const state = reduceHealthDialogOnProblems(minimizeHealthDialog(degradedOpen()), false)
      const view = healthDialogView(state, false)
      assert(!view.showDialog && !view.showDegradedBanner && state.mode === 'hidden', 'recovery while minimized must clear')
      const toast = recoveryWhileMinimizedToast({
        ...initialHealthDialogState(),
        episodeKinds: ['mcm'],
        mcmOutageMsPeak: MCM_PEAK_MS,
      })
      assert(toast === 'BlueOS video service is back (after about 5 sec).', `toast must omit Close hint: ${toast}`)
      assert(healthDialogView(minimizeHealthDialog(degradedOpen()), true).showDegradedBanner, 'minimized+degraded must show banner')
    }],
    ['open without degraded or recovery does not flash an empty dialog', () => {
      const view = healthDialogView(degradedOpen(), false)
      assert(!view.showDialog && !view.awaitingClose, 'healthy open-without-recovery must not show')
    }],
    ['healthy boot never opens the modal', () => {
      let state = initialHealthDialogState()
      for (const autopilot of ['unknown', 'syncing', 'online'] as const) {
        const booting = health({ autopilot, cameras_discovered: autopilot === 'online' ? 1 : 0 })
        const { degraded } = evaluateHealthFlags(baseInput({ systemHealth: booting }))
        assert(!degraded, `boot step autopilot=${autopilot} must not be degraded`)
        state = reduceHealthDialogOnProblems(state, degraded)
        assert(state.mode === 'hidden' && !state.awaitingClose, `boot step autopilot=${autopilot} must not open modal`)
      }
    }],
    ['disconnect preserves minimized and resets while open', () => {
      const minimized = healthDialogStateOnDisconnect(minimizeHealthDialog(degradedOpen()))
      assert(minimized.mode === 'minimized', 'disconnect must preserve minimized mode')
      const opened = healthDialogStateOnDisconnect(reopenHealthDialog(degradedOpen()))
      assert(opened.mode === 'hidden', 'disconnect while open must reset to hidden')
    }],
    ['forget waits for the problems to clear', () => {
      let state = noteForgetSuccess(noteActiveProblems(degradedOpen(), mcmProblems, 5000))
      assert(!state.awaitingClose, 'forget during MCM outage must not switch to recovery view')
      assert(state.forgetSuccess, 'forget must record forgetSuccess')
      state = reduceHealthDialogOnProblems(state, false)
      assert(state.awaitingClose, 'forget recovery must await close after problems clear')
    }],
    ['new problem during recovery resets episode tracking', () => {
      let state = noteActiveProblems(degradedOpen(), mcmProblems, 5000)
      state = reduceHealthDialogOnProblems(noteActiveProblems(state, mcmProblems, 60_000), false)
      state = reduceHealthDialogOnProblems(state, true)
      assert(state.episodeKinds.length === 0 && state.mcmOutageMsPeak === 0, 'episode tracking must reset')
    }],
    ['reappeared problem gets a fresh first-seen', () => {
      const autopilotError: HealthProblem = {
        kind: 'autopilot',
        severity: 'error',
        title: 'MAVLink connection unavailable',
        body: 'test',
      }
      let state = noteActiveProblems(initialHealthDialogState(), [autopilotError], 1_000_000)
      state = noteActiveProblems(state, [], 1_060_000)
      state = noteActiveProblems(state, [autopilotError], 1_120_000)
      assert(state.problemFirstSeen.autopilot === 1_120_000, `fresh first-seen: ${state.problemFirstSeen.autopilot}`)
      const since = formatProblemSince(1_000_000, 1_010_000)
      assert(since.includes('(10 sec)'), `formatProblemSince must show seconds below 1 min: ${since}`)
    }],
    ['degraded banner summarizes multiple issues', () => {
      const banner = degradedBannerCopy([
        { kind: 'mcm', severity: 'error', title: 'MCM down', body: 'video service down' },
        { kind: 'autopilot', severity: 'warning', title: 'AP warn', body: 'autopilot warn' },
      ])
      assert(banner.title.includes('2 issues'), `banner must summarize multiple issues: ${banner.title}`)
      assert(banner.body.includes('MCM down') && banner.body.includes('AP warn'), `banner must list titles: ${banner.body}`)
    }],
  ]
}

function assertList(actual: string[], expected: string[], label: string): void {
  assert(
    actual.length === expected.length && actual.every((value, index) => value === expected[index]),
    `${label}: expected [${expected.join(' | ')}], got [${actual.join(' | ')}]`,
  )
}

function assert(condition: unknown, message: string): void {
  if (!condition) throw new Error(`healthDialog: ${message}`)
}
