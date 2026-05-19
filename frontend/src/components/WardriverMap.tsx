import { useCallback, useEffect, useRef, useState } from 'react'
import L from 'leaflet'
import 'leaflet/dist/leaflet.css'
import { apiGet } from '../api'
import { buildMapLayersQuery, type MapLayerFilters } from '../mapLayerFilters'

export type MapLayerCounts = {
  track?: number
  grid?: number
  wigle_wifi?: number
  wigle_probe?: number
  wigle_ble?: number
  flock?: number
  cotravel_fires?: number
  cotravel_suspects?: number
}

export type MapLayersData = {
  mbtiles_configured: boolean
  tiles_url: string
  map_basemap?: 'dark' | 'light'
  tile_attribution?: string
  current_lat: number | null
  current_lon: number | null
  gps_fix?: boolean
  hdop: number | null
  track: Array<{ lat: number; lon: number; t_ms: number }>
  grid: Array<{ key: string; lat: number; lon: number; count: number }>
  wigle_points: Array<{ lat: number; lon: number; ssid: string; row_type: string; rssi: number }>
  alerts: string[]
  flock_map_pins?: Array<{
    lat: number
    lon: number
    mac: string
    t_ms: number
    signal_kind: string
    method_id: number
    method_label: string
  }>
  cotravel_pins: Array<{
    lat: number
    lon: number
    mac: string
    t_ms: number
    source: string
    duration_s: number
    track_distance_m: number
  }>
  cotravel_suspects?: Array<{
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
  layer_counts?: MapLayerCounts
}

export type { MapLayerFilters }

const LAYER_COLORS = {
  track: '#3388ff',
  current: '#f44',
  wigle_wifi: '#2c7be5',
  wigle_probe: '#17a2b8',
  wigle_ble: '#e67e22',
  grid: '#2ecc71',
  flock: '#e74c3c',
  cotravel_fires: '#c39bd3',
  cotravel_suspects: '#f5b041',
} as const

export type WardriverMapProps = {
  filters: MapLayerFilters
  pollIntervalMs: number
  mapClassName: string
  onLayersUpdate?: (data: MapLayersData) => void
  showFullscreenControl?: boolean
  showLegend?: boolean
}

function invalidateMapSoon(map: L.Map | null) {
  if (!map) return
  requestAnimationFrame(() => {
    map.invalidateSize()
    window.setTimeout(() => map.invalidateSize(), 150)
  })
}

function wigleRowKind(rowType: string): 'wifi' | 'probe' | 'ble' | null {
  const t = rowType.toUpperCase()
  if (t === 'WIFI') return 'wifi'
  if (t === 'PROBE') return 'probe'
  if (t === 'BLE') return 'ble'
  return null
}

export function WardriverMap({
  filters,
  pollIntervalMs,
  mapClassName,
  onLayersUpdate,
  showFullscreenControl = true,
  showLegend = true,
}: WardriverMapProps) {
  const wrapRef = useRef<HTMLDivElement>(null)
  const mapEl = useRef<HTMLDivElement>(null)
  const mapRef = useRef<L.Map | null>(null)
  const layersRef = useRef<{
    base: L.TileLayer | null
    track: L.Polyline | null
    current: L.CircleMarker | null
    wigle: L.LayerGroup | null
    grid: L.LayerGroup | null
    flock: L.LayerGroup | null
    cotravelFires: L.LayerGroup | null
    cotravelSuspects: L.LayerGroup | null
  }>({
    base: null,
    track: null,
    current: null,
    wigle: null,
    grid: null,
    flock: null,
    cotravelFires: null,
    cotravelSuspects: null,
  })
  const baseKeyRef = useRef<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [fullscreen, setFullscreen] = useState(false)
  const [lastData, setLastData] = useState<MapLayersData | null>(null)
  const didFitTrackOnlyRef = useRef(false)
  const didAutoFitRef = useRef(false)

  const applyLayers = useCallback(
    (data: MapLayersData) => {
      const map = mapRef.current
      if (!map) return

      const prev = layersRef.current
      const useMbtiles = data.mbtiles_configured
      const tileUrl = data.tiles_url || '/api/map/tiles/{z}/{x}/{y}'
      const attribution =
        data.tile_attribution ??
        (useMbtiles
          ? 'Local MBTiles · © OpenStreetMap contributors'
          : '© OpenStreetMap contributors (local tile proxy)')

      const baseKey = `${useMbtiles}:${attribution}`
      const needNewBase = !prev.base || baseKeyRef.current !== baseKey
      if (needNewBase) {
        if (prev.base) {
          map.removeLayer(prev.base)
        }
        const base = L.tileLayer(tileUrl, {
          maxZoom: 22,
          maxNativeZoom: 19,
          attribution,
        })
        base.addTo(map)
        layersRef.current.base = base
        baseKeyRef.current = baseKey
      }

      if (prev.track) map.removeLayer(prev.track)
      if (data.track.length >= 2) {
        const latlngs = data.track.map((p) => L.latLng(p.lat, p.lon))
        const pl = L.polyline(latlngs, { color: LAYER_COLORS.track, weight: 3, opacity: 0.85 })
        pl.addTo(map)
        layersRef.current.track = pl
      } else {
        layersRef.current.track = null
      }

      if (prev.current) map.removeLayer(prev.current)
      if (data.current_lat != null && data.current_lon != null) {
        const fixYes = data.gps_fix === true
        const cm = L.circleMarker([data.current_lat, data.current_lon], {
          radius: 8,
          color: '#c22',
          fillColor: LAYER_COLORS.current,
          fillOpacity: 0.9,
          weight: 2,
        })
        const hdopLine =
          fixYes && data.hdop != null ? `<br/>HDOP: ${data.hdop.toFixed(2)}` : ''
        cm.bindPopup(`Fix: ${fixYes ? 'Yes' : 'No'}${hdopLine}`)
        cm.addTo(map)
        layersRef.current.current = cm
      } else {
        layersRef.current.current = null
      }

      if (prev.wigle) map.removeLayer(prev.wigle)
      if (data.wigle_points.length > 0) {
        const wg = L.layerGroup()
        for (const p of data.wigle_points) {
          const kind = wigleRowKind(p.row_type)
          const color =
            kind === 'ble'
              ? LAYER_COLORS.wigle_ble
              : kind === 'probe'
                ? LAYER_COLORS.wigle_probe
                : LAYER_COLORS.wigle_wifi
          const m = L.circleMarker([p.lat, p.lon], {
            radius: 4,
            color,
            fillColor: color,
            fillOpacity: 0.65,
            weight: 1,
          })
          m.bindPopup(`${p.row_type}: ${p.ssid || '(hidden)'}<br/>RSSI ${p.rssi}`)
          wg.addLayer(m)
        }
        wg.addTo(map)
        layersRef.current.wigle = wg
      } else {
        layersRef.current.wigle = null
      }

      if (prev.grid) map.removeLayer(prev.grid)
      if (data.grid.length > 0) {
        const gr = L.layerGroup()
        for (const c of data.grid) {
          const radius = 3 + Math.min(10, Math.log2(1 + c.count))
          const m = L.circleMarker([c.lat, c.lon], {
            radius,
            color: '#1a7f37',
            fillColor: LAYER_COLORS.grid,
            fillOpacity: 0.25,
            weight: 0,
          })
          m.bindTooltip(`visits ×${c.count}`, { sticky: true })
          gr.addLayer(m)
        }
        gr.addTo(map)
        layersRef.current.grid = gr
      } else {
        layersRef.current.grid = null
      }

      if (prev.flock) map.removeLayer(prev.flock)
      const flockPins = data.flock_map_pins ?? []
      if (flockPins.length > 0) {
        const flockLayer = L.layerGroup()
        for (const p of flockPins) {
          const m = L.circleMarker([p.lat, p.lon], {
            radius: 7,
            color: '#c0392b',
            fillColor: LAYER_COLORS.flock,
            fillOpacity: 0.85,
            weight: 2,
          })
          m.bindPopup(
            `Flock (${p.signal_kind})<br/>${p.method_label}<br/>MAC ${p.mac}`,
          )
          flockLayer.addLayer(m)
        }
        flockLayer.addTo(map)
        layersRef.current.flock = flockLayer
      } else {
        layersRef.current.flock = null
      }

      if (prev.cotravelFires) map.removeLayer(prev.cotravelFires)
      if (data.cotravel_pins.length > 0) {
        const fires = L.layerGroup()
        for (const p of data.cotravel_pins) {
          const m = L.circleMarker([p.lat, p.lon], {
            radius: 7,
            color: '#9b59b6',
            fillColor: LAYER_COLORS.cotravel_fires,
            fillOpacity: 0.85,
            weight: 2,
          })
          m.bindPopup(
            `Co-travel fired (${p.source})<br/>MAC ${p.mac}<br/>Duration ${p.duration_s.toFixed(0)}s · path ~${p.track_distance_m.toFixed(0)} m`,
          )
          fires.addLayer(m)
        }
        fires.addTo(map)
        layersRef.current.cotravelFires = fires
      } else {
        layersRef.current.cotravelFires = null
      }

      if (prev.cotravelSuspects) map.removeLayer(prev.cotravelSuspects)
      const sus = data.cotravel_suspects ?? []
      if (sus.length > 0) {
        const suspects = L.layerGroup()
        for (const s of sus) {
          const m = L.circleMarker([s.lat, s.lon], {
            radius: 5,
            color: '#b9770e',
            fillColor: LAYER_COLORS.cotravel_suspects,
            fillOpacity: 0.75,
            weight: 1,
          })
          m.bindPopup(
            `Co-travel suspect<br/>MAC ${s.mac}<br/>Score ${s.score.toFixed(2)} · ${s.sightings} sightings<br/>` +
              `${s.streak_duration_s.toFixed(0)}s streak · ${s.user_path_m.toFixed(0)} m path · RSSI ${s.rssi}` +
              (s.in_cooldown ? '<br/><em>In cooldown</em>' : ''),
          )
          suspects.addLayer(m)
        }
        suspects.addTo(map)
        layersRef.current.cotravelSuspects = suspects
      } else {
        layersRef.current.cotravelSuspects = null
      }

      const followGps = filters.followGps
      if (followGps) {
        if (data.current_lat != null && data.current_lon != null) {
          const z = Math.min(18, Math.max(14, map.getZoom() || 16))
          map.setView([data.current_lat, data.current_lon], z, { animate: true })
          didFitTrackOnlyRef.current = true
        } else if (data.track.length >= 2 && !didFitTrackOnlyRef.current) {
          const latlngs = data.track.map((p) => L.latLng(p.lat, p.lon))
          map.fitBounds(L.latLngBounds(latlngs), { padding: [40, 40], maxZoom: 16 })
          didFitTrackOnlyRef.current = true
        }
      } else if (!didAutoFitRef.current) {
        const bounds: L.LatLngTuple[] = []
        if (data.track.length) {
          for (const p of data.track) bounds.push([p.lat, p.lon])
        }
        if (data.current_lat != null && data.current_lon != null) {
          bounds.push([data.current_lat, data.current_lon])
        }
        for (const p of data.wigle_points) bounds.push([p.lat, p.lon])
        for (const p of flockPins) bounds.push([p.lat, p.lon])
        for (const p of data.cotravel_pins) bounds.push([p.lat, p.lon])
        for (const s of sus) bounds.push([s.lat, s.lon])
        if (bounds.length === 1) {
          map.setView(bounds[0], 15)
          didAutoFitRef.current = true
        } else if (bounds.length >= 2) {
          map.fitBounds(L.latLngBounds(bounds), { padding: [40, 40], maxZoom: 16 })
          didAutoFitRef.current = true
        }
      }
    },
    [filters.followGps],
  )

  useEffect(() => {
    didFitTrackOnlyRef.current = false
    didAutoFitRef.current = false
  }, [filters.followGps])

  useEffect(() => {
    const wrap = wrapRef.current
    if (!wrap) return

    const syncFs = () => {
      const doc = document as Document & { webkitFullscreenElement?: Element | null }
      const active = document.fullscreenElement ?? doc.webkitFullscreenElement ?? null
      setFullscreen(active === wrap)
      invalidateMapSoon(mapRef.current)
    }

    document.addEventListener('fullscreenchange', syncFs)
    document.addEventListener('webkitfullscreenchange', syncFs)
    return () => {
      document.removeEventListener('fullscreenchange', syncFs)
      document.removeEventListener('webkitfullscreenchange', syncFs)
    }
  }, [])

  const toggleFullscreen = useCallback(async () => {
    const el = wrapRef.current
    if (!el) return
    try {
      const doc = document as Document & {
        webkitFullscreenElement?: Element | null
        webkitExitFullscreen?: () => Promise<void>
      }
      const active = document.fullscreenElement ?? doc.webkitFullscreenElement
      if (active) {
        if (document.exitFullscreen) await document.exitFullscreen()
        else await doc.webkitExitFullscreen?.()
      } else if (el.requestFullscreen) {
        await el.requestFullscreen()
      } else {
        await (el as HTMLElement & { webkitRequestFullscreen?: () => Promise<void> }).webkitRequestFullscreen?.()
      }
    } catch {
      /* user gesture / browser policy */
    }
  }, [])

  useEffect(() => {
    if (!mapEl.current || mapRef.current) return
    const map = L.map(mapEl.current, { zoomControl: true })
    map.setView([37.4419, -122.143], 3)
    mapRef.current = map
    const onResize = () => map.invalidateSize()
    window.addEventListener('resize', onResize)
    return () => {
      window.removeEventListener('resize', onResize)
      map.remove()
      mapRef.current = null
      baseKeyRef.current = null
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    const tick = async () => {
      try {
        const map = mapRef.current
        const z = map ? Math.max(0, Math.min(22, Math.round(map.getZoom()))) : 14
        const qs = buildMapLayersQuery(z, filters)
        const data = await apiGet<MapLayersData>(`/api/map/layers?${qs}`)
        if (!cancelled) {
          setErr(null)
          setLastData(data)
          applyLayers(data)
          onLayersUpdate?.(data)
        }
      } catch (e) {
        if (!cancelled) setErr(e instanceof Error ? e.message : String(e))
      }
    }
    void tick()
    const id = window.setInterval(() => void tick(), pollIntervalMs)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [applyLayers, pollIntervalMs, onLayersUpdate, filters])

  const legendItems: Array<{ key: string; label: string; color: string; show: boolean }> = []
  if (filters.include_track) {
    legendItems.push({ key: 'track', label: 'GPS track', color: LAYER_COLORS.track, show: (lastData?.track.length ?? 0) >= 2 })
  }
  legendItems.push({ key: 'current', label: 'Current fix', color: LAYER_COLORS.current, show: lastData?.current_lat != null })
  if (filters.include_wigle_wifi) {
    legendItems.push({
      key: 'wigle_wifi',
      label: 'WiFi AP',
      color: LAYER_COLORS.wigle_wifi,
      show: (lastData?.wigle_points ?? []).some((p) => wigleRowKind(p.row_type) === 'wifi'),
    })
  }
  if (filters.include_wigle_probe) {
    legendItems.push({
      key: 'wigle_probe',
      label: 'Probe',
      color: LAYER_COLORS.wigle_probe,
      show: (lastData?.wigle_points ?? []).some((p) => wigleRowKind(p.row_type) === 'probe'),
    })
  }
  if (filters.include_wigle_ble) {
    legendItems.push({
      key: 'wigle_ble',
      label: 'BLE',
      color: LAYER_COLORS.wigle_ble,
      show: (lastData?.wigle_points ?? []).some((p) => wigleRowKind(p.row_type) === 'ble'),
    })
  }
  if (filters.include_grid) {
    legendItems.push({ key: 'grid', label: 'Coverage grid', color: LAYER_COLORS.grid, show: (lastData?.grid.length ?? 0) > 0 })
  }
  if (filters.include_flock) {
    legendItems.push({
      key: 'flock',
      label: 'Flock alerts',
      color: LAYER_COLORS.flock,
      show: (lastData?.flock_map_pins?.length ?? 0) > 0,
    })
  }
  if (filters.include_cotravel_fires) {
    legendItems.push({
      key: 'cotravel_fires',
      label: 'Co-travel fires',
      color: LAYER_COLORS.cotravel_fires,
      show: (lastData?.cotravel_pins.length ?? 0) > 0,
    })
  }
  if (filters.include_cotravel_suspects) {
    legendItems.push({
      key: 'cotravel_suspects',
      label: 'Co-travel suspects',
      color: LAYER_COLORS.cotravel_suspects,
      show: (lastData?.cotravel_suspects?.length ?? 0) > 0,
    })
  }

  return (
    <div ref={wrapRef} className="wardriver-map-wrap">
      {err && <p className="error wardriver-map__error">{err}</p>}
      {showFullscreenControl && (
        <button
          type="button"
          className="wardriver-map__fullscreen"
          onClick={() => void toggleFullscreen()}
          aria-pressed={fullscreen}
        >
          {fullscreen ? 'Exit fullscreen' : 'Fullscreen'}
        </button>
      )}
      {showLegend && legendItems.length > 0 && (
        <div className="wardriver-map__legend" aria-label="Map layer legend">
          <div className="wardriver-map__legend-title">Layers</div>
          <ul className="wardriver-map__legend-list">
            {legendItems.map((item) => (
              <li key={item.key} className={item.show ? '' : 'wardriver-map__legend-item--empty'}>
                <span className="wardriver-map__legend-swatch" style={{ background: item.color }} />
                {item.label}
              </li>
            ))}
          </ul>
        </div>
      )}
      <div ref={mapEl} className={mapClassName} />
    </div>
  )
}
