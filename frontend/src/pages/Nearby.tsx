import { useCallback, useEffect, useMemo, useState } from 'react'
import { apiGet } from '../api'
import { useDocumentVisible, useVisibilityPollMs } from '../useDocumentVisible'

type FingerprintExtras = {
  probe_ie_tag_seq?: string
  probe_flock_ie_sig?: string
  vendor_ie_sigs?: string[]
  wps_device_name?: string
  wps_model_number?: string
  wps_uuid_e_partial?: string
  wps_device_password_id?: number
  wps_serial_number?: string
  rsn_group_cipher?: string
  rsn_pairwise_ciphers?: string[]
  rsn_akm_suites?: string[]
  rsn_mfp?: string
  interworking_access?: number
  phy_summary?: string
  ble_appearance?: number
  ble_service_uuids?: string[]
  ble_service_data_keys?: string[]
  ble_mfg_payload_hex?: string
  ble_mfg_other_sigs?: string[]
  ble_tx_power?: number
  ble_advertising_flags_hex?: string
  curated_hint?: string
  curated_confidence?: number
}

type NearbyDeviceRow = {
  kind: string
  mac: string
  label: string
  rssi_last: number
  rssi_ema: number
  channel: number
  last_seen_ms: number
  age_ms: number
  fingerprint_vendor: string | null
  fingerprint_model: string | null
  fingerprint_source: string
  randomized: boolean
  fingerprint_extras?: FingerprintExtras | null
}

type NearbySnapshot = {
  generated_ms: number
  devices: NearbyDeviceRow[]
}

function fingerprintExtrasSummary(x: FingerprintExtras | null | undefined): string {
  if (!x) return ''
  const parts: string[] = []
  if (x.curated_hint) parts.push(`curated: ${x.curated_hint}`)
  if (x.probe_ie_tag_seq) parts.push(`probe IE: ${x.probe_ie_tag_seq}`)
  if (x.probe_flock_ie_sig) parts.push(`flock IE sig: ${x.probe_flock_ie_sig}`)
  if (x.vendor_ie_sigs?.length) parts.push(`vendor IE: ${x.vendor_ie_sigs.slice(0, 3).join('; ')}`)
  if (x.phy_summary) parts.push(`PHY: ${x.phy_summary}`)
  if (x.wps_device_name) parts.push(`WPS name: ${x.wps_device_name}`)
  if (x.wps_model_number) parts.push(`WPS #${x.wps_model_number}`)
  if (x.wps_uuid_e_partial) parts.push(`WPS UUID-E prefix: ${x.wps_uuid_e_partial}`)
  if (x.wps_device_password_id != null) parts.push(`WPS pwd-id: ${x.wps_device_password_id}`)
  if (x.wps_serial_number) parts.push(`WPS serial: ${x.wps_serial_number}`)
  if (x.rsn_group_cipher) parts.push(`RSN group: ${x.rsn_group_cipher}`)
  if (x.rsn_pairwise_ciphers?.length) parts.push(`RSN PTK: ${x.rsn_pairwise_ciphers.join(',')}`)
  if (x.rsn_akm_suites?.length) parts.push(`RSN AKM: ${x.rsn_akm_suites.join(',')}`)
  if (x.rsn_mfp) parts.push(`MFP: ${x.rsn_mfp}`)
  if (x.interworking_access != null) parts.push(`IW access: 0x${x.interworking_access.toString(16)}`)
  if (x.ble_appearance != null) parts.push(`appearance: 0x${x.ble_appearance.toString(16)}`)
  if (x.ble_service_uuids?.length) parts.push(`UUIDs: ${x.ble_service_uuids.slice(0, 4).join(', ')}`)
  if (x.ble_mfg_payload_hex) parts.push(`mfg: ${x.ble_mfg_payload_hex}`)
  if (x.ble_mfg_other_sigs?.length) parts.push(`mfg+: ${x.ble_mfg_other_sigs.slice(0, 4).join('; ')}`)
  if (x.ble_tx_power != null) parts.push(`tx: ${x.ble_tx_power}`)
  return parts.join(' · ')
}

function buildQuery(params: {
  limit: number
  max_age_ms: number
  kinds: string[]
  min_rssi: string
  q: string
}): string {
  const sp = new URLSearchParams()
  sp.set('limit', String(params.limit))
  sp.set('max_age_ms', String(params.max_age_ms))
  if (params.kinds.length === 0) {
    sp.set('kinds', '')
  } else if (params.kinds.length < 3) {
    sp.set('kinds', params.kinds.join(','))
  }
  const min = params.min_rssi.trim()
  if (min !== '') sp.set('min_rssi', min)
  const q = params.q.trim()
  if (q !== '') sp.set('q', q.toLowerCase())
  return sp.toString()
}

export function Nearby() {
  const [snapshot, setSnapshot] = useState<NearbySnapshot | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [useStream, setUseStream] = useState(true)
  const [kinds, setKinds] = useState({ wifi_ap: true, wifi_sta: true, ble: true })
  const [q, setQ] = useState('')
  const [minRssi, setMinRssi] = useState('')
  const [maxAgeMs, setMaxAgeMs] = useState(60_000)
  const pageVisible = useDocumentVisible()
  const nearbyPollMs = useVisibilityPollMs(1000)

  const kindsList = useMemo(() => {
    const v: string[] = []
    if (kinds.wifi_ap) v.push('wifi_ap')
    if (kinds.wifi_sta) v.push('wifi_sta')
    if (kinds.ble) v.push('ble')
    return v
  }, [kinds])

  const queryString = useMemo(
    () =>
      buildQuery({
        limit: 200,
        max_age_ms: maxAgeMs,
        kinds: kindsList,
        min_rssi: minRssi,
        q,
      }),
    [kindsList, maxAgeMs, minRssi, q],
  )

  const applySnapshot = useCallback((raw: string) => {
    try {
      const parsed = JSON.parse(raw) as NearbySnapshot
      if (Array.isArray(parsed.devices)) {
        setSnapshot(parsed)
        setErr(null)
      }
    } catch {
      /* ignore malformed SSE chunk */
    }
  }, [])

  useEffect(() => {
    if (useStream) {
      if (!pageVisible) {
        return
      }
      const url = `/api/nearby/stream?${queryString}`
      const es = new EventSource(url)
      es.onmessage = (ev) => applySnapshot(ev.data)
      es.onerror = () => {
        setErr('EventSource disconnected (is the backend running?)')
        es.close()
      }
      return () => es.close()
    }

    let cancelled = false
    const poll = async () => {
      try {
        const s = await apiGet<NearbySnapshot>(`/api/nearby?${queryString}`)
        if (!cancelled) {
          setSnapshot(s)
          setErr(null)
        }
      } catch (e) {
        if (!cancelled) setErr(e instanceof Error ? e.message : String(e))
      }
    }
    queueMicrotask(() => void poll())
    const id = window.setInterval(() => void poll(), nearbyPollMs)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [useStream, queryString, applySnapshot, pageVisible, nearbyPollMs])

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Nearby</h2>
        <p className="muted page-head__lead">
          Live view from <code>/api/nearby</code>. Sorted by RSSI as a proxy for distance. Fingerprint hints come from OUI, BLE company ID, WPS when present, passive
          IE/advert fields, and potentially in-repo curated rules.
        </p>
      </div>
      <div className="panel">
        <div className="toolbar toolbar--tight">
          <label>
            <input
              type="checkbox"
              checked={useStream}
              onChange={(e) => setUseStream(e.target.checked)}
            />{' '}
            SSE stream (~4/s)
          </label>
          <label>
            Max age (ms){' '}
            <input
              type="number"
              min={1000}
              max={120000}
              step={1000}
              value={maxAgeMs}
              onChange={(e) => setMaxAgeMs(Number(e.target.value) || 60_000)}
              className="input-medium"
            />
          </label>
          <label>
            Min RSSI{' '}
            <input
              type="number"
              min={-100}
              max={0}
              value={minRssi}
              onChange={(e) => setMinRssi(e.target.value)}
              placeholder="any"
              className="input-narrow"
            />
          </label>
          <label>
            Search MAC / label{' '}
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="filter" />
          </label>
        </div>
        <div className="toolbar mt-4">
          <label>
            <input
              type="checkbox"
              checked={kinds.wifi_ap}
              onChange={(e) => setKinds((k) => ({ ...k, wifi_ap: e.target.checked }))}
            />{' '}
            WiFi AP
          </label>
          <label>
            <input
              type="checkbox"
              checked={kinds.wifi_sta}
              onChange={(e) => setKinds((k) => ({ ...k, wifi_sta: e.target.checked }))}
            />{' '}
            WiFi STA (probes)
          </label>
          <label>
            <input
              type="checkbox"
              checked={kinds.ble}
              onChange={(e) => setKinds((k) => ({ ...k, ble: e.target.checked }))}
            />{' '}
            BLE
          </label>
        </div>
      </div>

      {err && <div className="panel panel--error">{err}</div>}

      {!snapshot && !err && <p className="muted">Waiting for data…</p>}

      {snapshot && (
        <div className="panel">
          <p className="muted" style={{ marginTop: 0 }}>
            {snapshot.devices.length} device(s) · generated_ms {snapshot.generated_ms}
          </p>
          <div className="data-table-wrap">
            <table className="nearby-table">
            <thead>
              <tr>
                <th>Kind</th>
                <th>MAC</th>
                <th>Label</th>
                <th>RSSI last</th>
                <th>RSSI EMA</th>
                <th>Ch</th>
                <th>Age ms</th>
                <th>Vendor / model</th>
                <th>Src</th>
                <th>Rand</th>
              </tr>
            </thead>
            <tbody>
              {snapshot.devices.map((d) => (
                <tr key={`${d.kind}-${d.mac}`}>
                  <td>{d.kind}</td>
                  <td>
                    <code>{d.mac}</code>
                  </td>
                  <td>{d.label || '—'}</td>
                  <td>{d.rssi_last}</td>
                  <td>{d.rssi_ema}</td>
                  <td>{d.channel || '—'}</td>
                  <td>{d.age_ms}</td>
                  <td>
                    <div>
                      {d.fingerprint_vendor || '—'}
                      {d.fingerprint_model ? ` · ${d.fingerprint_model}` : ''}
                    </div>
                    {fingerprintExtrasSummary(d.fingerprint_extras ?? undefined) ? (
                      <div className="muted" style={{ fontSize: '0.85em', marginTop: 4, maxWidth: 420 }}>
                        {fingerprintExtrasSummary(d.fingerprint_extras ?? undefined)}
                      </div>
                    ) : null}
                  </td>
                  <td>{d.fingerprint_source}</td>
                  <td>{d.randomized ? 'yes' : ''}</td>
                </tr>
              ))}
            </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  )
}
