import { useCallback, useState } from 'react'
import { apiGet, apiPost } from '../api'
import { useApiLoad } from '../useApiLoad'
import { usePollApi } from '../usePollApi'
import { useVisibilityPollMs } from '../useDocumentVisible'

const WIGLE_ACCOUNT_URL = 'https://wigle.net/account'
const WDGWARS_PROFILE_URL = 'https://wdgwars.pl/profile/'

type UploadsFlags = {
  upload_on_boot: boolean
  enable_wigle_upload: boolean
  enable_wdgwars_upload: boolean
}

type AppConfigPublic = {
  uploads: UploadsFlags & {
    wigle_api_name?: string | null
  }
  wigle_api_token_set: boolean
  wdgwars_api_key_set: boolean
}

type UploadsForm = {
  wigle_api_name: string
  wigle_api_token: string
  wdgwars_api_key: string
  upload_on_boot: boolean
  enable_wigle_upload: boolean
  enable_wdgwars_upload: boolean
}

type UploadsPatch = {
  wigle_api_name?: string
  wigle_api_token?: string | null
  wdgwars_api_key?: string | null
  upload_on_boot: boolean
  enable_wigle_upload: boolean
  enable_wdgwars_upload: boolean
}

type UploadSummary = {
  wigle_ok: number
  wigle_failed: number
  wdgwars_ok: number
  wdgwars_failed: number
}

type BootUploadStats = {
  boot_upload_phase: string
  boot_upload_in_progress: boolean
  capture_started: boolean
  boot_upload_current_file: string | null
  boot_upload_sessions_done: number
  boot_upload_sessions_total: number
  boot_upload_error: string | null
  upload_last_message: string | null
  upload_wigle_ok: number
  upload_wigle_failed: number
  upload_wdgwars_ok: number
  upload_wdgwars_failed: number
}

const defaultForm = (): UploadsForm => ({
  wigle_api_name: '',
  wigle_api_token: '',
  wdgwars_api_key: '',
  upload_on_boot: true,
  enable_wigle_upload: true,
  enable_wdgwars_upload: true,
})

export function UploadsPage() {
  const [form, setForm] = useState<UploadsForm>(defaultForm)
  const [wigleTokenSet, setWigleTokenSet] = useState(false)
  const [wdgKeySet, setWdgKeySet] = useState(false)
  const [tokenDirty, setTokenDirty] = useState(false)
  const [wdgKeyDirty, setWdgKeyDirty] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [saveBusy, setSaveBusy] = useState(false)
  const [lastRun, setLastRun] = useState<UploadSummary | null>(null)
  const [runErr, setRunErr] = useState<string | null>(null)
  const [bootStats, setBootStats] = useState<BootUploadStats | null>(null)
  const bootPollMs = useVisibilityPollMs(1500)

  const load = useCallback(async () => {
    setErr(null)
    try {
      const c = await apiGet<AppConfigPublic>('/api/config')
      const u = c.uploads
      setForm({
        wigle_api_name: u.wigle_api_name?.trim() ?? '',
        wigle_api_token: '',
        wdgwars_api_key: '',
        upload_on_boot: u.upload_on_boot ?? true,
        enable_wigle_upload: u.enable_wigle_upload ?? true,
        enable_wdgwars_upload: u.enable_wdgwars_upload ?? true,
      })
      setWigleTokenSet(c.wigle_api_token_set)
      setWdgKeySet(c.wdgwars_api_key_set)
      setTokenDirty(false)
      setWdgKeyDirty(false)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }, [])

  useApiLoad(() => void load(), [load])

  usePollApi(async () => {
    try {
      const s = await apiGet<BootUploadStats>('/api/stats')
      setBootStats(s)
    } catch {
      /* keep last boot snapshot */
    }
  }, bootPollMs, [bootPollMs])

  const save = async () => {
    setMsg(null)
    setErr(null)
    setSaveBusy(true)
    try {
      const patch: UploadsPatch = {
        wigle_api_name: form.wigle_api_name.trim(),
        upload_on_boot: form.upload_on_boot,
        enable_wigle_upload: form.enable_wigle_upload,
        enable_wdgwars_upload: form.enable_wdgwars_upload,
      }
      if (tokenDirty) {
        patch.wigle_api_token = form.wigle_api_token.trim() || null
      }
      if (wdgKeyDirty) {
        patch.wdgwars_api_key = form.wdgwars_api_key.trim() || null
      }
      await apiPost('/api/config', { uploads: patch })
      setMsg('Saved to data/config/app.json')
      await load()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setSaveBusy(false)
    }
  }

  const runUpload = async () => {
    setRunErr(null)
    setBusy(true)
    try {
      const r = await apiPost<{ ok: boolean; summary: UploadSummary }>('/api/uploads/run', {})
      setLastRun(r.summary)
    } catch (e) {
      setRunErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const wigleReady =
    form.wigle_api_name.trim().length > 0 && (wigleTokenSet || (tokenDirty && form.wigle_api_token.trim().length > 0))
  const wdgReady = wdgKeySet || (wdgKeyDirty && form.wdgwars_api_key.trim().length > 0)

  const bootPhase = bootStats?.boot_upload_phase ?? 'skipped'
  const bootInProgress = bootStats?.boot_upload_in_progress ?? false
  const showStartupPanel =
    bootInProgress ||
    bootPhase === 'in_progress' ||
    bootPhase === 'skipped_offline' ||
    bootPhase === 'complete' ||
    bootPhase === 'failed'

  const startupPanelTitle = (() => {
    switch (bootPhase) {
      case 'in_progress':
        return 'Startup upload in progress'
      case 'skipped_offline':
        return 'Startup upload skipped (offline)'
      case 'complete':
        return 'Startup upload finished'
      case 'failed':
        return 'Startup upload failed'
      default:
        return 'Startup upload'
    }
  })()

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Uploads</h2>
        <p className="muted page-head__lead">
          Pending WiGLE CSVs under <code>data/wigle/pending</code> upload to WiGLE.net and wdgwars.pl. Sidecar markers
          track each destination; files move to <code>uploaded/</code> only when every enabled destination succeeded.
        </p>
      </div>

      {showStartupPanel && bootStats && (
        <div className="panel boot-upload-panel">
          <h2>{startupPanelTitle}</h2>
          {bootPhase === 'in_progress' && (
            <p className="muted">
              Wardriving capture is deferred until pending sessions upload. The dashboard stays
              available during this step.
            </p>
          )}
          {bootPhase === 'skipped_offline' && (
            <p className="muted">
              No connectivity to configured upload hosts — boot upload was skipped quickly. Capture is starting without
              uploading pending files.
            </p>
          )}
          {bootPhase === 'complete' && (
            <p className="muted">
              Boot-time upload finished. Capture {bootStats.capture_started ? 'has started' : 'is starting'}.
            </p>
          )}
          {bootPhase === 'failed' && (
            <p className="error">{bootStats.boot_upload_error ?? 'Upload failed; see logs.'}</p>
          )}
          {bootStats.boot_upload_current_file && (
            <p className="muted mt-4">
              Current file: <code>{bootStats.boot_upload_current_file}</code>
            </p>
          )}
          {bootStats.boot_upload_sessions_total > 0 && (
            <p className="muted mt-4">
              Sessions: {bootStats.boot_upload_sessions_done} / {bootStats.boot_upload_sessions_total}
            </p>
          )}
          {bootStats.upload_last_message && <p className="muted mt-4">{bootStats.upload_last_message}</p>}
          {(bootPhase === 'complete' || bootPhase === 'failed') && (
            <p className="muted mt-4">
              WiGLE ok/fail {bootStats.upload_wigle_ok}/{bootStats.upload_wigle_failed} · WDGwars ok/fail{' '}
              {bootStats.upload_wdgwars_ok}/{bootStats.upload_wdgwars_failed}
            </p>
          )}
        </div>
      )}

      {err && <div className="panel panel--error">{err}</div>}
      {msg && (
        <div className="panel" role="status">
          <p className="msg-ok" style={{ margin: 0 }}>
            {msg}
          </p>
        </div>
      )}

      <div className="panel">
        <h2>WiGLE</h2>
        <p className="muted">
          Get your API name and token from your{' '}
          <a href={WIGLE_ACCOUNT_URL} target="_blank" rel="noopener noreferrer">
            WiGLE account page
          </a>
          .
        </p>
        <p className="muted mt-4" style={{ marginBottom: 0 }}>
          New to WiGLE? Join our team: DeFlock Wardrivers.
        </p>
        <p className="muted">
          Status: <strong>{wigleReady ? 'Configured' : 'Not configured'}</strong>
          {form.enable_wigle_upload && !wigleReady && (
            <span className="error"> — enable uploads only after saving name and token</span>
          )}
        </p>
        <label className="block mt-4">
          API name
          <input
            type="text"
            autoComplete="off"
            value={form.wigle_api_name}
            onChange={(e) => setForm((f) => ({ ...f, wigle_api_name: e.target.value }))}
            style={{ display: 'block', width: '100%', maxWidth: '28rem', marginTop: '0.35rem' }}
          />
        </label>
        <label className="block mt-4">
          API token
          <input
            type="password"
            autoComplete="off"
            value={form.wigle_api_token}
            placeholder={wigleTokenSet ? 'Leave blank to keep existing token' : 'Paste WiGLE API token'}
            onChange={(e) => {
              setTokenDirty(true)
              setForm((f) => ({ ...f, wigle_api_token: e.target.value }))
            }}
            style={{ display: 'block', width: '100%', maxWidth: '28rem', marginTop: '0.35rem' }}
          />
        </label>
        <label className="iface mt-4">
          <input
            type="checkbox"
            checked={form.enable_wigle_upload}
            onChange={(e) => setForm((f) => ({ ...f, enable_wigle_upload: e.target.checked }))}
          />
          Enable WiGLE uploads
        </label>
      </div>

      <div className="panel">
        <h2>WDGwars</h2>
        <p className="muted">
          Get your API key from your{' '}
          <a href={WDGWARS_PROFILE_URL} target="_blank" rel="noopener noreferrer">
            WDGwars profile
          </a>
          .
        </p>
        <p className="muted mt-4" style={{ marginBottom: 0 }}>
          New to the game? Join our team! Use invite code <code>5vzi_hZMhsnn</code> to join{' '}
          <strong>No Flocking Way</strong> on wdgwars.pl.
        </p>
        <p className="muted">
          Status: <strong>{wdgReady ? 'Configured' : 'Not configured'}</strong>
          {form.enable_wdgwars_upload && !wdgReady && (
            <span className="error"> — enable uploads only after saving an API key</span>
          )}
        </p>
        <label className="block mt-4">
          API key
          <input
            type="password"
            autoComplete="off"
            value={form.wdgwars_api_key}
            placeholder={wdgKeySet ? 'Leave blank to keep existing key' : 'Paste WDGwars API key'}
            onChange={(e) => {
              setWdgKeyDirty(true)
              setForm((f) => ({ ...f, wdgwars_api_key: e.target.value }))
            }}
            style={{ display: 'block', width: '100%', maxWidth: '28rem', marginTop: '0.35rem' }}
          />
        </label>
        <label className="iface mt-4">
          <input
            type="checkbox"
            checked={form.enable_wdgwars_upload}
            onChange={(e) => setForm((f) => ({ ...f, enable_wdgwars_upload: e.target.checked }))}
          />
          Enable WDGwars uploads
        </label>
      </div>

      <div className="panel">
        <label className="iface">
          <input
            type="checkbox"
            checked={form.upload_on_boot}
            onChange={(e) => setForm((f) => ({ ...f, upload_on_boot: e.target.checked }))}
          />
          Upload pending files when the daemon starts
        </label>
        <p className="muted mt-4" style={{ marginBottom: 0 }}>
          Credentials are stored in <code>data/config/app.json</code> on this machine. Restrict file permissions on
          shared systems.
        </p>
        <div className="toolbar mt-4">
          <button type="button" disabled={saveBusy} onClick={() => void save()}>
            {saveBusy ? 'Saving…' : 'Save settings'}
          </button>
          <button
            type="button"
            disabled={busy || bootInProgress}
            title={bootInProgress ? 'Boot upload is already running' : undefined}
            onClick={() => void runUpload()}
          >
            {busy ? 'Uploading…' : bootInProgress ? 'Boot upload running…' : 'Upload pending now'}
          </button>
          <button type="button" className="secondary" disabled={saveBusy} onClick={() => void load()}>
            Refresh
          </button>
        </div>
        {runErr && <p className="error mt-4">{runErr}</p>}
        {lastRun && (
          <p className="muted mt-4">
            Last manual run: WiGLE ok/fail {lastRun.wigle_ok}/{lastRun.wigle_failed} · WDGwars ok/fail{' '}
            {lastRun.wdgwars_ok}/{lastRun.wdgwars_failed}
          </p>
        )}
      </div>
    </div>
  )
}