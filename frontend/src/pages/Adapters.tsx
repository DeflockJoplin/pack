import { useCallback, useState } from 'react'
import { Link } from 'react-router-dom'
import { apiGet, apiPost } from '../api'
import { useApiLoad } from '../useApiLoad'
import { usePollApi } from '../usePollApi'

type WifiInterface = {
  name: string
  link_type?: string
}

type WifiAdapter = {
  phy_name: string
  sysfs_path: string
  interfaces: WifiInterface[]
  iw_phy_headline?: string
}

type BleAdapter = {
  name: string
  address?: string
  powered?: boolean
}

type AdaptersResponse = { adapters: WifiAdapter[] }
type BleAdaptersResponse = { adapters: BleAdapter[] }

type ConfigResponse = {
  active_capture_interfaces: string[]
  scan_ble?: boolean
  active_ble_adapters?: string[]
}

type CaptureStats = {
  wifi_frames_rx: number
  wifi_frames_non_radiotap: number
  wifi_csv_rows: number
  gps_fix: boolean
  gps_ok_to_log: boolean
  last_pcap_error: string | null
  ble_last_error: string | null
}

type MonitorCheck = {
  name: string
  pass: boolean
  detail: string
}

type MonitorHealth = {
  ok: boolean
  checks: MonitorCheck[]
  suggested_commands: string[]
  documentation: string
}

type CaptureSelectWifiResult = {
  requested: string
  capture_iface: string
  monitor_created: boolean
  ok: boolean
  message: string
  persisted?: boolean
}

type CaptureSelectResponse = {
  ok: boolean
  wifi_results: CaptureSelectWifiResult[]
  active_capture_interfaces: string[]
}

type IfaceRow = {
  phy: WifiAdapter
  iface: WifiInterface
}

function linkTypeClass(linkType?: string): string {
  if (linkType === 'monitor') return 'tag ok'
  return 'tag'
}

/** One row per netdev (all PHYs). */
function allInterfaceRows(adapters: WifiAdapter[]): IfaceRow[] {
  const rows: IfaceRow[] = []
  for (const phy of adapters) {
    for (const iface of phy.interfaces) {
      rows.push({ phy, iface })
    }
  }
  return rows.sort((a, b) => {
    const pc = a.phy.phy_name.localeCompare(b.phy.phy_name)
    if (pc !== 0) return pc
    return a.iface.name.localeCompare(b.iface.name)
  })
}

function defaultWifiSelection(saved: string[]): Set<string> {
  if (saved.length > 0) {
    return new Set(saved)
  }
  return new Set()
}

export function Adapters() {
  const [adapters, setAdapters] = useState<WifiAdapter[]>([])
  const [bleAdapters, setBleAdapters] = useState<BleAdapter[]>([])
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [selectedBle, setSelectedBle] = useState<Set<string>>(new Set())
  const [savedCaptureIfaces, setSavedCaptureIfaces] = useState<string[]>([])
  const [scanBle, setScanBle] = useState(true)
  const [stats, setStats] = useState<CaptureStats | null>(null)
  const [monitorHealth, setMonitorHealth] = useState<MonitorHealth | null>(null)
  const [healthOpen, setHealthOpen] = useState(false)
  const [wifiResults, setWifiResults] = useState<CaptureSelectWifiResult[] | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)

  const rows = allInterfaceRows(adapters)

  const refresh = useCallback(async () => {
    setErr(null)
    try {
      const [a, ble, cfg] = await Promise.all([
        apiGet<AdaptersResponse>('/api/adapters'),
        apiGet<BleAdaptersResponse>('/api/ble-adapters'),
        apiGet<ConfigResponse>('/api/config'),
      ])
      setAdapters(a.adapters)
      setBleAdapters(ble.adapters)
      const active = cfg.active_capture_interfaces ?? []
      setSavedCaptureIfaces(active)
      setSelected(defaultWifiSelection(active))
      setSelectedBle(new Set(cfg.active_ble_adapters ?? []))
      setScanBle(cfg.scan_ble ?? true)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  const loadMonitorHealth = useCallback(async () => {
    try {
      const h = await apiGet<MonitorHealth>('/api/monitor-health')
      setMonitorHealth(h)
    } catch {
      /* optional panel */
    }
  }, [])

  useApiLoad(() => {
    void refresh()
    void loadMonitorHealth()
  }, [refresh, loadMonitorHealth])

  usePollApi(
    useCallback(async () => {
      try {
        const s = await apiGet<CaptureStats>('/api/stats')
        setStats(s)
      } catch {
        /* ignore poll errors */
      }
    }, []),
    3000,
    [],
  )

  const toggle = (iface: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(iface)) next.delete(iface)
      else next.add(iface)
      return next
    })
  }

  const toggleBle = (name: string) => {
    setSelectedBle((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })
  }

  const save = async () => {
    if (
      selected.size === 0 &&
      savedCaptureIfaces.length > 0 &&
      !window.confirm(
        'Clear all WiFi capture interfaces? Channel plan band assignments will be cleared too.',
      )
    ) {
      return
    }
    setMsg(null)
    setErr(null)
    setWifiResults(null)
    setSaving(true)
    try {
      const res = await apiPost<CaptureSelectResponse>('/api/capture/select', {
        wifi_interfaces: [...selected],
        active_ble_adapters: [...selectedBle],
        scan_ble: scanBle,
      })
      setWifiResults(res.wifi_results)
      const active = res.active_capture_interfaces ?? []
      setSavedCaptureIfaces(active)
      setSelected(defaultWifiSelection(active))
      if (selected.size > 0 && active.length === 0) {
        setErr(
          'Save did not persist any capture interfaces — see setup results (netdev may be missing or iw rejected the name).',
        )
        setMsg(null)
      } else if (res.ok) {
        setMsg(
          selected.size > 0
            ? 'Capture selection saved. Assign 2.4 / 5 GHz on the Channel plan page if needed.'
            : 'Capture selection cleared.',
        )
      } else {
        setErr('Some WiFi interfaces failed setup — see results below.')
      }
      await refresh()
      if (healthOpen) void loadMonitorHealth()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  if (loading && !adapters.length && !err) {
    return (
      <div className="page-head">
        <h2 className="page-head__title">Adapters</h2>
        <p className="muted page-head__lead">Loading adapters…</p>
      </div>
    )
  }

  const activeCheck = monitorHealth?.checks.find((c) => c.name === 'active_capture_interfaces')

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Adapters</h2>
        <p className="muted page-head__lead">
          PACK does not change interface mode. Put interfaces in monitor mode before selecting
          (e.g. <code>sudo iw dev wlan0 set type monitor</code> or your usual tool). Then assign
          bands on the <Link to="/channels">Channel plan</Link> page.
        </p>
      </div>

      {err && <div className="panel panel--error">{err}</div>}

      {savedCaptureIfaces.length > 0 && (
        <p className="muted">
          Active capture: <code>{savedCaptureIfaces.join(', ')}</code>
        </p>
      )}

      {stats && (
        <div className="panel">
          <h2>Capture health</h2>
          <p className="muted">
            WiFi frames: <strong>{stats.wifi_frames_rx}</strong>
            {stats.wifi_frames_non_radiotap > 0 && (
              <>
                {' '}
                (<strong>{stats.wifi_frames_non_radiotap}</strong> without radiotap header)
              </>
            )}
            {' · '}
            WiFi CSV rows: <strong>{stats.wifi_csv_rows}</strong>
            {' · '}
            GPS fix: <strong>{stats.gps_fix ? 'yes' : 'no'}</strong>
            {stats.gps_fix && (
              <>
                {' '}
                (ok to log: <strong>{stats.gps_ok_to_log ? 'yes' : 'no'}</strong>)
              </>
            )}
          </p>
          {!stats.gps_fix && stats.wifi_frames_rx > 0 && (
            <p className="muted">
              Packets are arriving but WiGLE WiFi rows need a usable GPS fix — check <code>gpsd</code>{' '}
              and the dashboard.
            </p>
          )}
          {stats.wifi_frames_rx > 0 && stats.wifi_csv_rows === 0 && stats.gps_fix && (
            <p className="muted">
              Frames received with GPS fix but no CSV rows yet — nearby networks may be on channels not
              in your <Link to="/channels">channel plan</Link>, or privacy filters may apply.
            </p>
          )}
          {stats.last_pcap_error && <p className="error">WiFi capture: {stats.last_pcap_error}</p>}
          {stats.ble_last_error && <p className="error">BLE: {stats.ble_last_error}</p>}
        </div>
      )}

      <div className="panel">
        <h2>WiFi capture</h2>
        <p className="muted">
          One row per interface. Only rows with <span className="tag ok">monitor</span> link type can be
          selected (refresh after changing mode with <code>iw</code>).
        </p>
        {rows.length === 0 ? (
          <p className="muted">No ieee80211 PHYs found.</p>
        ) : (
          <table className="mt-4">
            <thead>
              <tr>
                <th>Select</th>
                <th>PHY</th>
                <th>Interface</th>
                <th>iw (PHY headline)</th>
              </tr>
            </thead>
            <tbody>
              {rows.map(({ phy, iface }) => (
                <tr key={`${phy.phy_name}-${iface.name}`}>
                  <td>
                    <input
                      type="checkbox"
                      checked={selected.has(iface.name)}
                      disabled={iface.link_type !== 'monitor'}
                      onChange={() => toggle(iface.name)}
                    />
                  </td>
                  <td>
                    <code>{phy.phy_name}</code>
                  </td>
                  <td>
                    <code>{iface.name}</code>
                    {iface.link_type ? (
                      <span className={linkTypeClass(iface.link_type)} style={{ marginLeft: '0.35rem' }}>
                        {iface.link_type}
                      </span>
                    ) : (
                      <span className="tag" style={{ marginLeft: '0.35rem' }}>
                        unknown
                      </span>
                    )}
                  </td>
                  <td className="muted cell-iw">{phy.iw_phy_headline ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {wifiResults && (
          <div className="mt-4">
            <h3>Setup results</h3>
            <table>
              <thead>
                <tr>
                  <th>Requested</th>
                  <th>Capture iface</th>
                  <th>Status</th>
                  <th>Active</th>
                  <th>Detail</th>
                </tr>
              </thead>
              <tbody>
                {wifiResults.map((r) => (
                  <tr key={r.requested}>
                    <td>
                      <code>{r.requested}</code>
                    </td>
                    <td>
                      <code>{r.capture_iface || '—'}</code>
                    </td>
                    <td>
                      <span className={`tag ${r.ok ? 'ok' : 'fail'}`}>{r.ok ? 'ok' : 'fail'}</span>
                    </td>
                    <td>
                      <span
                        className={`tag ${r.persisted ? 'ok' : r.ok ? 'fail' : ''}`}
                      >
                        {r.persisted ? 'yes' : 'no'}
                      </span>
                    </td>
                    <td className="muted cell-iw">{r.message}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      <div className="panel">
        <h2>BLE capture sources</h2>
        <label className="iface" style={{ display: 'block', marginBottom: 'var(--space-3)' }}>
          <input
            type="checkbox"
            checked={scanBle}
            onChange={(e) => setScanBle(e.target.checked)}
          />{' '}
          Enable BLE wardriving scan
        </label>
        {bleAdapters.length === 0 ? (
          <p className="muted">
            No HCI adapters under <code>/sys/class/bluetooth</code> — install BlueZ and ensure{' '}
            <code>bluetoothd</code> is running.
          </p>
        ) : (
          <table>
            <thead>
              <tr>
                <th>Select</th>
                <th>Adapter</th>
                <th>Address</th>
                <th>Powered</th>
              </tr>
            </thead>
            <tbody>
              {bleAdapters.map((b) => (
                <tr key={b.name}>
                  <td>
                    <input
                      type="checkbox"
                      checked={selectedBle.has(b.name)}
                      onChange={() => toggleBle(b.name)}
                    />
                  </td>
                  <td>
                    <code>{b.name}</code>
                  </td>
                  <td className="muted">{b.address ?? '—'}</td>
                  <td className="muted">{b.powered == null ? '—' : b.powered ? 'yes' : 'no'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <p className="muted" style={{ marginTop: 'var(--space-3)' }}>
          BLE wardriving and HCI recon use the adapters you select above (recon uses the first selected).
        </p>
      </div>

      <div className="toolbar">
        <button type="button" onClick={() => void save()} disabled={saving}>
          {saving ? 'Saving…' : 'Save selection'}
        </button>
        <button type="button" className="secondary" onClick={() => void refresh()}>
          Refresh
        </button>
      </div>
      {msg && <p className="msg-ok">{msg}</p>}

      <div className="panel">
        <button
          type="button"
          className="secondary"
          onClick={() => {
            setHealthOpen((o) => !o)
            if (!healthOpen) void loadMonitorHealth()
          }}
        >
          {healthOpen ? 'Hide' : 'Show'} monitor health checks
        </button>
        {healthOpen && monitorHealth && (
          <div className="mt-4">
            <p>
              Overall:{' '}
              <span className={`tag ${monitorHealth.ok ? 'ok' : 'fail'}`}>
                {monitorHealth.ok ? 'PASS' : 'ATTENTION'}
              </span>
            </p>
            {activeCheck && !activeCheck.pass && (
              <p className="panel panel--error mt-4">
                {activeCheck.detail} Run <code>iw dev &lt;iface&gt; info</code> on the same host as PACK
                to confirm monitor mode, then Save selection above.
              </p>
            )}
            <ul className="check-list mt-4">
              {monitorHealth.checks.map((c) => (
                <li key={c.name}>
                  <span className={`tag ${c.pass ? 'ok' : 'fail'}`}>{c.pass ? 'ok' : 'fix'}</span>{' '}
                  <strong>{c.name}</strong> — {c.detail}
                </li>
              ))}
            </ul>
            <h3 className="mt-4">Suggested commands</h3>
            <pre>{monitorHealth.suggested_commands.join('\n')}</pre>
            <p className="muted">{monitorHealth.documentation}</p>
            <div className="toolbar mt-4">
              <button type="button" className="secondary" onClick={() => void loadMonitorHealth()}>
                Re-run checks
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
