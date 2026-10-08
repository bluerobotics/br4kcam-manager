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
  field.style.cssText =
    'position:fixed;top:0;left:0;width:2em;height:2em;padding:0;border:none;outline:none;box-shadow:none;background:transparent;opacity:0.01;z-index:2147483647'
  // A modal <dialog> makes everything outside it inert (unfocusable), so mount inside it.
  const container = document.activeElement?.closest('dialog, [role="dialog"]') ?? document.body
  container.appendChild(field)

  try {
    field.focus()
    field.select()
    field.setSelectionRange(0, text.length)
    return document.execCommand('copy')
  } catch (error) {
    console.error(`Failed to copy text to clipboard. Reason: ${error}`)
    return false
  } finally {
    field.remove()
  }
}

/**
 * Copy text. execCommand runs first and synchronously so it stays inside the click's
 * user activation (awaiting permissions/Clipboard API first can lose it in BlueOS iframes);
 * the Clipboard API is only the secondary path.
 */
export async function copyText(text: string): Promise<CopyTextResult> {
  if (!text) return 'manual'
  if (copyWithFallbackMethod(text)) return 'copied'
  if (typeof navigator !== 'undefined' && typeof navigator.clipboard?.writeText === 'function') {
    try {
      await navigator.clipboard.writeText(text)
      return 'copied'
    } catch (error) {
      console.error(`Failed to copy text to clipboard using Clipboard API. Reason: ${error}`)
    }
  }
  return 'manual'
}

export function diagnosticsJson(blob: unknown): string {
  return JSON.stringify(
    blob,
    (_key, value) => (typeof value === 'bigint' ? value.toString() : value),
    2,
  )
}
