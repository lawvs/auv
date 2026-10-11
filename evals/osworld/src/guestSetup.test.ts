import { afterEach, describe, expect, it, vi } from 'vitest'

import { installRuntimeLibraries } from './guestSetup'

function result(overrides: Partial<{
  error: string
  output: string
  returncode: number
  status: string
}> = {}) {
  return {
    error: '',
    output: '',
    returncode: 0,
    status: 'success',
    ...overrides,
  }
}

describe('installRuntimeLibraries', () => {
  const context = { caseId: 'case-1', podName: 'pod-1' }

  afterEach(() => {
    vi.useRealTimers()
  })

  it('retries while packagekit temporarily owns an apt lock', async () => {
    // ROOT CAUSE:
    //
    // If packagekitd still owns the apt lists lock during VM startup, the
    // one-shot apt command failed even though the lock was released shortly
    // afterward. The retry keeps transient lock contention inside setup.
    vi.useFakeTimers()
    const locked = result({
      error: 'E: Could not get lock /var/lib/apt/lists/lock. It is held by process 1213 (packagekitd)',
      returncode: 100,
      status: 'error',
    })
    const installed = result({ output: 'installed' })
    const execute = vi.fn()
      .mockResolvedValueOnce(locked)
      .mockResolvedValueOnce(installed)

    const installation = installRuntimeLibraries(execute, new AbortController().signal, context)
    await vi.advanceTimersByTimeAsync(2_000)

    await expect(installation).resolves.toEqual(installed)
    expect(execute).toHaveBeenCalledTimes(2)
  })

  it('retries when a concurrent apt run replaces the package cache', async () => {
    // ROOT CAUSE:
    //
    // If another apt process rebuilt the package cache during VM startup,
    // apt-get failed with "The package cache file is corrupted" and the
    // episode errored (gimp/d52d6308, test_nogdrive 2026-10-11). Like lock
    // contention, a later attempt succeeds.
    vi.useFakeTimers()
    const raced = result({
      error: 'E: Problem renaming the file /var/cache/apt/pkgcache.bin.w86gQJ to /var/cache/apt/pkgcache.bin - rename (2: No such file or directory)\nE: The package cache file is corrupted',
      returncode: 100,
      status: 'error',
    })
    const installed = result({ output: 'installed' })
    const execute = vi.fn()
      .mockResolvedValueOnce(raced)
      .mockResolvedValueOnce(installed)

    const installation = installRuntimeLibraries(execute, new AbortController().signal, context)
    await vi.advanceTimersByTimeAsync(2_000)

    await expect(installation).resolves.toEqual(installed)
    expect(execute).toHaveBeenCalledTimes(2)
  })

  it('does not retry unrelated apt failures', async () => {
    const failure = result({
      error: 'E: Unable to locate package libdoes-not-exist',
      returncode: 100,
      status: 'error',
    })
    const execute = vi.fn().mockResolvedValue(failure)

    await expect(installRuntimeLibraries(execute, new AbortController().signal, context)).resolves.toEqual(failure)
    expect(execute).toHaveBeenCalledOnce()
  })
})
