import { useCallback, useState } from 'react'
import { Link } from 'react-router-dom'
import { useApiLoad } from '../useApiLoad'
import { usePollApi } from '../usePollApi'
import { apiGet, apiPost } from '../api'

type CotravelCfg = {
  enabled: boolean
  min_duration_s: number
  min_track_distance_m: number
  min_rssi: number
  max_gap_s: number
  min_sightings: number
  max_tracked_macs: number
  alert_cooldown_s: number
  wifi_probes: boolean
  ble_adverts: boolean
}

type ConfigSlice = {
  cotravel: CotravelCfg
}

type CotravelStatus = {
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

const defaultCotravel = (): CotravelCfg => ({
  enabled: false,
  min_duration_s: 90,
  min_track_distance_m: 150,
  min_rssi: -82,
  max_gap_s: 60,
  min_sightings: 6,
  max_tracked_macs: 384,
  alert_cooldown_s: 600,
  wifi_probes: true,
  ble_adverts: true,
})

export function CoTravelPage() {
  const [ct, setCt] = useState<CotravelCfg>(defaultCotravel)
  const [status, setStatus] = useState<CotravelStatus | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)

  const loadConfig = useCallback(async () => {
    setErr(null)
    try {
      const c = await apiGet<ConfigSlice>('/api/config')
      setCt(c.cotravel ?? defaultCotravel())
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }, [])

  const pollStatus = useCallback(async () => {
    try {
      const s = await apiGet<CotravelStatus>('/api/cotravel/status?limit=200')
      setStatus(s)
    } catch {
      /* ignore while backend down */
    }
  }, [])

  useApiLoad(() => void loadConfig(), [loadConfig])
  usePollApi(() => void pollStatus(), 3000, [pollStatus])

  const save = async () => {
    setMsg(null)
    setErr(null)
    try {
      await apiPost('/api/config', {
        cotravel: { ...ct },
      })
      setMsg('Saved to data/config/app.json')
      void loadConfig()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  const clearSession = async () => {
    setMsg(null)
    setErr(null)
    try {
      await apiPost<{ ok: boolean }>('/api/cotravel/clear', {})
      setMsg('Co-travel session state cleared (SQLite history kept).')
      void pollStatus()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Co-travel</h2>
        <p className="muted page-head__lead">
          Co-travel (untested / experimental feature) correlates strong RSSI sightings with your GPS motion. Tune thresholds and use the map suspect
          layer while zooming. Personal devices: <Link to="/privacy">Privacy</Link>. Flock false-positive MACs:{' '}
          <Link to="/detections">Detections</Link>.
        </p>
      </div>
      {err && <div className="panel panel--error">{err}</div>}
      {msg && (
        <div className="panel" role="status">
          <p className="msg-ok" style={{ margin: 0 }}>
            {msg}
          </p>
        </div>
      )}

      <div className="panel">
        <h2>Thresholds</h2>
        <div className="toolbar toolbar--tight">
          <label>
            <input
              type="checkbox"
              checked={ct.enabled}
              onChange={(e) => setCt({ ...ct, enabled: e.target.checked })}
            />{' '}
            Enabled
          </label>
          <label>
            <input
              type="checkbox"
              checked={ct.wifi_probes}
              onChange={(e) => setCt({ ...ct, wifi_probes: e.target.checked })}
            />{' '}
            WiFi probes
          </label>
          <label>
            <input
              type="checkbox"
              checked={ct.ble_adverts}
              onChange={(e) => setCt({ ...ct, ble_adverts: e.target.checked })}
            />{' '}
            BLE adverts
          </label>
        </div>
        <div className="toolbar toolbar--tight mt-4">
          <label>
            Min duration (s){' '}
            <input
              type="number"
              min={10}
              value={ct.min_duration_s}
              onChange={(e) => setCt({ ...ct, min_duration_s: parseInt(e.target.value, 10) || 0 })}
            />
          </label>
          <label>
            Min path (m){' '}
            <input
              type="number"
              min={10}
              step={10}
              value={ct.min_track_distance_m}
              onChange={(e) => setCt({ ...ct, min_track_distance_m: parseFloat(e.target.value) || 0 })}
            />
          </label>
          <label>
            Min RSSI (dBm){' '}
            <input
              type="number"
              max={-20}
              min={-100}
              value={ct.min_rssi}
              onChange={(e) => setCt({ ...ct, min_rssi: parseInt(e.target.value, 10) || -100 })}
            />
          </label>
          <label>
            Max gap (s){' '}
            <input
              type="number"
              min={5}
              value={ct.max_gap_s}
              onChange={(e) => setCt({ ...ct, max_gap_s: parseInt(e.target.value, 10) || 0 })}
            />
          </label>
          <label>
            Min sightings{' '}
            <input
              type="number"
              min={2}
              value={ct.min_sightings}
              onChange={(e) => setCt({ ...ct, min_sightings: parseInt(e.target.value, 10) || 0 })}
            />
          </label>
          <label>
            Max tracked MACs{' '}
            <input
              type="number"
              min={32}
              value={ct.max_tracked_macs}
              onChange={(e) => setCt({ ...ct, max_tracked_macs: parseInt(e.target.value, 10) || 32 })}
            />
          </label>
          <label>
            Alert cooldown (s){' '}
            <input
              type="number"
              min={30}
              value={ct.alert_cooldown_s}
              onChange={(e) => setCt({ ...ct, alert_cooldown_s: parseInt(e.target.value, 10) || 30 })}
            />
          </label>
        </div>
        <div className="toolbar mt-4">
          <button type="button" onClick={() => void save()}>
            Save config
          </button>
          <button type="button" className="secondary" onClick={() => void clearSession()}>
            Clear session state
          </button>
        </div>
      </div>

      <div className="panel">
        <h2>Live suspects</h2>
        <p className="muted">
          In-progress streaks (also drawn on the Map as amber markers). Score estimates proximity to firing.
        </p>
        {status && (
          <p className="muted">
            Engine: {status.enabled ? 'on' : 'off'} · tracked MACs: {status.tracked}
          </p>
        )}
        {status && status.suspects.length === 0 && <p className="muted">No suspects in memory.</p>}
        {status && status.suspects.length > 0 && (
          <div className="data-table-wrap">
            <table>
              <thead>
                <tr>
                  <th>MAC</th>
                  <th>Score</th>
                  <th>Sightings</th>
                  <th>Streak s</th>
                  <th>Path m</th>
                  <th>RSSI</th>
                  <th>Cooldown</th>
                </tr>
              </thead>
              <tbody>
                {status.suspects.map((s) => (
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
    </div>
  )
}
