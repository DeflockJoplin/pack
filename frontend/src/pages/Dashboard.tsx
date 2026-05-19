import { useCallback, useEffect, useRef, useState } from 'react'
import { usePollApi } from '../usePollApi'
import { Link } from 'react-router-dom'
import { apiGet } from '../api'
import { useVisibilityPollMs } from '../useDocumentVisible'
import {
  audioContextState,
  playDetectionChirp,
  unlockDetectionAudio,
} from '../detectionChirp'

type Stats = {
  boot_upload_in_progress: boolean
  capture_started: boolean
  active_capture_interfaces: string[]
  gps_fix: boolean
  gps_ok_to_log: boolean
  gps_connected: boolean
  last_pcap_error: string | null
  ble_last_error: string | null
  alerts_fired_total: number
  flock_wifi_alerts_fired: number
  flock_wifi_alerts_method_1: number
  flock_wifi_alerts_method_2: number
  flock_wifi_alerts_method_3: number
  flock_wifi_ie_sig_match_builtin_default: number
  flock_wifi_ie_sig_match_builtin_alt_linux: number
  flock_wifi_ie_sig_match_config: number
  flock_wifi_ie_sig_computed_builtin_default: number
  flock_wifi_ie_sig_computed_builtin_alt_linux: number
  flock_wifi_ie_sig_computed_other: number
  wifi_csv_rows: number
  probe_csv_rows: number
  ble_csv_rows: number
  home_geofence_suppressing: boolean
  home_geo_radius_m: number
  home_geo_center_set: boolean
  ble_alerts_fired: number
  cotravel_alerts_fired: number
  ssid_watch_probe_alerts_fired: number
  ssid_watch_beacon_alerts_fired: number
}

type CotravelStatusLite = {
  enabled: boolean
  tracked: number
  suspects: Array<{
    mac: string
    lat: number
    lon: number
    rssi: number
    sightings: number
    streak_duration_s: number
    user_path_m: number
    score: number
    in_cooldown: boolean
  }>
}

type ConfigMbtiles = {
  mbtiles_path?: string | null
}

type CotravelRecentPin = {
  lat: number
  lon: number
  mac: string
  t_ms: number
  source: string
  duration_s: number
  track_distance_m: number
}

type FlockRecentAlert = {
  t_ms: number
  lat: number | null
  lon: number | null
  mac: string
  signal_kind: string
  method_id: number
  method_label: string
  batch_id: number
  unique_macs_in_batch: number
}

export function Dashboard() {
  const [stats, setStats] = useState<Stats | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const statsLoadedOnce = useRef(false)
  const [cotravelMeta, setCotravelMeta] = useState<CotravelStatusLite | null>(null)
  const [cotravelRecent, setCotravelRecent] = useState<CotravelRecentPin[]>([])
  const [flockRecent, setFlockRecent] = useState<FlockRecentAlert[]>([])
  const [mbtiles, setMbtiles] = useState(false)
  const [detectionBanner, setDetectionBanner] = useState<string | null>(null)
  const [chirpEnabled, setChirpEnabled] = useState(true)
  const [audioReady, setAudioReady] = useState(false)

  const prevAlertsRef = useRef({ flock: 0, ble: 0, cotravel: 0, ssid: 0, primed: false })
  const dashboardPollMs = useVisibilityPollMs(4000)

  const tryUnlockAudio = useCallback(async () => {
    await unlockDetectionAudio()
    setAudioReady(audioContextState() === 'running')
  }, [])

  usePollApi(async () => {
    const [statsResult, cotravelResult, configResult] = await Promise.allSettled([
      apiGet<Stats>('/api/stats'),
      apiGet<CotravelStatusLite>('/api/cotravel/status?limit=50'),
      apiGet<ConfigMbtiles>('/api/config'),
    ])
    if (statsResult.status === 'fulfilled') {
      statsLoadedOnce.current = true
      setStats(statsResult.value)
      setErr(null)
    } else if (!statsLoadedOnce.current) {
      const e = statsResult.reason
      setErr(e instanceof Error ? e.message : String(e))
    }
    if (cotravelResult.status === 'fulfilled') {
      setCotravelMeta(cotravelResult.value)
    } else {
      setCotravelMeta(null)
    }
    if (configResult.status === 'fulfilled') {
      setMbtiles(Boolean(configResult.value.mbtiles_path?.trim()))
    }
  }, dashboardPollMs, [dashboardPollMs])

  useEffect(() => {
    if (!stats) return
    queueMicrotask(() => {
      const p = prevAlertsRef.current
      if (!p.primed) {
        p.flock = stats.flock_wifi_alerts_fired
        p.ble = stats.ble_alerts_fired
        p.cotravel = stats.cotravel_alerts_fired
        p.ssid =
          stats.ssid_watch_probe_alerts_fired + stats.ssid_watch_beacon_alerts_fired
        p.primed = true
        return
      }
      const parts: string[] = []
      if (stats.flock_wifi_alerts_fired > p.flock) {
        parts.push(`WiFi Flock +${stats.flock_wifi_alerts_fired - p.flock}`)
      }
      if (stats.ble_alerts_fired > p.ble) {
        parts.push(`BLE Flock +${stats.ble_alerts_fired - p.ble}`)
      }
      if (stats.cotravel_alerts_fired > p.cotravel) {
        parts.push(`Co-travel +${stats.cotravel_alerts_fired - p.cotravel}`)
      }
      const ssidTotal =
        stats.ssid_watch_probe_alerts_fired + stats.ssid_watch_beacon_alerts_fired
      if (ssidTotal > p.ssid) {
        parts.push(`SSID watch +${ssidTotal - p.ssid}`)
      }
      p.flock = stats.flock_wifi_alerts_fired
      p.ble = stats.ble_alerts_fired
      p.cotravel = stats.cotravel_alerts_fired
      p.ssid = ssidTotal
      if (parts.length > 0) {
        setDetectionBanner(parts.join(' · '))
        if (chirpEnabled) {
          playDetectionChirp()
        }
      }
    })
  }, [stats, chirpEnabled])

  usePollApi(async () => {
    try {
      const r = await apiGet<{ recent_fires: CotravelRecentPin[] }>('/api/cotravel/recent')
      setCotravelRecent(r.recent_fires.slice().sort((a, b) => b.t_ms - a.t_ms).slice(0, 20))
    } catch {
      setCotravelRecent([])
    }
  }, dashboardPollMs, [dashboardPollMs])

  usePollApi(async () => {
    try {
      const r = await apiGet<{ alerts: FlockRecentAlert[] }>('/api/flock/recent')
      setFlockRecent(r.alerts.slice().sort((a, b) => b.t_ms - a.t_ms).slice(0, 20))
    } catch {
      setFlockRecent([])
    }
  }, dashboardPollMs, [dashboardPollMs])

  const pinsSorted = cotravelRecent
  const flockSorted = flockRecent
  const suspects = cotravelMeta?.suspects ?? []

  if (err && !stats) {
    return (
      <div className="stack">
        <div className="page-head">
          <h2 className="page-head__title">Dashboard</h2>
        </div>
        <div className="panel panel--error">
          Could not load <code>/api/stats</code>: {err}
          <p className="muted" style={{ marginTop: '0.75rem' }}>
            Start the backend: <code>sudo cargo run -p pack</code> (from repo root), then reload. Vite proxies{' '}
            <code>/api</code> to <code>127.0.0.1:8787</code>.
          </p>
        </div>
      </div>
    )
  }

  return (
    <div
      className="stack dashboard dashboard--glance"
      onPointerDownCapture={() => {
        void tryUnlockAudio()
      }}
    >
      {detectionBanner && (
        <div className="dashboard-glance__detection-banner" role="alert">
          <span className="dashboard-glance__detection-text">Detection: {detectionBanner}</span>
          <button type="button" className="dashboard-glance__dismiss" onClick={() => setDetectionBanner(null)}>
            Dismiss
          </button>
        </div>
      )}

      <div className="page-head dashboard-glance__head">
        <h2 className="page-head__title dashboard-glance__title">Dashboard</h2>
        <p className="muted page-head__lead dashboard-glance__lead">
          <Link to="/map">Map</Link> · <Link to="/cotravel">Co-travel</Link> · <Link to="/privacy">Privacy</Link> ·
          Wi‑Fi map points = pending WiGLE CSV sample.
        </p>
        {stats?.boot_upload_in_progress && (
          <p className="muted dashboard__banner" role="status">
            Uploads in progress — capture deferred. <Link to="/uploads">Uploads</Link>
          </p>
        )}
      </div>

      {stats && (
        <>
          <div className="dashboard-glance__grid">
            <div className="dashboard-glance__card">
              <div className="dashboard-glance__card-label">WiFi CSV rows</div>
              <div className="dashboard-glance__card-value">{stats.wifi_csv_rows}</div>
              <div className="dashboard-glance__card-foot">Probe CSV rows: {stats.probe_csv_rows}</div>
            </div>
            <div className="dashboard-glance__card">
              <div className="dashboard-glance__card-label">BLE CSV rows</div>
              <div className="dashboard-glance__card-value">{stats.ble_csv_rows}</div>
            </div>
            <div className="dashboard-glance__card">
              <div className="dashboard-glance__card-label">GPS</div>
              <div className="dashboard-glance__card-value dashboard-glance__card-value--sm">
                {stats.gps_connected ? 'ON' : 'OFF'} · Fix {stats.gps_fix ? 'Yes' : 'No'}
              </div>
              <div className="dashboard-glance__card-foot muted">
                log {stats.gps_ok_to_log ? 'OK' : 'no'}
              </div>
            </div>
            <div className="dashboard-glance__card">
              <div className="dashboard-glance__card-label">Home zone</div>
              <div className="dashboard-glance__card-value dashboard-glance__card-value--sm">
                {stats.home_geo_center_set ? (
                  <>
                    Geofence status: <strong>{stats.home_geofence_suppressing ? 'IN' : 'out'}</strong>
                  </>
                ) : (
                  <>not set</>
                )}
              </div>
              <div className="dashboard-glance__card-foot">
                <Link to="/privacy">Edit</Link> · r={stats.home_geo_radius_m}m
              </div>
            </div>
            <div className="dashboard-glance__card">
              <div className="dashboard-glance__card-label">Alerts (total)</div>
              <div className="dashboard-glance__card-value">{stats.alerts_fired_total}</div>
              <div className="dashboard-glance__card-foot">
                Flock {stats.flock_wifi_alerts_fired} · BLE {stats.ble_alerts_fired} · CT{' '}
                {stats.cotravel_alerts_fired} · SSID {stats.ssid_watch_probe_alerts_fired}/
                {stats.ssid_watch_beacon_alerts_fired}
                {(stats.flock_wifi_ie_sig_match_builtin_default > 0 ||
                  stats.flock_wifi_ie_sig_match_builtin_alt_linux > 0) && (
                  <>
                    {' '}
                    · IE sig m2/3: def {stats.flock_wifi_ie_sig_match_builtin_default} / alt{' '}
                    {stats.flock_wifi_ie_sig_match_builtin_alt_linux}
                  </>
                )}
              </div>
            </div>
            <div className="dashboard-glance__card">
              <div className="dashboard-glance__card-label">Capture</div>
              <div className="dashboard-glance__card-value dashboard-glance__card-value--sm">
                {stats.active_capture_interfaces.length ? stats.active_capture_interfaces.join(', ') : '—'}
              </div>
              <div className="dashboard-glance__card-foot">MBTiles: {mbtiles ? 'yes' : 'no'}</div>
            </div>
          </div>

          <div className="panel dashboard-glance__controls">
            <label className="dashboard-glance__check">
              <input
                type="checkbox"
                checked={chirpEnabled}
                onChange={(e) => setChirpEnabled(e.target.checked)}
              />{' '}
              Alert chirp (after tap to unlock audio)
            </label>
            {!audioReady && (
              <span className="dashboard-glance__audio-hint">Tap anywhere on the dashboard to enable sounds.</span>
            )}
          </div>

          {stats.last_pcap_error && (
            <p className="error dashboard-glance__error-line">WiFi capture: {stats.last_pcap_error}</p>
          )}
          {stats.ble_last_error && <p className="error dashboard-glance__error-line">BLE: {stats.ble_last_error}</p>}
        </>
      )}

      <div className="dashboard__cotravel-grid dashboard-glance__cotravel">
        <div className="panel">
          <h2 className="dashboard-glance__h2">Co-travel live suspects</h2>
          <p className="muted dashboard-glance__muted">
            Amber markers on the <Link to="/map">map</Link>.
          </p>
          {cotravelMeta && (
            <p className="muted dashboard-glance__muted">
              Engine: {cotravelMeta.enabled ? 'on' : 'off'} · tracked MACs: {cotravelMeta.tracked}
            </p>
          )}
          {suspects.length === 0 && <p className="muted dashboard-glance__muted">No suspects in memory.</p>}
          {suspects.length > 0 && (
            <div className="data-table-wrap">
              <table className="dashboard-glance__table">
                <thead>
                  <tr>
                    <th>MAC</th>
                    <th>Score</th>
                    <th>Sightings</th>
                    <th>Streak s</th>
                    <th>Path m</th>
                    <th>RSSI</th>
                    <th>CD</th>
                  </tr>
                </thead>
                <tbody>
                  {suspects.map((s) => (
                    <tr key={s.mac}>
                      <td className="font-mono">{s.mac}</td>
                      <td>{s.score.toFixed(2)}</td>
                      <td>{s.sightings}</td>
                      <td>{s.streak_duration_s.toFixed(0)}</td>
                      <td>{s.user_path_m.toFixed(0)}</td>
                      <td>{s.rssi}</td>
                      <td>{s.in_cooldown ? 'yes' : ''}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>

        <div className="panel">
          <h2 className="dashboard-glance__h2">Co-travel recent alerts</h2>
          <p className="muted dashboard-glance__muted">Short-lived ring buffer (~48); map uses session SQLite.</p>
          {pinsSorted.length === 0 && <p className="muted dashboard-glance__muted">No recent fires in memory.</p>}
          {pinsSorted.length > 0 && (
            <ul className="dashboard__alert-list dashboard-glance__alert-list">
              {pinsSorted.map((p) => (
                <li key={`${p.mac}-${p.t_ms}`}>
                  <strong className="font-mono">{p.mac}</strong> · {new Date(p.t_ms).toLocaleString()} ·{' '}
                  {p.duration_s.toFixed(0)}s · ~{p.track_distance_m.toFixed(0)} m · {p.source}
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="panel">
          <h2 className="dashboard-glance__h2">Recent Flock</h2>
          <p className="muted dashboard-glance__muted">Last ~2 minutes; red markers on map are session CSV.</p>
          {flockSorted.length === 0 && (
            <p className="muted dashboard-glance__muted">No recent Flock alerts.</p>
          )}
          {flockSorted.length > 0 && (
            <ul className="dashboard__alert-list dashboard-glance__alert-list">
              {flockSorted.map((a) => (
                <li key={`${a.mac}-${a.t_ms}-${a.method_id}`}>
                  <strong className="font-mono">{a.mac}</strong> · {a.signal_kind} · {a.method_label}
                  {a.unique_macs_in_batch > 1 && (
                    <> · total possible cameras: {a.unique_macs_in_batch}</>
                  )}
                  <br />
                  <span className="muted">{new Date(a.t_ms).toLocaleString()}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  )
}
