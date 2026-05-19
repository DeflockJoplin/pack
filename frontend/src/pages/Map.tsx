import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { WardriverMap, type MapLayersData, type MapLayerCounts } from '../components/WardriverMap'
import { useVisibilityPollMs } from '../useDocumentVisible'
import { apiGet } from '../api'
import {
  ALL_FLOCK_METHOD_IDS,
  FLOCK_BLE_METHOD_IDS,
  FLOCK_WIFI_METHOD_IDS,
  mapFiltersFromSearchParams,
  mapFiltersToSearchParams,
  type MapLayerFilters,
} from '../mapLayerFilters'

type ConfigMbtiles = {
  mbtiles_path?: string | null
}

function toggleFlockMethod(methods: number[], id: number, on: boolean): number[] {
  if (on) {
    return methods.includes(id) ? methods : [...methods, id].sort((a, b) => a - b)
  }
  return methods.filter((m) => m !== id)
}

function formatCount(n: number | undefined): string {
  if (n == null) return '—'
  return n.toLocaleString()
}

function LayerCountRow({ label, count }: { label: string; count: number | undefined }) {
  return (
    <div className="map-page__count-row">
      <span>{label}</span>
      <span className="map-page__count-value">{formatCount(count)}</span>
    </div>
  )
}

export function MapPage() {
  const mapPollMs = useVisibilityPollMs(4000)
  const [searchParams, setSearchParams] = useSearchParams()
  const [filters, setFilters] = useState<MapLayerFilters>(() => mapFiltersFromSearchParams(searchParams))
  const [mbtiles, setMbtiles] = useState(false)
  const [layerCounts, setLayerCounts] = useState<MapLayerCounts | null>(null)

  useEffect(() => {
    setFilters(mapFiltersFromSearchParams(searchParams))
  }, [searchParams])

  useEffect(() => {
    void (async () => {
      try {
        const c = await apiGet<ConfigMbtiles>('/api/config')
        setMbtiles(Boolean(c.mbtiles_path?.trim()))
      } catch {
        setMbtiles(false)
      }
    })()
  }, [])

  const syncUrl = useCallback(
    (next: MapLayerFilters) => {
      setFilters(next)
      setSearchParams(mapFiltersToSearchParams(next), { replace: true })
    },
    [setSearchParams],
  )

  const patchFilters = useCallback(
    (patch: Partial<MapLayerFilters>) => {
      syncUrl({ ...filters, ...patch })
    },
    [filters, syncUrl],
  )

  const onLayersUpdate = useCallback((data: MapLayersData) => {
    setLayerCounts(data.layer_counts ?? null)
  }, [])

  const flockMethodChecks = useMemo(
    () => (
      <>
        <p className="map-page__filter-subhead">WiFi methods</p>
        {FLOCK_WIFI_METHOD_IDS.map((id) => (
          <label key={id} className="map-page__filter-check">
            <input
              type="checkbox"
              checked={filters.flock_methods.includes(id)}
              onChange={(e) =>
                patchFilters({
                  flock_methods: toggleFlockMethod(filters.flock_methods, id, e.target.checked),
                })
              }
            />
            Method {id}
          </label>
        ))}
        <p className="map-page__filter-subhead">BLE methods</p>
        {FLOCK_BLE_METHOD_IDS.map((id) => (
          <label key={id} className="map-page__filter-check">
            <input
              type="checkbox"
              checked={filters.flock_methods.includes(id)}
              onChange={(e) =>
                patchFilters({
                  flock_methods: toggleFlockMethod(filters.flock_methods, id, e.target.checked),
                })
              }
            />
            Method {id}
          </label>
        ))}
        <button
          type="button"
          className="map-page__filter-link"
          onClick={() => patchFilters({ flock_methods: [...ALL_FLOCK_METHOD_IDS] })}
        >
          All methods
        </button>
      </>
    ),
    [filters.flock_methods, patchFilters],
  )

  const counts = layerCounts

  return (
    <div className="map-page stack">
      <div className="page-head">
        <h2 className="page-head__title">Offline map</h2>
        <p className="muted page-head__lead">
          Set <code>mbtiles_path</code> in <code>data/config/app.json</code> (or <code>POST /api/config</code>) to a
          raster <code>.mbtiles</code> file. Tiles at <code>/api/map/tiles/&lt;z&gt;/&lt;x&gt;/&lt;y&gt;</code>. Layer
          filters sync to the URL. Purple: co-travel fires; amber: suspects (
          <Link to="/cotravel">Co-travel tuning</Link>). GPS track: <Link to="/privacy">Privacy</Link>.
        </p>
        <p className="muted page-head__lead" style={{ marginTop: '0.5rem' }}>
          MBTiles: <strong>{mbtiles ? 'yes' : 'no'}</strong>
          {!mbtiles && ' — raster tiles via local OSM/CARTO proxy.'} Pan and zoom; layers refresh every few seconds.
        </p>
      </div>

      <div className="map-page__layout">
        <aside className="panel map-page__sidebar">
          <h3 className="map-page__sidebar-title">Layers</h3>

          <fieldset className="map-page__filter-group">
            <legend>Wardriving</legend>
            <label className="map-page__filter-check">
              <input
                type="checkbox"
                checked={filters.include_wigle_wifi}
                onChange={(e) => patchFilters({ include_wigle_wifi: e.target.checked })}
              />
              WiFi AP
            </label>
            <label className="map-page__filter-check">
              <input
                type="checkbox"
                checked={filters.include_wigle_probe}
                onChange={(e) => patchFilters({ include_wigle_probe: e.target.checked })}
              />
              Probe requests
            </label>
            <label className="map-page__filter-check">
              <input
                type="checkbox"
                checked={filters.include_wigle_ble}
                onChange={(e) => patchFilters({ include_wigle_ble: e.target.checked })}
              />
              BLE
            </label>
          </fieldset>

          <fieldset className="map-page__filter-group">
            <legend>Alerts</legend>
            <label className="map-page__filter-check">
              <input
                type="checkbox"
                checked={filters.include_flock}
                onChange={(e) => patchFilters({ include_flock: e.target.checked })}
              />
              Flock
            </label>
            <label className="map-page__filter-check">
              <input
                type="checkbox"
                checked={filters.include_cotravel_fires}
                onChange={(e) => patchFilters({ include_cotravel_fires: e.target.checked })}
              />
              Co-travel fires
            </label>
            <label className="map-page__filter-check">
              <input
                type="checkbox"
                checked={filters.include_cotravel_suspects}
                onChange={(e) => patchFilters({ include_cotravel_suspects: e.target.checked })}
              />
              Co-travel suspects
            </label>
          </fieldset>

          <fieldset className="map-page__filter-group">
            <legend>Flock filters</legend>
            <label className="map-page__filter-check map-page__filter-check--row">
              Signal
              <select
                className="map-page__select"
                value={filters.flock_signal}
                onChange={(e) =>
                  patchFilters({ flock_signal: e.target.value as MapLayerFilters['flock_signal'] })
                }
              >
                <option value="all">All</option>
                <option value="wifi">WiFi</option>
                <option value="ble">BLE</option>
              </select>
            </label>
            {flockMethodChecks}
          </fieldset>

          <fieldset className="map-page__filter-group">
            <legend>Map</legend>
            <label className="map-page__filter-check">
              <input
                type="checkbox"
                checked={filters.include_track}
                onChange={(e) => patchFilters({ include_track: e.target.checked })}
              />
              GPS track
            </label>
            <label className="map-page__filter-check">
              <input
                type="checkbox"
                checked={filters.include_grid}
                onChange={(e) => patchFilters({ include_grid: e.target.checked })}
              />
              Coverage grid
            </label>
            <label className="map-page__filter-check">
              <input
                type="checkbox"
                checked={filters.followGps}
                onChange={(e) => patchFilters({ followGps: e.target.checked })}
              />
              Follow GPS
            </label>
          </fieldset>

          <div className="map-page__counts">
            <h4 className="map-page__counts-title">Layer counts</h4>
            <LayerCountRow label="Track" count={counts?.track} />
            <LayerCountRow label="Grid" count={counts?.grid} />
            <LayerCountRow label="WiFi AP" count={counts?.wigle_wifi} />
            <LayerCountRow label="Probe" count={counts?.wigle_probe} />
            <LayerCountRow label="BLE" count={counts?.wigle_ble} />
            <LayerCountRow label="Flock" count={counts?.flock} />
            <LayerCountRow label="Co-travel fires" count={counts?.cotravel_fires} />
            <LayerCountRow label="Co-travel suspects" count={counts?.cotravel_suspects} />
          </div>
        </aside>

        <div className="map-page__map-col">
          <WardriverMap
            filters={filters}
            pollIntervalMs={mapPollMs}
            mapClassName="map-page__canvas"
            onLayersUpdate={onLayersUpdate}
          />
        </div>
      </div>
    </div>
  )
}
