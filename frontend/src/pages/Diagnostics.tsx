import { useCallback, useState } from 'react'
import { apiGet } from '../api'
import { useApiLoad } from '../useApiLoad'

export function Diagnostics() {
  const [json, setJson] = useState<string>('')
  const [err, setErr] = useState<string | null>(null)

  const load = useCallback(async () => {
    setErr(null)
    try {
      const d = await apiGet<unknown>('/api/diagnostics')
      setJson(JSON.stringify(d, null, 2))
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }, [])

  useApiLoad(() => void load(), [load])

  if (err) {
    return (
      <div className="stack">
        <div className="page-head">
          <h2 className="page-head__title">Diagnostics</h2>
        </div>
        <div className="panel panel--error">{err}</div>
      </div>
    )
  }

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Diagnostics</h2>
        <p className="muted page-head__lead">
          Snapshot from <code>GET /api/diagnostics</code>. WiFi Flock per-method counts: <code>flock_wifi_alerts_method_1</code> …{' '}
          <code>_3</code>. IE signature match (methods 2–3): <code>flock_wifi_ie_sig_match_builtin_default</code>,{' '}
          <code>flock_wifi_ie_sig_match_builtin_alt_linux</code>, <code>flock_wifi_ie_sig_match_config</code>.
          Computed on wildcard probes: <code>flock_wifi_ie_sig_computed_builtin_default</code>,{' '}
          <code>_builtin_alt_linux</code>, <code>_computed_other</code>. SSID watch:{' '}
          <code>ssid_watch_probe_alerts_fired</code>, <code>ssid_watch_beacon_alerts_fired</code>,{' '}
          <code>probe_csv_rows</code>. Tune on the Detections page.
        </p>
        <div className="page-head__row">
          <button type="button" className="secondary" onClick={() => void load()}>
            Refresh
          </button>
        </div>
      </div>
      <div className="panel">
        <pre className="diagnostics-pre">{json || '…'}</pre>
      </div>
    </div>
  )
}
