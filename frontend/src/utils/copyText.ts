import type { CameraConnectivity, SystemHealth } from '@/bindings/br4kcam_api'

/** Best-effort copy for BlueOS HTTP pages (often non-secure-context / iframe). */
export type CopyTextResult = 'copied' | 'manual'

export type DiagnosticsContext = {
  system_health: SystemHealth | null
  camera_connectivity?: CameraConnectivity | null
  problem_titles?: string[]
  page_url?: string
  page_title?: string
  user_agent?: string
}

export function buildDiagnosticsPayload(ctx: DiagnosticsContext): Record<string, unknown> {
  return {
    exported_at: new Date().toISOString(),
    page: {
      url: ctx.page_url ?? (typeof window !== 'undefined' ? window.location.href : null),
      title: ctx.page_title ?? (typeof document !== 'undefined' ? document.title : null),
    },
    user_agent: ctx.user_agent ?? (typeof navigator !== 'undefined' ? navigator.userAgent : null),
    problems: ctx.problem_titles ?? [],
    system_health: ctx.system_health,
    camera_connectivity: ctx.camera_connectivity ?? null,
  }
}

/**
 * Fallback copy via a temporary field + execCommand.
 * stopPropagation on focusin matters inside dialog focus traps (BlueOS pattern).
 * textarea keeps large JSON payloads intact.
 */
function copyWithFallbackMethod(text: string): boolean {
  const field = document.createElement('textarea')
  field.addEventListener('focusin', (event) => event.stopPropagation())
  field.value = text
  field.setAttribute('readonly', '')
  field.style.cssText =
    'position:fixed;top:0;left:0;width:2em;height:2em;padding:0;border:none;outline:none;box-shadow:none;background:transparent;opacity:0.01;z-index:2147483647'
  document.body.appendChild(field)

  try {
    field.focus()
    field.select()
    field.setSelectionRange(0, text.length)
    return document.execCommand('copy')
  } catch (error) {
    console.error(`Failed to copy text to clipboard. Reason: ${error}`)
    return false
  } finally {
    document.body.removeChild(field)
  }
}

async function copyWithClipboardAPI(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch (error) {
    console.error(`Failed to copy text to clipboard using Clipboard API. Reason: ${error}`)
    return copyWithFallbackMethod(text)
  }
}

/** Copy text with Clipboard API when allowed, else BlueOS-style execCommand fallback. */
export async function copyText(text: string): Promise<CopyTextResult> {
  if (!text) return 'manual'

  const canUseClipboardApi =
    typeof navigator !== 'undefined'
    && typeof navigator.clipboard?.writeText === 'function'

  try {
    if (canUseClipboardApi && typeof navigator.permissions?.query === 'function') {
      const permissionStatus = await navigator.permissions.query({
        name: 'clipboard-write' as PermissionName,
      })

      if (permissionStatus.state === 'granted' || permissionStatus.state === 'prompt') {
        // prompt: writeText itself triggers the browser prompt; don't wait on onchange.
        return (await copyWithClipboardAPI(text)) ? 'copied' : 'manual'
      }
      return copyWithFallbackMethod(text) ? 'copied' : 'manual'
    }
  } catch (error) {
    console.error('Error while requesting clipboard-write permission:', error)
  }

  if (canUseClipboardApi) {
    return (await copyWithClipboardAPI(text)) ? 'copied' : 'manual'
  }
  return copyWithFallbackMethod(text) ? 'copied' : 'manual'
}

export function diagnosticsJson(blob: unknown): string {
  return JSON.stringify(
    blob,
    (_key, value) => (typeof value === 'bigint' ? value.toString() : value),
    2,
  )
}
