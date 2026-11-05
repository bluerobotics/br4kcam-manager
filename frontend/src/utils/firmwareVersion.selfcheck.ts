import { newerFirmware } from './firmwareVersion'

export function runFirmwareVersionSelfCheck(): void {
  const v16 = '18.010.U35.5_0054_V16(260305)'
  const v18 = '18.010.U35.6_0054_V18(261030)'
  const v18Rebuild = '18.010.U35.6_0054_V18(261101)'
  const cases: [string, string[], string | null][] = [
    [v16, [v16, v18], v18],
    [v18, [v16, v18], null],
    [v18, [v18Rebuild, v16], v18Rebuild],
    ['garbage', [v18], null],
    [v16, ['garbage'], null],
  ]
  for (const [current, available, expected] of cases) {
    const actual = newerFirmware(current, available)
    if (actual !== expected) {
      throw new Error(`firmwareVersion: ${current} vs ${available} gave ${actual}, expected ${expected}`)
    }
  }
  console.log('firmwareVersion self-check ok')
}
