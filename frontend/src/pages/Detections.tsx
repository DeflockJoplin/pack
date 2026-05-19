import { useCallback, useState } from 'react'
import { Link } from 'react-router-dom'
import { apiGet, apiPost } from '../api'
import { useApiLoad } from '../useApiLoad'
import { macsToText, textToMacs } from '../macHelpers'

type DetectionsConfigResponse = DetectionsConfig & {
  flock_ignore_macs?: string[]
}

type DetectionsConfig = {
  flock_disable_wifi_mask: number
  flock_disable_ble_mask: number
  flock_wifi_min_wildcards_in_window: number
  flock_wifi_min_distinct_channels: number
  flock_wifi_min_rssi_span: number
  flock_wifi_per_src_cooldown_ms: number
  flock_wifi_ie_sig_primary: string
  flock_wifi_ie_sig_alternates: string[]
  ssid_watch_enabled_probe: boolean
  ssid_watch_enabled_beacon: boolean
  ssid_watch_ssids: string[]
  ssid_watch_per_src_cooldown_ms: number
  probe_csv_log_enabled: boolean
  probe_csv_log_wildcards: boolean
}

const WIFI_DISABLE_BIT = (method: 1 | 2 | 3) => 1 << (method - 1)

function wifiMethodDisabled(mask: number, method: 1 | 2 | 3): boolean {
  return (mask & WIFI_DISABLE_BIT(method)) !== 0
}

function withWifiMethodDisabled(mask: number, method: 1 | 2 | 3, disabled: boolean): number {
  const b = WIFI_DISABLE_BIT(method)
  if (disabled) {
    return mask | b
  }
  return mask & ~b
}

type BleMethod = 32 | 33 | 34 | 35
const BLE_DISABLE_BIT = (method: BleMethod) => 1 << (method - 32)

function bleMethodDisabled(mask: number, method: BleMethod): boolean {
  return (mask & BLE_DISABLE_BIT(method)) !== 0
}

function withBleMethodDisabled(mask: number, method: BleMethod, disabled: boolean): number {
  const b = BLE_DISABLE_BIT(method)
  if (disabled) {
    return mask | b
  }
  return mask & ~b
}

/** Matches `FLOCK_*_DISABLE_MASK_DEFAULT` in `backend/src/flock_types.rs` (deprecated methods off). */
const DEFAULT_FLOCK_DISABLE_WIFI_MASK = 1 << 0 // method 1
const DEFAULT_FLOCK_DISABLE_BLE_MASK = (1 << 0) | (1 << 1) | (1 << 2) // methods 32–34

const defaultForm = (): DetectionsConfig => ({
  flock_disable_wifi_mask: DEFAULT_FLOCK_DISABLE_WIFI_MASK,
  flock_disable_ble_mask: DEFAULT_FLOCK_DISABLE_BLE_MASK,
  flock_wifi_min_wildcards_in_window: 1,
  flock_wifi_min_distinct_channels: 0,
  flock_wifi_min_rssi_span: 0,
  flock_wifi_per_src_cooldown_ms: 30_000,
  flock_wifi_ie_sig_primary: '',
  flock_wifi_ie_sig_alternates: [],
  ssid_watch_enabled_probe: false,
  ssid_watch_enabled_beacon: false,
  ssid_watch_ssids: [],
  ssid_watch_per_src_cooldown_ms: 30_000,
  probe_csv_log_enabled: false,
  probe_csv_log_wildcards: false,
})

export function DetectionsPage() {
  const [form, setForm] = useState<DetectionsConfig>(defaultForm)
  const [ssidLines, setSsidLines] = useState('')
  const [flockIgnoreMacs, setFlockIgnoreMacs] = useState('')
  const [msg, setMsg] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)

  const load = useCallback(async () => {
    setErr(null)
    try {
      const c = await apiGet<DetectionsConfigResponse>('/api/config')
      setForm({
        flock_disable_wifi_mask: c.flock_disable_wifi_mask ?? DEFAULT_FLOCK_DISABLE_WIFI_MASK,
        flock_disable_ble_mask: c.flock_disable_ble_mask ?? DEFAULT_FLOCK_DISABLE_BLE_MASK,
        flock_wifi_min_wildcards_in_window: c.flock_wifi_min_wildcards_in_window ?? 1,
        flock_wifi_min_distinct_channels: c.flock_wifi_min_distinct_channels ?? 0,
        flock_wifi_min_rssi_span: c.flock_wifi_min_rssi_span ?? 0,
        flock_wifi_per_src_cooldown_ms: c.flock_wifi_per_src_cooldown_ms ?? 30_000,
        flock_wifi_ie_sig_primary: c.flock_wifi_ie_sig_primary ?? '',
        flock_wifi_ie_sig_alternates: c.flock_wifi_ie_sig_alternates ?? [],
        ssid_watch_enabled_probe: c.ssid_watch_enabled_probe ?? false,
        ssid_watch_enabled_beacon: c.ssid_watch_enabled_beacon ?? false,
        ssid_watch_ssids: c.ssid_watch_ssids ?? [],
        ssid_watch_per_src_cooldown_ms: c.ssid_watch_per_src_cooldown_ms ?? 30_000,
        probe_csv_log_enabled: c.probe_csv_log_enabled ?? false,
        probe_csv_log_wildcards: c.probe_csv_log_wildcards ?? false,
      })
      setSsidLines((c.ssid_watch_ssids ?? []).join('\n'))
      setFlockIgnoreMacs(macsToText(c.flock_ignore_macs ?? []))
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }, [])

  useApiLoad(() => void load(), [load])

  const save = async () => {
    setMsg(null)
    setErr(null)
    const ssid_watch_ssids = ssidLines
      .split('\n')
      .map((s) => s.trim())
      .filter((s) => s.length > 0)
    try {
      await apiPost('/api/config', {
        flock_disable_wifi_mask: form.flock_disable_wifi_mask,
        flock_disable_ble_mask: form.flock_disable_ble_mask,
        flock_wifi_min_wildcards_in_window: form.flock_wifi_min_wildcards_in_window,
        flock_wifi_min_distinct_channels: form.flock_wifi_min_distinct_channels,
        flock_wifi_min_rssi_span: form.flock_wifi_min_rssi_span,
        flock_wifi_per_src_cooldown_ms: form.flock_wifi_per_src_cooldown_ms,
        flock_wifi_ie_sig_primary: form.flock_wifi_ie_sig_primary,
        flock_wifi_ie_sig_alternates: form.flock_wifi_ie_sig_alternates,
        ssid_watch_enabled_probe: form.ssid_watch_enabled_probe,
        ssid_watch_enabled_beacon: form.ssid_watch_enabled_beacon,
        ssid_watch_ssids,
        ssid_watch_per_src_cooldown_ms: form.ssid_watch_per_src_cooldown_ms,
        probe_csv_log_enabled: form.probe_csv_log_enabled,
        probe_csv_log_wildcards: form.probe_csv_log_wildcards,
        flock_ignore_macs: textToMacs(flockIgnoreMacs),
      })
      setMsg('Saved to data/config/app.json')
      void load()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  const setWifiMethodEnabled = (method: 1 | 2 | 3, enabled: boolean) => {
    setForm((f) => ({
      ...f,
      flock_disable_wifi_mask: withWifiMethodDisabled(f.flock_disable_wifi_mask, method, !enabled),
    }))
  }

  const setBleMethodEnabled = (method: BleMethod, enabled: boolean) => {
    setForm((f) => ({
      ...f,
      flock_disable_ble_mask: withBleMethodDisabled(f.flock_disable_ble_mask, method, !enabled),
    }))
  }

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Detections</h2>
        <p className="muted page-head__lead">
          Configure WiFi Flock, BLE Flock, and optional SSID watch logging. SSID matches are exact and case-sensitive;
          hits are written to <code>data/ssid_watch/</code>. Optional probe CSV logging goes under <code>data/probe_csv/</code>.
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
        <h2>WiFi — Flock</h2>
        <p className="muted">
          Three parallel detections on broadcast wildcard probes (see <code>docs/FINGERPRINTING.md</code>). Per-source
          cooldown is per method. Set cooldown to <code>0</code> to log every qualifying fire (CSV can grow quickly).
        </p>
        <h3 className="mt-4">Enable methods</h3>
        <p className="muted">
          Checked = enabled. Unchecked sets <code>flock_disable_wifi_mask</code>.
        </p>
        <label className="iface">
          <input
            type="checkbox"
            checked={!wifiMethodDisabled(form.flock_disable_wifi_mask, 1)}
            onChange={(e) => setWifiMethodEnabled(1, e.target.checked)}
          />
          Method 1 — wildcard probe + Flock OUI
        </label>
        <label className="iface">
          <input
            type="checkbox"
            checked={!wifiMethodDisabled(form.flock_disable_wifi_mask, 2)}
            onChange={(e) => setWifiMethodEnabled(2, e.target.checked)}
          />
          Method 2 — wildcard probe + IE signature + Flock OUI (Experimental)
        </label>
        <label className="iface">
          <input
            type="checkbox"
            checked={!wifiMethodDisabled(form.flock_disable_wifi_mask, 3)}
            onChange={(e) => setWifiMethodEnabled(3, e.target.checked)}
          />
          Method 3 — wildcard probe + IE signature (Experimental. Possibly useful for hunting new OUIs)
        </label>

        <h3 className="mt-4">Gates and cooldown</h3>
        <div className="toolbar toolbar--tight" style={{ flexWrap: 'wrap', gap: '1rem' }}>
          <label>
            Min wildcards in 15s window{' '}
            <input
              type="number"
              min={1}
              max={4096}
              value={form.flock_wifi_min_wildcards_in_window}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  flock_wifi_min_wildcards_in_window: Number(e.target.value) || 1,
                }))
              }
            />
          </label>
          <label>
            Min distinct 2.4 GHz channels (0 = off){' '}
            <input
              type="number"
              min={0}
              max={14}
              value={form.flock_wifi_min_distinct_channels}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  flock_wifi_min_distinct_channels: Number(e.target.value) || 0,
                }))
              }
            />
          </label>
          <label>
            Min RSSI span (0 = off){' '}
            <input
              type="number"
              min={0}
              max={120}
              value={form.flock_wifi_min_rssi_span}
              onChange={(e) =>
                setForm((f) => ({ ...f, flock_wifi_min_rssi_span: Number(e.target.value) || 0 }))
              }
            />
          </label>
          <label>
            Per-source cooldown (ms, 0 = none){' '}
            <input
              type="number"
              min={0}
              max={3600000}
              value={form.flock_wifi_per_src_cooldown_ms}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  flock_wifi_per_src_cooldown_ms: Math.max(0, Number(e.target.value) || 0),
                }))
              }
            />
          </label>
        </div>

        <h3 className="mt-4">Primary IE signature</h3>
        <p className="muted">
          Same format as <code>scripts/flock_probe_ie_sig.py</code>. Pipe <code>|</code> for alternatives in the primary
          field.
        </p>
        <textarea
          className="textarea-block"
          rows={3}
          spellCheck={false}
          value={form.flock_wifi_ie_sig_primary}
          onChange={(e) => setForm((f) => ({ ...f, flock_wifi_ie_sig_primary: e.target.value }))}
          placeholder="(optional — built-ins always allowed)"
        />
        <p className="muted mt-2">Additional exact-match signatures (one per line):</p>
        <textarea
          className="textarea-block"
          rows={4}
          spellCheck={false}
          value={form.flock_wifi_ie_sig_alternates.join('\n')}
          onChange={(e) =>
            setForm((f) => ({
              ...f,
              flock_wifi_ie_sig_alternates: e.target.value
                .split('\n')
                .map((s) => s.trim())
                .filter((s) => s.length > 0),
            }))
          }
          placeholder="(optional)"
        />

        <h3 className="mt-4">False positive MACs</h3>
        <p className="muted">
          Fixed infrastructure you pass regularly that triggers Flock or co-travel without being your own device. One
          MAC per line (<code>aa:bb:cc:dd:ee:ff</code>). For phones and hotspots you carry, use{' '}
          <Link to="/privacy">Privacy</Link>.
        </p>
        <textarea
          className="textarea-block"
          rows={5}
          spellCheck={false}
          value={flockIgnoreMacs}
          onChange={(e) => setFlockIgnoreMacs(e.target.value)}
          placeholder="(none)"
        />
      </div>

      <div className="panel">
        <h2>BLE — Flock</h2>
        <p className="muted">
          Checked = method enabled.
        </p>
        <label className="iface">
          <input
            type="checkbox"
            checked={!bleMethodDisabled(form.flock_disable_ble_mask, 32)}
            onChange={(e) => setBleMethodEnabled(32, e.target.checked)}
          />
          32 — Xuntong manufacturer ID  (deprecated)
        </label>
        <label className="iface">
          <input
            type="checkbox"
            checked={!bleMethodDisabled(form.flock_disable_ble_mask, 33)}
            onChange={(e) => setBleMethodEnabled(33, e.target.checked)}
          />
          33 — Advertised name keyword (deprecated)
        </label>
        <label className="iface">
          <input
            type="checkbox"
            checked={!bleMethodDisabled(form.flock_disable_ble_mask, 34)}
            onChange={(e) => setBleMethodEnabled(34, e.target.checked)}
          />
          34 — MAC OUI (deprecated)
        </label>
        <label className="iface">
          <input
            type="checkbox"
            checked={!bleMethodDisabled(form.flock_disable_ble_mask, 35)}
            onChange={(e) => setBleMethodEnabled(35, e.target.checked)}
          />
          35 — Axon OUI (untested)
        </label>
      </div>

      <div className="panel">
        <h2>WiFi — SSID watch</h2>
        <p className="muted">
          Log matches to timestamped CSV under <code>ssid_watch/</code> when GPS is usable. Per-source cooldown applies
          separately for probe (STA MAC) and beacon (BSSID).
        </p>
        <label className="iface">
          <input
            type="checkbox"
            checked={form.ssid_watch_enabled_probe}
            onChange={(e) => setForm((f) => ({ ...f, ssid_watch_enabled_probe: e.target.checked }))}
          />
          Alert on directed probes (non-wildcard SSID in probe request)
        </label>
        <label className="iface">
          <input
            type="checkbox"
            checked={form.ssid_watch_enabled_beacon}
            onChange={(e) => setForm((f) => ({ ...f, ssid_watch_enabled_beacon: e.target.checked }))}
          />
          Alert on AP advertisements (beacon / probe-response SSID)
        </label>
        <label className="mt-4" style={{ display: 'block' }}>
          Per-source cooldown (ms, 0 = none){' '}
          <input
            type="number"
            min={0}
            max={3600000}
            value={form.ssid_watch_per_src_cooldown_ms}
            onChange={(e) =>
              setForm((f) => ({
                ...f,
                ssid_watch_per_src_cooldown_ms: Math.max(0, Number(e.target.value) || 0),
              }))
            }
          />
        </label>
        <p className="muted mt-4">Watched SSIDs (one per line, exact match):</p>
        <textarea
          className="textarea-block"
          rows={6}
          spellCheck={false}
          value={ssidLines}
          onChange={(e) => setSsidLines(e.target.value)}
          placeholder="ExampleNet"
        />
      </div>

      <div className="panel">
        <h2>WiFi — Probe CSV log</h2>
        <p className="muted">
          Log probe requests to a MAC-free CSV under <code>data/probe_csv/</code> (format{' '}
          <code>PACK-probe-log-1.0</code>): SSID, location, and RF metadata only — not WiGLE upload
          format.
        </p>
        <label className="iface">
          <input
            type="checkbox"
            checked={form.probe_csv_log_enabled}
            onChange={(e) => setForm((f) => ({ ...f, probe_csv_log_enabled: e.target.checked }))}
          />
          Log probe requests to CSV
        </label>
        <label className="iface">
          <input
            type="checkbox"
            checked={form.probe_csv_log_wildcards}
            onChange={(e) => setForm((f) => ({ ...f, probe_csv_log_wildcards: e.target.checked }))}
          />
          Include broadcast wildcard probes (empty SSID column)
        </label>
      </div>

      <div className="toolbar">
        <button type="button" onClick={() => void save()}>
          Save
        </button>
        <button type="button" className="secondary" onClick={() => void load()}>
          Reload
        </button>
      </div>
    </div>
  )
}
