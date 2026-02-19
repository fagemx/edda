/**
 * Graceful shutdown handler.
 *
 * - First Ctrl+C: set shuttingDown flag, finish current phase
 * - Second Ctrl+C: force abort current phase + exit
 *
 * Uses Promise-based shutdown signal (vm0 pattern) for multiple listeners.
 */

let _resolveShutdown: (() => void) | null = null;
const _shutdownPromise = new Promise<void>((resolve) => {
  _resolveShutdown = resolve;
});

let _isShuttingDown = false;
let _currentAbort: AbortController | null = null;
let _sigintCount = 0;
let _installed = false;

/** Whether shutdown has been requested */
export function isShuttingDown(): boolean {
  return _isShuttingDown;
}

/** Promise that resolves when shutdown is requested */
export function shutdownPromise(): Promise<void> {
  return _shutdownPromise;
}

/** Register the current phase's AbortController for force-quit */
export function setCurrentAbort(controller: AbortController | null): void {
  _currentAbort = controller;
}

/** Install signal handlers. Safe to call multiple times (idempotent). */
export function installShutdownHandlers(): void {
  if (_installed) return;
  _installed = true;

  process.on('SIGINT', () => {
    _sigintCount++;

    if (_sigintCount === 1) {
      if (!_currentAbort) {
        // No phase running — just exit
        process.exit(0);
      }
      console.log('\n⏸ Stopping after current phase completes...');
      console.log('  Press Ctrl+C again to force quit.\n');
      _isShuttingDown = true;
      _resolveShutdown?.();
    } else {
      console.log('\n⚡ Force quit. Current phase marked as stale.');
      _currentAbort?.abort();
      // Give a moment for cleanup, then force exit
      setTimeout(() => process.exit(1), 3000);
    }
  });
}

/** Reset shutdown state (for testing) */
export function resetShutdownState(): void {
  _isShuttingDown = false;
  _currentAbort = null;
  _sigintCount = 0;
}
