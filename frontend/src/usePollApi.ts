import { useEffect } from 'react'

/** Poll on an interval; first tick is deferred so the effect body does not call setState synchronously. */
export function usePollApi(
  fn: () => void | Promise<void>,
  intervalMs: number,
  deps: readonly unknown[],
): void {
  useEffect(() => {
    let cancelled = false
    const run = () => {
      if (!cancelled) void fn()
    }
    queueMicrotask(run)
    const id = window.setInterval(run, intervalMs)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- caller supplies deps
  }, [intervalMs, ...deps])
}
