import { useCallback, useEffect, useRef, useState } from 'react'
import { apiGet, apiPost } from '../api'
import { useApiLoad } from '../useApiLoad'

type ReconCatalog = {
  jobs: Array<{
    id: string
    label: string
    default_duration_secs: number
    output_subdir: string
    filename_suffix: string
  }>
  recent_files: Array<{ name: string; subdir: string }>
  last_output: string | null
  last_error: string | null
}

type ReconStartResp = { ok: boolean; started: boolean }
type ReconStopResp = { ok: boolean; stopped: boolean }

type ReconStatus = {
  running: boolean
  kind?: string
  duration_secs: number
  elapsed_secs: number
  mode?: string
  interfaces: Array<{ name: string; channel?: number; role: string }>
  hop_sequence?: number[]
  hop_index?: number
  ble_adapter?: string
  last_output?: string | null
  last_error?: string | null
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`
  return `${(n / (1024 * 1024)).toFixed(2)} MiB`
}

function parseChannelsInput(raw: string): number[] | undefined {
  const trimmed = raw.trim()
  if (!trimmed) return undefined
  const parts = trimmed.split(/[\s,]+/).filter(Boolean)
  const nums = parts.map((p) => Number.parseInt(p, 10)).filter((n) => !Number.isNaN(n) && n > 0)
  return nums.length > 0 ? nums : undefined
}

function reconDownloadPath(subdir: string, name: string): string {
  return `${subdir}/${name}`
}

export function ReconPage() {
  const [cat, setCat] = useState<ReconCatalog | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [durationSecs, setDurationSecs] = useState(60)
  const [channelsText, setChannelsText] = useState('')
  const [status, setStatus] = useState<ReconStatus | null>(null)
  const [lastPath, setLastPath] = useState<string | null>(null)
  const [runErr, setRunErr] = useState<string | null>(null)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const load = useCallback(async () => {
    setErr(null)
    try {
      const c = await apiGet<ReconCatalog>('/api/recon/catalog')
      setCat(c)
      setLastPath(c.last_output)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }, [])

  const pollStatus = useCallback(async () => {
    try {
      const s = await apiGet<ReconStatus>('/api/recon/status')
      setStatus(s)
      if (!s.running) {
        if (s.last_output) setLastPath(s.last_output)
        if (s.last_error) setRunErr(s.last_error)
        setBusy(false)
        if (pollRef.current) {
          clearInterval(pollRef.current)
          pollRef.current = null
        }
        await load()
      }
    } catch {
      /* keep polling */
    }
  }, [load])

  useApiLoad(() => void load(), [load])

  useEffect(() => {
    return () => {
      if (pollRef.current) clearInterval(pollRef.current)
    }
  }, [])

  const stopCapture = async () => {
    setRunErr(null)
    try {
      await apiPost<ReconStopResp>('/api/recon/stop', {})
      pollRef.current = setInterval(() => void pollStatus(), 1000)
      await pollStatus()
    } catch (e) {
      setRunErr(e instanceof Error ? e.message : String(e))
    }
  }

  const start = async (kind: 'probe' | 'raw' | 'ble') => {
    setRunErr(null)
    setBusy(true)
    setStatus(null)
    try {
      const channels = kind === 'ble' ? undefined : parseChannelsInput(channelsText)
      const body: {
        kind: string
        duration_secs: number
        channels?: number[]
      } = { kind, duration_secs: durationSecs }
      if (channels) body.channels = channels
      await apiPost<ReconStartResp>('/api/recon/start', body)
      pollRef.current = setInterval(() => void pollStatus(), 1000)
      await pollStatus()
    } catch (e) {
      setRunErr(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  if (err) {
    return (
      <div className="stack">
        <div className="page-head">
          <h2 className="page-head__title">Recon</h2>
        </div>
        <div className="panel panel--error">{err}</div>
      </div>
    )
  }
  if (!cat) {
    return (
      <div className="page-head">
        <h2 className="page-head__title">Recon</h2>
        <p className="muted page-head__lead">Loading…</p>
      </div>
    )
  }

  const live = status?.running ? status : null

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Recon</h2>
        <p className="muted page-head__lead">
          PCAP-NG capture jobs with live channel status. WiFi jobs use your active capture
          interfaces; BLE HCI recon needs <code>btmon</code> (BlueZ).
        </p>
      </div>
      <div className="panel">
        <h2>Capture</h2>
        <p className="muted">
          WiFi writes under <code>data/recon/wifiprobes/</code> plus a{' '}
          <code>*_channels.jsonl</code> sidecar. BLE writes under <code>data/recon/ble/</code>.
          Wardriving capture pauses during WiFi recon; BLE scan pauses during BLE recon.
        </p>
        <div className="row">
          <label>
            Duration (seconds){' '}
            <input
              type="number"
              min={1}
              max={3600}
              value={durationSecs}
              onChange={(ev) => setDurationSecs(Number(ev.target.value) || 60)}
              className="input-medium"
            />
          </label>
        </div>
        <div className="row mt-4">
          <label className="stack gap-1">
            WiFi channels (optional)
            <input
              type="text"
              value={channelsText}
              onChange={(ev) => setChannelsText(ev.target.value)}
              placeholder="e.g. 1, 6, 11, 36 — empty = channel plan hop (200 ms)"
              className="input-wide"
              disabled={busy}
            />
          </label>
        </div>
        <p className="muted small">
          Up to one channel per adapter without hopping. More channels than adapters pins the
          first N and hops the rest (200 ms dwell).
        </p>
        <div className="toolbar mt-4">
          <button type="button" disabled={busy} onClick={() => void start('probe')}>
            {busy ? 'Capturing…' : 'Probe recon (filtered)'}
          </button>
          <button type="button" disabled={busy} className="secondary" onClick={() => void start('raw')}>
            Raw recon (all frames)
          </button>
          <button type="button" disabled={busy} className="secondary" onClick={() => void start('ble')}>
            BLE HCI recon
          </button>
          <button type="button" className="secondary" disabled={busy} onClick={() => void load()}>
            Refresh list
          </button>
        </div>
        {runErr && <p className="error mt-4">{runErr}</p>}
        {cat.last_error && !runErr && <p className="error mt-4">Last error: {cat.last_error}</p>}
        {(lastPath || cat.last_output) && (
          <p className="muted mt-4">
            Last file: <code>{lastPath ?? cat.last_output}</code>
          </p>
        )}
      </div>

      {live && (
        <div className="panel">
          <div className="toolbar mb-4">
            <button type="button" onClick={() => void stopCapture()}>
              Stop capture
            </button>
          </div>
          <h3>Live status</h3>
          <p className="muted">
            {live.kind ?? 'recon'} · {live.elapsed_secs}s / {live.duration_secs}s
            {live.mode ? ` · mode ${live.mode}` : ''}
            {live.ble_adapter ? ` · ${live.ble_adapter}` : ''}
          </p>
          {live.hop_sequence && live.hop_sequence.length > 0 && (
            <p className="muted small">
              Hop sequence: {live.hop_sequence.join(', ')}
              {live.hop_index != null ? ` (index ${live.hop_index})` : ''}
            </p>
          )}
          {live.interfaces.length > 0 ? (
            <table className="mt-4">
              <thead>
                <tr>
                  <th>Interface</th>
                  <th>Channel</th>
                  <th>Role</th>
                </tr>
              </thead>
              <tbody>
                {live.interfaces.map((i) => (
                  <tr key={i.name}>
                    <td>
                      <code>{i.name}</code>
                    </td>
                    <td>{i.channel ?? '—'}</td>
                    <td>{i.role}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : live.ble_adapter ? (
            <p className="muted mt-4">Capturing HCI on {live.ble_adapter}…</p>
          ) : null}
        </div>
      )}

      <div className="panel">
        <h3>Job catalog</h3>
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>Label</th>
              <th>Default (s)</th>
              <th>Output</th>
            </tr>
          </thead>
          <tbody>
            {cat.jobs.map((j) => (
              <tr key={j.id}>
                <td>
                  <code>{j.id}</code>
                </td>
                <td>{j.label}</td>
                <td>{j.default_duration_secs}</td>
                <td className="muted">
                  recon/{j.output_subdir}/*_{j.filename_suffix}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <p className="muted small mt-4">
          WiFi recon also writes <code>*_channels.jsonl</code> beside each PCAP-NG.
        </p>
      </div>

      <div className="panel">
        <h3>Recent captures</h3>
        {cat.recent_files.length === 0 ? (
          <p className="muted">No PCAP-NG files yet.</p>
        ) : (
          <table>
            <thead>
              <tr>
                <th>Location</th>
                <th>File</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {cat.recent_files.map((f) => {
                const rel = reconDownloadPath(f.subdir, f.name)
                return (
                  <tr key={rel}>
                    <td className="muted">
                      <code>recon/{f.subdir}/</code>
                    </td>
                    <td>
                      <code>{f.name}</code>
                    </td>
                    <td>
                      <a
                        className="button"
                        href={`/api/files/recon/${encodeURIComponent(rel)}`}
                        download={f.name}
                      >
                        Download
                      </a>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        )}
        <p className="muted small mt-4">
          Large files may hit the download cap ({fmtBytes(100 * 1024 * 1024)}).
        </p>
      </div>
    </div>
  )
}
