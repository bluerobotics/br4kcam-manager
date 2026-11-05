/** Orders camera firmware strings like `18.010.U35.6_0054_V18(261030)` by major version, then build date. */
function firmwareRank(version: string): number | null {
  const match = version.match(/_V(\d+)\((\d+)\)/)
  return match ? Number(match[1]) * 1e6 + Number(match[2]) : null
}

/** The newest of `available` when it is newer than `current`, else null. */
export function newerFirmware(current: string, available: string[]): string | null {
  const currentRank = firmwareRank(current)
  if (currentRank === null) return null
  let best: string | null = null
  let bestRank = currentRank
  for (const version of available) {
    const rank = firmwareRank(version)
    if (rank !== null && rank > bestRank) {
      best = version
      bestRank = rank
    }
  }
  return best
}

/** Whether two camera UUIDs are the same physical camera: 4K Cam ONVIF UUIDs carry the MAC in
 * their first and last 12 hex digits, while the middle changes with the camera firmware. */
export function sameCamera(a: string, b: string): boolean {
  const [x, y] = [a, b].map((uuid) => uuid.replace(/-/g, '').toLowerCase())
  return x.length === 32 && y.length === 32 && x.slice(0, 12) === y.slice(0, 12) && x.slice(20) === y.slice(20)
}
