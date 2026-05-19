import { useCallback, useState } from 'react'
import { Link } from 'react-router-dom'
import { useApiLoad } from '../useApiLoad'
import { usePollApi } from '../usePollApi'
import { apiGet, apiPost } from '../api'
import { macsToText, ssidsToText, textToMacs, textToSsids } from '../macHelpers'

type HomeGeo = {
  lat?: number | null
  lon?: number | null
  radius_m?: number | null
}

type ConfigSlice = {
  home_geo: HomeGeo
  persist_gps_track_sqlite?: boolean
  wigle_exclude_bssids?: string[]
  privacy_exclude_ssids?: string[]
}

type StatsSlice = {
  data_root: string
  home_geofence_suppressing: boolean
  home_geo_radius_m: number
  home_geo_center_set: boolean
  gps_fix?: boolean
  current_lat?: number | null
  current_lon?: number | null
}

type ConfigReloadResponse = {
  ok: boolean
  config_path: string
  privacy_exclude_ssids: number
  wigle_exclude_bssids: number
}

function parseOptFloat(s: string): number | null {
  const t = s.trim()
  if (t === '') return null
  const n = Number(t)
  return Number.isFinite(n) ? n : null
}

function parseOptUInt(s: string): number | null {
  const t = s.trim()
  if (t === '') return null
  const n = parseInt(t, 10)
  return Number.isFinite(n) && n >= 0 ? n : null
}

function formatGpsCoord(n: number): string {
  return n.toFixed(6)
}

function exclusionSummary(ssidCount: number, macCount: number): string {
  const parts: string[] = []
  if (ssidCount > 0) {
    parts.push(`${ssidCount} excluded SSID${ssidCount === 1 ? '' : 's'}`)
  }
  if (macCount > 0) {
    parts.push(`${macCount} excluded MAC${macCount === 1 ? '' : 's'}`)
  }
  return parts.length > 0 ? parts.join(', ') : 'no exclusions'
}

export function PrivacyPage() {
  const [latIn, setLatIn] = useState('')
  const [lonIn, setLonIn] = useState('')
  const [radiusIn, setRadiusIn] = useState('')
  const [persistGpsSqlite, setPersistGpsSqlite] = useState(false)
  const [excludeMacs, setExcludeMacs] = useState('')
  const [excludeSsids, setExcludeSsids] = useState('')
  const [dataRoot, setDataRoot] = useState<string | null>(null)
  const [stats, setStats] = useState<StatsSlice | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [reloadBusy, setReloadBusy] = useState(false)

  const applyConfig = useCallback((c: ConfigSlice) => {
    const h = c.home_geo ?? {}
    setLatIn(h.lat != null ? String(h.lat) : '')
    setLonIn(h.lon != null ? String(h.lon) : '')
    setRadiusIn(h.radius_m != null ? String(h.radius_m) : '')
    setPersistGpsSqlite(c.persist_gps_track_sqlite ?? false)
    setExcludeMacs(macsToText(c.wigle_exclude_bssids ?? []))
    setExcludeSsids(ssidsToText(c.privacy_exclude_ssids ?? []))
    return {
      ssidCount: (c.privacy_exclude_ssids ?? []).length,
      macCount: (c.wigle_exclude_bssids ?? []).length,
    }
  }, [])

  const loadConfig = useCallback(async () => {
    setErr(null)
    try {
      const c = await apiGet<ConfigSlice>('/api/config')
      applyConfig(c)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [applyConfig])

  useApiLoad(() => void loadConfig(), [loadConfig])

  usePollApi(async () => {
    try {
      const s = await apiGet<StatsSlice>('/api/stats')
      setStats(s)
      setDataRoot(s.data_root)
    } catch {
      setStats(null)
    }
  }, 4000, [])

  const configPathLabel = dataRoot ? `${dataRoot}/config/app.json` : 'config/app.json'

  const save = async () => {
    setMsg(null)
    setErr(null)
    const lat = parseOptFloat(latIn)
    const lon = parseOptFloat(lonIn)
    const radius_m = parseOptUInt(radiusIn)
    if (latIn.trim() !== '' && lat === null) {
      setErr('Latitude must be a valid number. Fix home zone fields or clear them to save exclusions.')
      return
    }
    if (lonIn.trim() !== '' && lon === null) {
      setErr('Longitude must be a valid number. Fix home zone fields or clear them to save exclusions.')
      return
    }
    if (radiusIn.trim() !== '' && radius_m === null) {
      setErr(
        'Radius must be a non-negative integer (meters). Fix home zone fields or clear them to save exclusions.',
      )
      return
    }
    const ssids = textToSsids(excludeSsids)
    const macs = textToMacs(excludeMacs)
    try {
      await apiPost<{ ok: boolean }>('/api/config', {
        home_geo: {
          lat: latIn.trim() === '' ? null : lat,
          lon: lonIn.trim() === '' ? null : lon,
          radius_m: radiusIn.trim() === '' ? null : radius_m,
        },
        persist_gps_track_sqlite: persistGpsSqlite,
        wigle_exclude_bssids: macs,
        privacy_exclude_ssids: ssids,
      })
      setMsg(`Saved to ${configPathLabel} — ${exclusionSummary(ssids.length, macs.length)}.`)
      void loadConfig()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  const fillFromCurrentGpsFix = async () => {
    setMsg(null)
    setErr(null)
    try {
      const s = await apiGet<StatsSlice>('/api/stats')
      setStats(s)
      setDataRoot(s.data_root)
      if (!s.gps_fix) {
        setErr('No usable GPS fix — check gpsd.')
        return
      }
      const lat = s.current_lat
      const lon = s.current_lon
      if (lat == null || lon == null || !Number.isFinite(lat) || !Number.isFinite(lon)) {
        setErr('No usable GPS fix — check gpsd.')
        return
      }
      setLatIn(formatGpsCoord(lat))
      setLonIn(formatGpsCoord(lon))
      setMsg('Filled latitude and longitude from current GPS fix. Press Save to persist.')
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  const reloadFromDisk = async () => {
    setMsg(null)
    setErr(null)
    setReloadBusy(true)
    try {
      const r = await apiPost<ConfigReloadResponse>('/api/config/reload', {})
      const c = await apiGet<ConfigSlice>('/api/config')
      applyConfig(c)
      setMsg(
        `Reloaded from ${r.config_path} — ${exclusionSummary(r.privacy_exclude_ssids, r.wigle_exclude_bssids)}.`,
      )
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setReloadBusy(false)
    }
  }

  if (loading && !err) {
    return (
      <div className="page-head">
        <h2 className="page-head__title">Privacy</h2>
        <p className="muted page-head__lead">Loading…</p>
      </div>
    )
  }

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Privacy</h2>
        <p className="muted page-head__lead">
          Home zone, location persistence, and device exclusions. Excluded MACs and SSIDs are dropped from all capture
          (logs, Nearby, detectors, co-travel). For Flock false positives on fixed infrastructure, use{' '}
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

      {stats && (
        <div className="panel">
          <h3>Home zone — live status</h3>
          <p className="muted" style={{ marginTop: 0 }}>
            <strong>Inside home zone (WiGLE suppressed):</strong>{' '}
            {stats.home_geofence_suppressing ? 'yes' : 'no'}
          </p>
          <p className="muted">
            <strong>Center configured:</strong> {stats.home_geo_center_set ? 'yes' : 'no'} ·{' '}
            <strong>Radius (m):</strong> {stats.home_geo_radius_m}
            {!stats.home_geo_center_set && ' — set lat/lon below to enable.'}
          </p>
          {stats.gps_fix &&
            stats.current_lat != null &&
            stats.current_lon != null &&
            Number.isFinite(stats.current_lat) &&
            Number.isFinite(stats.current_lon) && (
              <p className="muted">
                <strong>Current fix:</strong> {formatGpsCoord(stats.current_lat)},{' '}
                {formatGpsCoord(stats.current_lon)}
              </p>
            )}
        </div>
      )}

      <div className="panel">
        <h3>Home zone</h3>
        <p className="muted">
          WiGLE CSV <strong>Wi‑Fi and BLE rows</strong> are suppressed when your GPS fix is inside this circle. Flock,
          co-travel, and other paths keep running. Leave latitude and longitude empty to disable the geofence.
        </p>
        <div className="toolbar toolbar--tight mt-4" style={{ flexDirection: 'column', alignItems: 'stretch', gap: '0.75rem' }}>
          <label>
            Latitude{' '}
            <input
              type="text"
              inputMode="decimal"
              value={latIn}
              onChange={(e) => setLatIn(e.target.value)}
              placeholder="e.g. 37.44"
              className="input-medium"
            />
          </label>
          <label>
            Longitude{' '}
            <input
              type="text"
              inputMode="decimal"
              value={lonIn}
              onChange={(e) => setLonIn(e.target.value)}
              placeholder="e.g. -122.16"
              className="input-medium"
            />
          </label>
          <label>
            Radius (m){' '}
            <input
              type="number"
              min={10}
              max={200000}
              value={radiusIn}
              onChange={(e) => setRadiusIn(e.target.value)}
              placeholder="default 805 m if empty"
              className="input-medium"
            />
          </label>
        </div>
        <div className="toolbar toolbar--tight mt-4">
          <button
            type="button"
            className="secondary"
            disabled={!stats?.gps_fix}
            title={stats?.gps_fix ? undefined : 'No usable GPS fix — check gpsd'}
            onClick={() => void fillFromCurrentGpsFix()}
          >
            Use current GPS fix
          </button>
        </div>
      </div>

      <div className="panel">
        <h3>Location data</h3>
        <p className="muted">
          The in-memory map track is always updated from <code>gpsd</code>. Optionally, fixes can also be appended to{' '}
          <code>wardrive.sqlite</code> (<code>gps_track_point</code>).
        </p>
        <label className="iface">
          <input
            type="checkbox"
            checked={persistGpsSqlite}
            onChange={(e) => setPersistGpsSqlite(e.target.checked)}
          />
          Persist GPS track to <code>wardrive.sqlite</code>
        </label>
      </div>

      <div className="panel">
        <h3>Excluded devices</h3>
        <p className="muted">
          Phones, hotspots, and other gear you carry. Matching traffic is not logged, shown on Nearby, or passed to
          detectors (including co-travel).
        </p>
        <h4 className="mt-4">Excluded MAC addresses</h4>
        <p className="muted">One per line (<code>aa:bb:cc:dd:ee:ff</code>). STA and AP addresses.</p>
        <textarea
          className="textarea-block"
          rows={5}
          value={excludeMacs}
          onChange={(e) => setExcludeMacs(e.target.value)}
        />
        <h4 className="mt-4">Excluded SSIDs</h4>
        <p className="muted">Exact name match on beacon or directed probe (case-sensitive).</p>
        <textarea
          className="textarea-block"
          rows={4}
          value={excludeSsids}
          onChange={(e) => setExcludeSsids(e.target.value)}
        />
      </div>

      <div className="toolbar">
        <button type="button" onClick={() => void save()}>
          Save
        </button>
        <button type="button" className="secondary" disabled={reloadBusy} onClick={() => void reloadFromDisk()}>
          {reloadBusy ? 'Reloading…' : 'Reload from disk'}
        </button>
      </div>
    </div>
  )
}
