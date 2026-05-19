import { useCallback, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { apiGet, apiPost } from '../api'
import { useApiLoad } from '../useApiLoad'
import { usePollApi } from '../usePollApi'

type ChannelPlan = {
  include_2_4_ghz: boolean
  include_5_ghz: boolean
  include_dfs: boolean
  dwell_ms: number
  channels_2_4: number[]
  channels_5: number[]
  channels_5_dfs?: number[]
  adapters_2_4: string[]
  adapters_5: string[]
  per_adapter: { interface: string; pinned_channel?: number | null }[]
}

type ChannelPlanEffective = {
  sequence: number[]
  channels_2_4_in_use: number[]
  channels_5_in_use: number[]
  channels_5_dfs_in_use: number[]
  hop_cycle_ms: number
}

type InterfaceHopStatus = {
  name: string
  current_channel: number | null
}

type ChannelPlanRuntime = {
  capture_enabled: boolean
  interfaces: InterfaceHopStatus[]
}

type ChannelPlanResponse = {
  plan: ChannelPlan
  effective: ChannelPlanEffective
  runtime: ChannelPlanRuntime
}

type ConfigResponse = {
  active_capture_interfaces: string[]
}

type PostChannelPlanBody = {
  include_2_4_ghz: boolean
  include_5_ghz: boolean
  include_dfs: boolean
  dwell_ms: number
  channels_2_4_in_use: number[]
  channels_5_in_use: number[]
  channels_5_dfs_in_use: number[]
  adapters_2_4: string[]
  adapters_5: string[]
  per_adapter: ChannelPlan['per_adapter']
}

function parseChannels(s: string): number[] {
  return s
    .split(/[\s,]+/)
    .map((x) => parseInt(x.trim(), 10))
    .filter((n) => !Number.isNaN(n) && n > 0 && n < 256)
}

function formatChannels(ch: number[]): string {
  return ch.join(' ')
}

function draftBody(
  plan: ChannelPlan,
  c24: string,
  c5: string,
  c5dfs: string,
): PostChannelPlanBody {
  return {
    include_2_4_ghz: plan.include_2_4_ghz,
    include_5_ghz: plan.include_5_ghz,
    include_dfs: plan.include_dfs,
    dwell_ms: plan.dwell_ms,
    channels_2_4_in_use: parseChannels(c24),
    channels_5_in_use: parseChannels(c5),
    channels_5_dfs_in_use: parseChannels(c5dfs),
    adapters_2_4: plan.adapters_2_4,
    adapters_5: plan.adapters_5,
    per_adapter: plan.per_adapter,
  }
}

function bandForIface(plan: ChannelPlan, iface: string): '2.4' | '5' | '—' {
  if (plan.adapters_2_4.includes(iface)) return '2.4'
  if (plan.adapters_5.includes(iface)) return '5'
  return '—'
}

export function ChannelPlanPage() {
  const [plan, setPlan] = useState<ChannelPlan | null>(null)
  const [effective, setEffective] = useState<ChannelPlanEffective | null>(null)
  const [runtime, setRuntime] = useState<ChannelPlanRuntime | null>(null)
  const [captureIfaces, setCaptureIfaces] = useState<string[]>([])
  const [c24, setC24] = useState('')
  const [c5, setC5] = useState('')
  const [c5dfs, setC5dfs] = useState('')
  const [defaults24, setDefaults24] = useState<number[]>([])
  const [defaults5, setDefaults5] = useState<number[]>([])
  const [defaults5dfs, setDefaults5dfs] = useState<number[]>([])
  const [msg, setMsg] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const previewGen = useRef(0)

  const applyResponse = useCallback((r: ChannelPlanResponse) => {
    setPlan(r.plan)
    setEffective(r.effective)
    setRuntime(r.runtime)
    setC24(formatChannels(r.effective.channels_2_4_in_use))
    setC5(formatChannels(r.effective.channels_5_in_use))
    setC5dfs(formatChannels(r.effective.channels_5_dfs_in_use))
    setDefaults24(r.plan.channels_2_4)
    setDefaults5(r.plan.channels_5)
    setDefaults5dfs(r.plan.channels_5_dfs ?? [])
  }, [])

  const load = useCallback(async () => {
    setErr(null)
    try {
      const [r, cfg] = await Promise.all([
        apiGet<ChannelPlanResponse>('/api/channel-plan'),
        apiGet<ConfigResponse>('/api/config'),
      ])
      applyResponse(r)
      setCaptureIfaces(cfg.active_capture_interfaces ?? [])
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }, [applyResponse])

  useApiLoad(() => void load(), [load])

  const pollRuntime = useCallback(async () => {
    try {
      const r = await apiGet<ChannelPlanResponse>('/api/channel-plan')
      setEffective(r.effective)
      setRuntime(r.runtime)
    } catch {
      /* ignore poll errors */
    }
  }, [])

  usePollApi(() => void pollRuntime(), 3000, [pollRuntime])

  const previewFromDraft = useCallback(
    async (body: PostChannelPlanBody) => {
      const gen = ++previewGen.current
      try {
        const eff = await apiPost<ChannelPlanEffective>('/api/channel-plan/preview', body)
        if (gen !== previewGen.current) return
        setEffective(eff)
        setC24(formatChannels(eff.channels_2_4_in_use))
        setC5(formatChannels(eff.channels_5_in_use))
        setC5dfs(formatChannels(eff.channels_5_dfs_in_use))
      } catch (e) {
        if (gen === previewGen.current) {
          setErr(e instanceof Error ? e.message : String(e))
        }
      }
    },
    [],
  )

  const updatePlan = useCallback(
    (next: ChannelPlan, nextC24: string, nextC5: string, nextC5dfs: string) => {
      setPlan(next)
      void previewFromDraft(draftBody(next, nextC24, nextC5, nextC5dfs))
    },
    [previewFromDraft],
  )

  const toggleAdapter = (iface: string, band: '2.4' | '5') => {
    if (!plan) return
    const on24 = new Set(plan.adapters_2_4)
    const on5 = new Set(plan.adapters_5)
    if (band === '2.4') {
      if (on24.has(iface)) on24.delete(iface)
      else {
        on24.add(iface)
        on5.delete(iface)
      }
    } else {
      if (on5.has(iface)) on5.delete(iface)
      else {
        on5.add(iface)
        on24.delete(iface)
      }
    }
    const next = {
      ...plan,
      adapters_2_4: [...on24].sort(),
      adapters_5: [...on5].sort(),
    }
    updatePlan(next, c24, c5, c5dfs)
  }

  const onBandToggle = (patch: Partial<ChannelPlan>) => {
    if (!plan) return
    const next = { ...plan, ...patch }
    updatePlan(next, c24, c5, c5dfs)
  }

  const save = async () => {
    if (!plan) return
    setMsg(null)
    setErr(null)
    const body = draftBody(plan, c24, c5, c5dfs)
    try {
      const r = await apiPost<ChannelPlanResponse>('/api/channel-plan', body)
      applyResponse(r)
      setMsg('Channel plan saved to data/config/app.json')
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  if (err && !plan) {
    return (
      <div className="stack">
        <div className="page-head">
          <h2 className="page-head__title">Channel plan</h2>
        </div>
        <div className="panel panel--error">{err}</div>
      </div>
    )
  }
  if (!plan || !effective) {
    return (
      <div className="page-head">
        <h2 className="page-head__title">Channel plan</h2>
        <p className="muted page-head__lead">Loading…</p>
      </div>
    )
  }

  const cycleSec = (effective.hop_cycle_ms / 1000).toFixed(1)
  const adapterList = captureIfaces
  const staleBandIfaces = [
    ...plan.adapters_2_4.filter((iface) => !captureIfaces.includes(iface)),
    ...plan.adapters_5.filter((iface) => !captureIfaces.includes(iface)),
  ]

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Channel plan</h2>
        <p className="muted page-head__lead">
          Assign each selected capture interface to 2.4 or 5 GHz below. Channels on a band are split
          evenly across adapters on that band. Save monitor interfaces on the{' '}
          <Link to="/adapters">Adapters</Link> page first.
        </p>
      </div>
      {err && <div className="panel panel--error">{err}</div>}
      {staleBandIfaces.length > 0 && (
        <div className="panel panel--error">
          Channel plan still references{' '}
          {staleBandIfaces.map((i) => (
            <code key={i}>{i} </code>
          ))}
          but those interfaces are not in active capture —{' '}
          <Link to="/adapters">re-save on Adapters</Link> or Save channel plan after fixing selection.
        </div>
      )}
      {captureIfaces.length === 0 && (
        <div className="panel panel--error">
          No capture interfaces selected — <Link to="/adapters">select monitor interfaces</Link> on the
          Adapters page first.
        </div>
      )}

      <div className="panel">
        <h2>Dwell</h2>
        <p className="muted small">
          Each hopping adapter listens on its channel for this long before advancing (per-radio
          dwell, not a shared global pause). Full cycle time below is the slowest adapter’s bucket
          length × dwell.
        </p>
        <div className="row">
          <label>
            Dwell (ms){' '}
            <input
              type="number"
              min={50}
              max={10000}
              value={plan.dwell_ms}
              onChange={(e) =>
                setPlan({
                  ...plan,
                  dwell_ms: Math.max(50, parseInt(e.target.value, 10) || 200),
                })
              }
              className="input-medium"
            />
          </label>
        </div>
      </div>

      <div className="panel">
        <h2>2.4 GHz</h2>
        <label>
          <input
            type="checkbox"
            checked={plan.include_2_4_ghz}
            onChange={(e) => onBandToggle({ include_2_4_ghz: e.target.checked })}
          />{' '}
          Enable 2.4 GHz band
        </label>
        {adapterList.length > 0 && (
          <div className="mt-4">
            <p className="muted">Adapters on this band (exclusive with 5 GHz):</p>
            <div className="row" style={{ flexWrap: 'wrap', gap: '0.5rem' }}>
              {adapterList.map((iface) => (
                <label key={iface} className="iface">
                  <input
                    type="checkbox"
                    checked={plan.adapters_2_4.includes(iface)}
                    disabled={!plan.include_2_4_ghz}
                    onChange={() => toggleAdapter(iface, '2.4')}
                  />
                  <code>{iface}</code>
                </label>
              ))}
            </div>
          </div>
        )}
        <label className="field-col mt-4">
          Channels
          <input
            value={c24}
            onChange={(e) => {
              setC24(e.target.value)
              void previewFromDraft(draftBody(plan, e.target.value, c5, c5dfs))
            }}
            disabled={!plan.include_2_4_ghz}
            className="input-block"
          />
        </label>
        <button
          type="button"
          className="secondary mt-4"
          disabled={!plan.include_2_4_ghz}
          onClick={() => {
            const s = formatChannels(defaults24)
            setC24(s)
            void previewFromDraft(draftBody(plan, s, c5, c5dfs))
          }}
        >
          Reset to defaults
        </button>
      </div>

      <div className="panel">
        <h2>5 GHz</h2>
        <label>
          <input
            type="checkbox"
            checked={plan.include_5_ghz}
            onChange={(e) => onBandToggle({ include_5_ghz: e.target.checked })}
          />{' '}
          Enable 5 GHz band
        </label>
        {adapterList.length > 0 && (
          <div className="mt-4">
            <p className="muted">Adapters on this band (exclusive with 2.4 GHz):</p>
            <div className="row" style={{ flexWrap: 'wrap', gap: '0.5rem' }}>
              {adapterList.map((iface) => (
                <label key={iface} className="iface">
                  <input
                    type="checkbox"
                    checked={plan.adapters_5.includes(iface)}
                    disabled={!plan.include_5_ghz}
                    onChange={() => toggleAdapter(iface, '5')}
                  />
                  <code>{iface}</code>
                </label>
              ))}
            </div>
          </div>
        )}
        <label className="field-col mt-4">
          Non-DFS channels
          <input
            value={c5}
            onChange={(e) => {
              setC5(e.target.value)
              void previewFromDraft(draftBody(plan, c24, e.target.value, c5dfs))
            }}
            disabled={!plan.include_5_ghz}
            className="input-block"
          />
        </label>
        <button
          type="button"
          className="secondary mt-4"
          disabled={!plan.include_5_ghz}
          onClick={() => {
            const s = formatChannels(defaults5)
            setC5(s)
            void previewFromDraft(draftBody(plan, c24, s, c5dfs))
          }}
        >
          Reset non-DFS defaults
        </button>
        <div className="mt-4">
          <label>
            <input
              type="checkbox"
              checked={plan.include_dfs}
              disabled={!plan.include_5_ghz}
              onChange={(e) => onBandToggle({ include_dfs: e.target.checked })}
            />{' '}
            Include DFS channels
          </label>
        </div>
        {plan.include_dfs && plan.include_5_ghz && (
          <>
            <label className="field-col mt-4">
              DFS channels
              <input
                value={c5dfs}
                onChange={(e) => {
                  setC5dfs(e.target.value)
                  void previewFromDraft(draftBody(plan, c24, c5, e.target.value))
                }}
                className="input-block"
              />
            </label>
            <button
              type="button"
              className="secondary mt-4"
              onClick={() => {
                const s = formatChannels(defaults5dfs)
                setC5dfs(s)
                void previewFromDraft(draftBody(plan, c24, c5, s))
              }}
            >
              Reset DFS defaults
            </button>
          </>
        )}
      </div>

      <div className="toolbar">
        <button type="button" onClick={() => void save()}>
          Save channel plan
        </button>
        <button type="button" className="secondary" onClick={() => void load()}>
          Reload
        </button>
      </div>
      {msg && <p className="msg-ok">{msg}</p>}

      <div className="panel">
        <h2>Live hopper</h2>
        <p className="muted">
          Capture {runtime?.capture_enabled ? 'enabled' : 'disabled'} · slowest adapter full bucket
          cycle ≈ {effective.hop_cycle_ms} ms ({cycleSec} s)
        </p>
        {runtime && runtime.interfaces.length > 0 ? (
          <table className="table mt-4">
            <thead>
              <tr>
                <th>Interface</th>
                <th>Band</th>
                <th>Current channel</th>
              </tr>
            </thead>
            <tbody>
              {runtime.interfaces.map((row) => (
                <tr key={row.name}>
                  <td>{row.name}</td>
                  <td>{bandForIface(plan, row.name)}</td>
                  <td>{row.current_channel ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p className="muted mt-4">
            No active capture interfaces —{' '}
            <Link to="/adapters">select adapters</Link> first.
          </p>
        )}
      </div>
    </div>
  )
}
