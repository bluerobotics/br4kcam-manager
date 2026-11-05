import { newerFirmware, sameCamera } from './firmwareVersion'

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
  const cam = 'bc071803-0839-8301-ac36-bc0718030839'
  if (!sameCamera(cam, 'bc071803-0839-8801-ae86-bc0718030839')) {
    throw new Error('firmwareVersion: same MAC with another firmware must be the same camera')
  }
  if (sameCamera(cam, 'bc071802-2c6d-8801-ae86-bc0718022c6d') || sameCamera(cam, 'garbage')) {
    throw new Error('firmwareVersion: another MAC must be another camera')
  }
  console.log('firmwareVersion self-check ok')
}
