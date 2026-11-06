import type {
  ConnectionStats,
  UiDismissField,
  WsClientMessage,
  WsEvent,
  WsRequest,
  WsResponse,
} from '@/bindings/br4kcam_api'

type RequestMethod = 'GET' | 'POST' | 'PATCH'

type PendingRequest = {
  method: RequestMethod
  path: string
  body?: unknown
  resolve: (body: unknown) => void
  reject: (error: Error) => void
  timeoutId: ReturnType<typeof setTimeout>
  sent: boolean
}

export type BackendRequestError = Error & {
  response?: {
    status?: number
    statusText?: string
    data?: unknown
  }
}

export type ConnectionState = 'disconnected' | 'connecting' | 'connected'

export type { ConnectionStats }

export type BackendVersionObservation = {
  initial: string | null
  changed: boolean
}

export function observeBackendVersion(
  state: BackendVersionObservation,
  backendVersion: string | undefined,
): void {
  if (!backendVersion || state.changed) return
  if (state.initial === null) {
    state.initial = backendVersion
    return
  }
  if (backendVersion !== state.initial) {
    state.changed = true
  }
}

// Above reboot wait budget (150s) and matching camera_ui::REBOOT_GRACE (180s).
const REQUEST_TIMEOUT_MS = 180_000
const REQUEST_MAX_ATTEMPTS = 3
const RECONNECT_BASE_MS = 1_000
/** Give up on a WebSocket stuck in CONNECTING and let backoff schedule the next try. */
const HANDSHAKE_TIMEOUT_MS = 10_000
const RECONNECT_MAX_MS = 30_000
/** Backend pushes connection/stats every 5s; four misses means the peer is frozen or gone. */
export const STALE_CONNECTION_MS = 20_000

/** True when no WebSocket data frame has arrived within the stale window. */
export function isWsConnectionStale(
  lastMessageAtMs: number,
  nowMs: number,
  staleMs: number = STALE_CONNECTION_MS,
): boolean {
  return nowMs - lastMessageAtMs >= staleMs
}

function makeRequestError(status: number, body: unknown): BackendRequestError {
  const error = new Error(`Request failed with status ${status}`) as BackendRequestError
  error.response = { status, data: body }
  return error
}

function isRetriableConnectionError(error: unknown, method: RequestMethod, sent: boolean): boolean {
  if (!(error instanceof Error)) return false
  const message = error.message

  // Never retry a change that may have reached the backend.
  if (method !== 'GET' && sent) return false

  // Timeouts after send are ambiguous for every method.
  if (message === 'Request timed out' && sent) return false

  return (
    message === 'WebSocket closed'
    || message === 'WebSocket connection failed'
    || message === 'WebSocket send failed'
    || message === 'Request timed out'
  )
}

function isTransportError(error: unknown): boolean {
  if (!(error instanceof Error)) return false
  const message = error.message
  return (
    message === 'WebSocket closed'
    || message === 'WebSocket connection failed'
    || message === 'WebSocket send failed'
    || message === 'Request timed out'
  )
}

function transportErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message === 'Request timed out') {
    return 'Request timed out. Check the connection and try again.'
  }
  return 'Backend disconnected. Changes were not applied.'
}

class BackendClient {
  private ws: WebSocket | null = null
  private nextId = 1
  private pending = new Map<number, PendingRequest>()
  private eventHandlers = new Map<string, Set<(body: unknown) => void>>()
  private connectPromise: Promise<void> | null = null
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private staleCheckTimer: ReturnType<typeof setInterval> | null = null
  // Reserved for an explicit disconnect() path; reconnect runs while false.
  private intentionalClose = false
  private reconnectAttempt = 0
  private lastMessageAt = 0
  private connectionState: ConnectionState = 'connecting'
  private connectionStateHandlers = new Set<(state: ConnectionState, previousState: ConnectionState) => void>()
  private transportErrorHandlers = new Set<(message: string) => void>()
  private listenersRegistered = false
  private subscribedCameraUuid: string | null = null
  private cameraSubscribeCounts = new Map<string, number>()
  private pendingSubscribe: string | null = null
  private subscribeQueued = false
  /** True once camera/state arrives for the active subscribe target. */
  private hasCameraState = false
  /** Re-subscribe on next camera/list only after unknown_camera rejection. */
  private subscribeNeedsRetry = false
  /** Hard-stop list retries after camera_cap until the user re-selects. */
  private subscribeBlocked = false
  private backendVersionObservation: BackendVersionObservation = {
    initial: null,
    changed: false,
  }
  private backendVersionChangeHandlers = new Set<() => void>()

  private wsUrl(): string {
    const url = new URL('v1/ws', window.location.href)
    url.protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    return url.href
  }

  private setConnectionState(state: ConnectionState): void {
    if (this.connectionState === state) return
    const previousState = this.connectionState
    this.connectionState = state
    for (const handler of this.connectionStateHandlers) {
      handler(state, previousState)
    }
  }

  private notifyTransportError(error: unknown): void {
    if (!isTransportError(error)) return
    const message = transportErrorMessage(error)
    this.notifyWarning(message)
  }

  private notifyWarning(message: string): void {
    for (const handler of this.transportErrorHandlers) {
      handler(message)
    }
  }

  onConnectionState(handler: (state: ConnectionState, previousState: ConnectionState) => void): () => void {
    this.connectionStateHandlers.add(handler)
    handler(this.connectionState, this.connectionState)
    return () => {
      this.connectionStateHandlers.delete(handler)
    }
  }

  onTransportError(handler: (message: string) => void): () => void {
    this.transportErrorHandlers.add(handler)
    return () => {
      this.transportErrorHandlers.delete(handler)
    }
  }

  onBackendVersionChanged(handler: () => void): () => void {
    this.backendVersionChangeHandlers.add(handler)
    if (this.backendVersionObservation.changed) {
      handler()
    }
    return () => {
      this.backendVersionChangeHandlers.delete(handler)
    }
  }

  private noteBackendVersion(diagnostics: { backend_version?: string } | null | undefined): void {
    const before = this.backendVersionObservation.changed
    observeBackendVersion(
      this.backendVersionObservation,
      diagnostics?.backend_version,
    )
    if (!before && this.backendVersionObservation.changed) {
      for (const changeHandler of this.backendVersionChangeHandlers) {
        changeHandler()
      }
    }
  }

  private registerListeners(): void {
    if (this.listenersRegistered) return
    this.listenersRegistered = true

    // Process-lifetime listeners on the singleton; never removed.
    window.addEventListener('online', () => {
      this.connect().catch(() => {
        this.scheduleReconnect()
      })
    })

    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState !== 'visible') return
      if (this.ws?.readyState === WebSocket.OPEN) return
      this.connect().catch(() => {
        this.scheduleReconnect()
      })
    })
  }

  connect(): Promise<void> {
    this.registerListeners()

    if (this.ws?.readyState === WebSocket.OPEN) {
      return Promise.resolve()
    }

    if (this.ws?.readyState === WebSocket.CONNECTING && this.connectPromise) {
      return this.connectPromise
    }

    // Wait out CLOSING so a stale onclose cannot orphan a newer socket.
    if (this.ws?.readyState === WebSocket.CLOSING) {
      return new Promise((resolve, reject) => {
        const started = Date.now()
        const check = setInterval(() => {
          if (this.ws?.readyState === WebSocket.OPEN) {
            clearInterval(check)
            resolve()
            return
          }
          if (!this.ws || this.ws.readyState === WebSocket.CLOSED) {
            clearInterval(check)
            this.connect().then(resolve).catch(reject)
            return
          }
          if (Date.now() - started > 5_000) {
            clearInterval(check)
            this.ws = null
            this.connect().then(resolve).catch(reject)
          }
        }, 50)
      })
    }

    if (this.connectPromise) {
      return this.connectPromise
    }

    this.intentionalClose = false
    this.setConnectionState('connecting')

    this.connectPromise = new Promise<void>((resolve, reject) => {
      let settled = false
      const ws = new WebSocket(this.wsUrl())
      this.ws = ws

      // A handshake started while the backend was down can stay in CONNECTING
      // forever. Reconnect is driven by onclose, so force one: without this the
      // client waits on a dead socket and the UI stays disabled until reload.
      const handshakeTimeout = setTimeout(() => {
        if (this.ws === ws && ws.readyState === WebSocket.CONNECTING) {
          ws.close()
        }
      }, HANDSHAKE_TIMEOUT_MS)

      ws.onopen = () => {
        clearTimeout(handshakeTimeout)
        settled = true
        this.connectPromise = null
        this.lastMessageAt = Date.now()
        this.setConnectionState('connected')
        this.startStaleCheck()
        // Cap is per-connection — clear before re-subscribe on a new socket.
        this.subscribeBlocked = false
        if (this.subscribedCameraUuid) {
          this.queueSubscribe(this.subscribedCameraUuid)
        }
        resolve()
      }

      ws.onmessage = (event) => {
        this.lastMessageAt = Date.now()
        // Reset backoff only after the connection proves it can deliver traffic.
        this.reconnectAttempt = 0
        this.handleMessage(String(event.data))
      }

      ws.onclose = () => {
        clearTimeout(handshakeTimeout)
        if (!this.dropSocket(ws)) return

        if (!settled) {
          settled = true
          reject(new Error('WebSocket connection failed'))
        }

        if (!this.intentionalClose) {
          this.scheduleReconnect()
        }
      }

      ws.onerror = () => {
        // onclose handles reconnect and initial failure
      }
    })

    return this.connectPromise
  }

  /**
   * Drop local socket state. Returns false if `ws` is no longer current.
   * Used by onclose and by the stale timer: a SIGSTOP'd peer never finishes the
   * WebSocket close handshake, so waiting for onclose alone leaves the UI "connected".
   */
  private dropSocket(ws: WebSocket): boolean {
    if (this.ws !== ws) return false
    this.ws = null
    this.connectPromise = null
    this.stopStaleCheck()
    this.setConnectionState('disconnected')
    // Force list-driven re-subscribe after reconnect — prior state may be gone.
    this.hasCameraState = false
    this.subscribeBlocked = false
    this.rejectAllPending(new Error('WebSocket closed'))
    return true
  }

  private startStaleCheck(): void {
    this.stopStaleCheck()
    this.staleCheckTimer = setInterval(() => {
      const ws = this.ws
      if (ws?.readyState !== WebSocket.OPEN) return
      if (!isWsConnectionStale(this.lastMessageAt, Date.now())) return
      if (!this.dropSocket(ws)) return
      if (!this.intentionalClose) {
        this.scheduleReconnect()
      }
      try {
        ws.close()
      } catch {
        // Peer may already be unreachable.
      }
    }, STALE_CONNECTION_MS / 3)
  }

  private stopStaleCheck(): void {
    if (this.staleCheckTimer !== null) {
      clearInterval(this.staleCheckTimer)
      this.staleCheckTimer = null
    }
  }

  private scheduleReconnect(): void {
    if (this.intentionalClose || this.reconnectTimer !== null) return

    const delay = Math.min(
      RECONNECT_BASE_MS * 2 ** this.reconnectAttempt,
      RECONNECT_MAX_MS,
    )
    const jitter = Math.floor(Math.random() * RECONNECT_BASE_MS)
    this.reconnectAttempt++

    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      this.connect().catch(() => {
        this.scheduleReconnect()
      })
    }, delay + jitter)
  }

  private rejectAllPending(error: Error): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeoutId)
      pending.reject(error)
    }
    this.pending.clear()
  }

  private handleMessage(raw: string): void {
    let message: WsResponse | WsEvent
    try {
      message = JSON.parse(raw) as WsResponse | WsEvent
    } catch (error) {
      console.warn('Ignoring invalid WebSocket JSON', error)
      return
    }

    if (message.type === 'response') {
      const pending = this.pending.get(message.id)
      if (!pending) return
      this.pending.delete(message.id)
      clearTimeout(pending.timeoutId)

      if (message.status >= 200 && message.status < 300) {
        pending.resolve(message.body)
      } else {
        pending.reject(makeRequestError(message.status, message.body))
      }
      return
    }

    if (message.type === 'event') {
      // Re-subscribe only when we still need state (boot race) or the last reject was
      // unknown_camera — not on every list churn. system/health is a trigger too: the
      // backend also accepts a subscribe once the camera is merely configured, and that
      // flip is carried by health (expected_missing), never by camera/list.
      if (
        (message.event === 'camera/list' || message.event === 'system/health')
        && this.subscribedCameraUuid
      ) {
        if (this.subscribeBlocked) {
          // Over capacity — wait for an explicit subscribeCamera (user action).
        } else if (!this.hasCameraState || this.subscribeNeedsRetry) {
          this.queueSubscribe(this.subscribedCameraUuid)
        }
      }
      if (message.event === 'system/health') {
        const body = message.body as { diagnostics?: { backend_version?: string } } | null
        this.noteBackendVersion(body?.diagnostics)
      }
      if (message.event === 'camera/subscribe_rejected' && this.subscribedCameraUuid) {
        const body = message.body as { camera_uuid?: string; reason?: string } | null
        if (!body?.camera_uuid || body.camera_uuid === this.subscribedCameraUuid) {
          if (body?.reason === 'unknown_camera') {
            // Retry from the next list/health push — do not immediate-resubscribe (storm).
            if (!this.subscribeNeedsRetry) {
              console.warn('Camera subscribe rejected: unknown_camera; retrying on next list/health')
            }
            this.subscribeNeedsRetry = true
            this.subscribeBlocked = false
          } else if (body?.reason === 'camera_cap') {
            this.subscribeBlocked = true
            this.subscribeNeedsRetry = false
            this.notifyWarning('Camera subscribe rejected: camera limit reached')
          } else {
            console.warn('Camera subscribe rejected:', body?.reason ?? 'unknown')
          }
        }
      }
      if (message.event === 'camera/state' && this.subscribedCameraUuid) {
        const body = message.body as { camera_uuid?: string } | null
        if (body?.camera_uuid === this.subscribedCameraUuid) {
          this.hasCameraState = true
          this.subscribeNeedsRetry = false
        }
      }

      const handlers = this.eventHandlers.get(message.event)
      if (!handlers) return
      for (const handler of handlers) {
        handler(message.body)
      }
    }
  }

  private sendRequest<T>(method: RequestMethod, path: string, body?: unknown): Promise<T> {
    const id = this.nextId++
    const request: WsRequest = { id, method, path }
    if (body !== undefined) {
      request.body = body
    }

    return new Promise<T>((resolve, reject) => {
      const pending: PendingRequest = {
        method,
        path,
        body,
        resolve: (data) => resolve(data as T),
        reject,
        timeoutId: setTimeout(() => {
          if (!this.pending.has(id)) return
          this.pending.delete(id)
          reject(new Error('Request timed out'))
        }, REQUEST_TIMEOUT_MS),
        sent: false,
      }

      this.pending.set(id, pending)

      try {
        if (this.ws?.readyState !== WebSocket.OPEN) {
          throw new Error('WebSocket send failed')
        }
        this.ws.send(JSON.stringify(request))
        pending.sent = true
      } catch {
        clearTimeout(pending.timeoutId)
        this.pending.delete(id)
        this.ws?.close()
        reject(new Error('WebSocket send failed'))
      }
    })
  }

  async request<T>(method: RequestMethod, path: string, body?: unknown): Promise<T> {
    let lastError: unknown
    // Deliberately sticky across attempts: once a POST may have reached the backend,
    // never retry it (non-idempotent camera/autopilot control).
    let sent = false

    for (let attempt = 0; attempt < REQUEST_MAX_ATTEMPTS; attempt++) {
      try {
        // Promote a scheduled reconnect into an immediate connect attempt.
        if (this.reconnectTimer !== null) {
          clearTimeout(this.reconnectTimer)
          this.reconnectTimer = null
        }
        await this.connect()
        const result = await this.sendRequest<T>(method, path, body)
        return result
      } catch (error) {
        lastError = error
        if (error instanceof Error && error.message !== 'WebSocket send failed' && error.message !== 'WebSocket connection failed') {
          sent = true
        }
        if (!isRetriableConnectionError(error, method, sent) || attempt === REQUEST_MAX_ATTEMPTS - 1) {
          this.notifyTransportError(error)
          throw error
        }
      }
    }

    throw lastError
  }

  onEvent(event: string, handler: (body: unknown) => void): () => void {
    let handlers = this.eventHandlers.get(event)
    if (!handlers) {
      handlers = new Set()
      this.eventHandlers.set(event, handlers)
    }
    handlers.add(handler)

    return () => {
      const current = this.eventHandlers.get(event)
      if (!current) return
      current.delete(handler)
      if (current.size === 0) {
        this.eventHandlers.delete(event)
      }
    }
  }

  subscribeCamera(cameraUuid: string): void {
    const count = this.cameraSubscribeCounts.get(cameraUuid) ?? 0
    this.cameraSubscribeCounts.set(cameraUuid, count + 1)

    if (this.subscribedCameraUuid && this.subscribedCameraUuid !== cameraUuid) {
      const previous = this.subscribedCameraUuid
      // Single active wire UUID: drop previous counts and unsubscribe once.
      this.cameraSubscribeCounts.delete(previous)
      // Null before unsubscribe so the deferred/open guard does not drop the frame.
      this.subscribedCameraUuid = null
      this.sendCameraSubscription('unsubscribe', previous)
    }

    this.subscribedCameraUuid = cameraUuid
    this.hasCameraState = false
    this.subscribeNeedsRetry = false
    this.subscribeBlocked = false
    // Always send subscribe so the backend re-pushes UI + snapshot for this
    // consumer (refcount > 0 alone used to skip the frame and leave remounts blank).
    this.queueSubscribe(cameraUuid)
  }

  /** Re-push subscribe for the active camera without changing refcounts (tab remount). */
  refreshCameraSubscription(): void {
    if (this.subscribedCameraUuid) {
      this.queueSubscribe(this.subscribedCameraUuid)
    }
  }

  /** Collapse same-uuid subscribes issued in the same tick into one wire frame. */
  private queueSubscribe(cameraUuid: string): void {
    if (this.subscribeBlocked) return
    this.pendingSubscribe = cameraUuid
    if (this.subscribeQueued) return
    this.subscribeQueued = true
    queueMicrotask(() => {
      this.subscribeQueued = false
      const pending = this.pendingSubscribe
      this.pendingSubscribe = null
      // Skip if unsubscribe cleared the active uuid in the same tick.
      if (pending && pending === this.subscribedCameraUuid && !this.subscribeBlocked) {
        this.sendCameraSubscription('subscribe', pending)
      }
    })
  }

  unsubscribeCamera(cameraUuid: string): void {
    const count = this.cameraSubscribeCounts.get(cameraUuid) ?? 0
    if (count <= 1) {
      this.cameraSubscribeCounts.delete(cameraUuid)
      if (this.subscribedCameraUuid === cameraUuid) {
        this.subscribedCameraUuid = null
        this.sendCameraSubscription('unsubscribe', cameraUuid)
      }
      return
    }

    this.cameraSubscribeCounts.set(cameraUuid, count - 1)
  }

  dismissUi(cameraUuid: string, field: UiDismissField): void {
    const message: WsClientMessage = {
      type: 'ui_dismiss',
      camera_uuid: cameraUuid,
      field,
    }
    const payload = JSON.stringify(message)
    const send = (): void => {
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.ws.send(payload)
      }
    }

    if (this.ws?.readyState === WebSocket.OPEN) {
      send()
      return
    }

    this.connect().then(send).catch((error) => {
      // Local dismiss still cleared the overlay; wire retry is best-effort.
      console.warn('Failed to send ui_dismiss', error)
    })
  }

  private sendCameraSubscription(type: 'subscribe' | 'unsubscribe', cameraUuid: string): void {
    const message: WsClientMessage = type === 'subscribe'
      ? { type: 'subscribe', camera_uuid: cameraUuid }
      : { type: 'unsubscribe', camera_uuid: cameraUuid }
    const payload = JSON.stringify(message)
    const send = (): void => {
      if (this.ws?.readyState !== WebSocket.OPEN) return
      // Drop stale deferred sends that no longer match the active subscription.
      if (type === 'subscribe' && this.subscribedCameraUuid !== cameraUuid) return
      if (type === 'unsubscribe' && this.subscribedCameraUuid === cameraUuid) return
      this.ws.send(payload)
    }

    if (this.ws?.readyState === WebSocket.OPEN) {
      send()
      return
    }

    this.connect().then(send).catch((error) => {
      console.warn(`Failed to send camera ${type}`, error)
    })
  }
}

export const backendClient = new BackendClient()
