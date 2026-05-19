import { useEffect } from 'react'

/**
 * Run an async loader after mount/deps change without calling setState synchronously
 * inside the effect body (satisfies react-hooks/set-state-in-effect).
 */
export function useApiLoad(load: () => void | Promise<void>, deps: readonly unknown[]): void {
  useEffect(() => {
    queueMicrotask(() => {
      void load()
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps -- caller supplies full dep list
  }, deps)
}
