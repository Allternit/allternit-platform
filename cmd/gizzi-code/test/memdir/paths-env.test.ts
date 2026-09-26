import { afterEach, describe, expect, test } from 'bun:test'
import {
  getMemoryBaseDir,
  getRemoteMemoryDirOverride,
} from '../../src/memdir/paths'

// Runtime-copy env convergence: both GIZZI_CODE_REMOTE_MEMORY_DIR and legacy
// GIZZI_REMOTE_MEMORY_DIR must be honored, GIZZI_CODE_ winning when both set.
// (The ink-app copy is not test-importable — its module graph hangs under
// bun test — so the identical logic there is covered by code review + the
// production-bundle grep in the session evidence.)

const ENV_KEYS = [
  'GIZZI_CODE_REMOTE_MEMORY_DIR',
  'GIZZI_REMOTE_MEMORY_DIR',
] as const

afterEach(() => {
  for (const k of ENV_KEYS) delete process.env[k]
})

describe('memdir remote memory dir env convergence', () => {
  test('GIZZI_CODE_REMOTE_MEMORY_DIR sets the base dir', () => {
    process.env.GIZZI_CODE_REMOTE_MEMORY_DIR = '/tmp/p11-code-name'
    expect(getRemoteMemoryDirOverride()).toBe('/tmp/p11-code-name')
    expect(getMemoryBaseDir()).toBe('/tmp/p11-code-name')
  })

  test('legacy GIZZI_REMOTE_MEMORY_DIR still sets the base dir', () => {
    process.env.GIZZI_REMOTE_MEMORY_DIR = '/tmp/p11-legacy-name'
    expect(getRemoteMemoryDirOverride()).toBe('/tmp/p11-legacy-name')
    expect(getMemoryBaseDir()).toBe('/tmp/p11-legacy-name')
  })

  test('GIZZI_CODE_ name wins when both are set', () => {
    process.env.GIZZI_CODE_REMOTE_MEMORY_DIR = '/tmp/p11-wins'
    process.env.GIZZI_REMOTE_MEMORY_DIR = '/tmp/p11-loses'
    expect(getRemoteMemoryDirOverride()).toBe('/tmp/p11-wins')
  })

  test('neither set → override is undefined', () => {
    expect(getRemoteMemoryDirOverride()).toBeUndefined()
  })
})
