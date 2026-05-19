import { useEffect, useState } from 'react'

/** Whether the page is visible (foreground tab, not minimized). */
export function useDocumentVisible(): boolean {
  const [visible, setVisible] = useState(
    () => typeof document === 'undefined' || !document.hidden,
  )

  useEffect(() => {
    const onChange = () => setVisible(!document.hidden)
    document.addEventListener('visibilitychange', onChange)
    return () => document.removeEventListener('visibilitychange', onChange)
  }, [])

  return visible
}

/** Poll interval when the tab is in the background (Page Visibility API). */
export const HIDDEN_TAB_POLL_MS = 30_000

/** `visibleMs` while the tab is visible; {@link HIDDEN_TAB_POLL_MS} when hidden. */
export function useVisibilityPollMs(visibleMs: number): number {
  const visible = useDocumentVisible()
  return visible ? visibleMs : HIDDEN_TAB_POLL_MS
}
