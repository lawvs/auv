import { writeExecutionLog } from './executionLog'

export interface GuestSetupContext {
  caseId: string
  podName: string
}

export interface SetupCommandResult {
  error: string
  output: string
  returncode: number
  status: string
}

const maxAttempts = 60
const retryDelayMs = 2_000

/**
 * Installs guest runtime libraries, waiting out transient apt lock contention.
 *
 * A fresh OSWorld VM can start PackageKit concurrently with the episode. Known
 * apt/dpkg lock and package-cache races, and setup requests that lose their
 * connection (`fetch` TypeError), are retried. Package, network, and
 * repository failures inside the guest return immediately to preserve the
 * original diagnostic.
 */
export async function installRuntimeLibraries(
  execute: () => Promise<SetupCommandResult>,
  signal: AbortSignal,
  context: GuestSetupContext,
): Promise<SetupCommandResult> {
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    signal.throwIfAborted()
    // A dropped port-forward connection is retried like lock contention;
    // the apt command is idempotent, so resending it is safe.
    const result = await execute().catch((error: unknown) => {
      if (signal.aborted || attempt === maxAttempts || !(error instanceof TypeError)) {
        throw error
      }
      return undefined
    })
    if (result != null && (!isAptLockContention(result) || attempt === maxAttempts)) {
      return result
    }

    writeExecutionLog('osworld:guest-setup', 'warn', result == null ? 'setup request lost its connection; retrying runtime library installation' : 'apt lock is busy; retrying runtime library installation', {
      ...context,
      attempt,
      maxAttempts,
      retryDelayMs,
    })
    await delay(retryDelayMs, signal)
  }

  throw new Error('Runtime library installation exhausted its bounded retry loop.')
}

async function delay(milliseconds: number, signal: AbortSignal): Promise<void> {
  await new Promise<void>((resolveDelay, reject) => {
    let timeout: ReturnType<typeof setTimeout>
    const onAbort = () => {
      clearTimeout(timeout)
      reject(signal.reason)
    }
    timeout = setTimeout(() => {
      signal.removeEventListener('abort', onAbort)
      resolveDelay()
    }, milliseconds)
    signal.addEventListener('abort', onAbort, { once: true })
  })
}

function isAptLockContention(result: SetupCommandResult): boolean {
  if (result.returncode === 0) {
    return false
  }
  const diagnostic = `${result.error}\n${result.output}`.toLowerCase()
  return diagnostic.includes('could not get lock')
    || diagnostic.includes('unable to lock directory')
    || diagnostic.includes('unable to acquire the dpkg frontend lock')
    || diagnostic.includes('is another process using it')
    // A concurrent apt run replaced pkgcache.bin while this one rebuilt it.
    || diagnostic.includes('the package cache file is corrupted')
}
