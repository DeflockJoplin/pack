import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'
import { apiGet } from './api'
import { usePollApi } from './usePollApi'
import { useVisibilityPollMs } from './useDocumentVisible'

type BootStats = {
  boot_upload_phase: string
  boot_upload_in_progress: boolean
  capture_started: boolean
  boot_upload_current_file: string | null
  boot_upload_sessions_done: number
  boot_upload_sessions_total: number
  boot_upload_error: string | null
  upload_last_message: string | null
}

const COMPLETE_DISMISS_MS = 8000

function BootBanner({
  variant,
  title,
  body,
  onDismiss,
}: {
  variant: 'progress' | 'info' | 'ok' | 'warn'
  title: string
  body: ReactNode
  onDismiss?: () => void
}) {
  return (
    <div className={`boot-upload-banner boot-upload-banner--${variant}`} role="status">
      <div className="boot-upload-banner__text">
        <strong className="boot-upload-banner__title">{title}</strong>
        <p className="boot-upload-banner__body muted">{body}</p>
        <p className="boot-upload-banner__link muted">
          <Link to="/uploads">Uploads</Link>
        </p>
      </div>
      {onDismiss && (
        <button type="button" className="secondary boot-upload-banner__dismiss" onClick={onDismiss}>
          Dismiss
        </button>
      )}
    </div>
  )
}

export function BootUploadBanner() {
  const pollMs = useVisibilityPollMs(1500)
  const [snap, setSnap] = useState<BootStats | null>(null)
  const [dismissedPhase, setDismissedPhase] = useState<string | null>(null)
  const completeDismissTimer = useRef<number | null>(null)

  usePollApi(async () => {
    try {
      const s = await apiGet<BootStats>('/api/stats')
      setSnap(s)
    } catch {
      /* keep last snapshot */
    }
  }, pollMs, [pollMs])

  useEffect(() => {
    if (completeDismissTimer.current != null) {
      window.clearTimeout(completeDismissTimer.current)
      completeDismissTimer.current = null
    }
    if (snap?.boot_upload_phase !== 'complete') return
    completeDismissTimer.current = window.setTimeout(() => {
      setDismissedPhase('complete')
    }, COMPLETE_DISMISS_MS)
    return () => {
      if (completeDismissTimer.current != null) {
        window.clearTimeout(completeDismissTimer.current)
        completeDismissTimer.current = null
      }
    }
  }, [snap?.boot_upload_phase])

  if (!snap) return null

  const phase = snap.boot_upload_phase
  if (phase === 'skipped' || phase === 'pending') return null
  if (dismissedPhase === phase) return null

  const sessions =
    snap.boot_upload_sessions_total > 0
      ? `${snap.boot_upload_sessions_done} / ${snap.boot_upload_sessions_total} sessions`
      : null

  if (phase === 'in_progress') {
    return (
      <BootBanner
        variant="progress"
        title="Uploading pending sessions before wardriving…"
        body={
          <>
            Capture starts when uploads finish.
            {snap.boot_upload_current_file && (
              <>
                {' '}
                Current: <code>{snap.boot_upload_current_file}</code>
              </>
            )}
            {sessions && <> · {sessions}</>}
            {snap.upload_last_message && (
              <span className="boot-upload-banner__sub"> {snap.upload_last_message}</span>
            )}
          </>
        }
        onDismiss={() => setDismissedPhase(phase)}
      />
    )
  }

  if (phase === 'skipped_offline') {
    if (snap.capture_started) return null
    return (
      <BootBanner
        variant="info"
        title="No network — skipped boot upload"
        body="Starting capture without uploading pending sessions."
        onDismiss={() => setDismissedPhase(phase)}
      />
    )
  }

  if (phase === 'complete') {
    return (
      <BootBanner
        variant="ok"
        title="Boot upload finished"
        body={
          <>
            {sessions ?? 'Pending sessions processed.'} Capture is starting or running.
            {snap.upload_last_message && (
              <span className="boot-upload-banner__sub"> {snap.upload_last_message}</span>
            )}
          </>
        }
        onDismiss={() => setDismissedPhase(phase)}
      />
    )
  }

  if (phase === 'failed') {
    return (
      <BootBanner
        variant="warn"
        title="Boot upload failed"
        body={
          <>
            {snap.boot_upload_error ?? 'See Uploads or server logs.'} Capture will still start.{' '}
            <Link to="/files">Files</Link>
          </>
        }
        onDismiss={() => setDismissedPhase(phase)}
      />
    )
  }

  return null
}
