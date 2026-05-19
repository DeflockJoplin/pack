/** Layer toggles and Flock filters for `GET /api/map/layers`. */
export type MapLayerFilters = {
  include_track: boolean
  include_grid: boolean
  include_wigle_wifi: boolean
  include_wigle_probe: boolean
  include_wigle_ble: boolean
  include_flock: boolean
  include_cotravel_fires: boolean
  include_cotravel_suspects: boolean
  flock_methods: number[]
  flock_signal: 'wifi' | 'ble' | 'all'
  followGps: boolean
}

export const FLOCK_WIFI_METHOD_IDS = [1, 2, 3] as const
export const FLOCK_BLE_METHOD_IDS = [32, 33, 34, 35] as const
export const ALL_FLOCK_METHOD_IDS: number[] = [...FLOCK_WIFI_METHOD_IDS, ...FLOCK_BLE_METHOD_IDS]

export const defaultMapLayerFilters = (): MapLayerFilters => ({
  include_track: true,
  include_grid: true,
  include_wigle_wifi: true,
  include_wigle_probe: true,
  include_wigle_ble: true,
  include_flock: true,
  include_cotravel_fires: true,
  include_cotravel_suspects: true,
  flock_methods: [...ALL_FLOCK_METHOD_IDS],
  flock_signal: 'all',
  followGps: false,
})

function parseBool(v: string | null, fallback: boolean): boolean {
  if (v == null || v === '') return fallback
  const t = v.toLowerCase()
  if (t === '1' || t === 'true' || t === 'yes') return true
  if (t === '0' || t === 'false' || t === 'no') return false
  return fallback
}

function parseFlockMethods(v: string | null, fallback: number[]): number[] {
  if (v == null || v === '') return fallback
  const ids = v
    .split(',')
    .map((s) => parseInt(s.trim(), 10))
    .filter((n) => Number.isFinite(n) && ALL_FLOCK_METHOD_IDS.includes(n))
  return ids.length > 0 ? ids : fallback
}

function parseFlockSignal(v: string | null, fallback: MapLayerFilters['flock_signal']): MapLayerFilters['flock_signal'] {
  if (v === 'wifi' || v === 'ble' || v === 'all') return v
  return fallback
}

export function mapFiltersFromSearchParams(params: URLSearchParams): MapLayerFilters {
  const d = defaultMapLayerFilters()
  return {
    include_track: parseBool(params.get('include_track'), d.include_track),
    include_grid: parseBool(params.get('include_grid'), d.include_grid),
    include_wigle_wifi: parseBool(params.get('include_wigle_wifi'), d.include_wigle_wifi),
    include_wigle_probe: parseBool(params.get('include_wigle_probe'), d.include_wigle_probe),
    include_wigle_ble: parseBool(params.get('include_wigle_ble'), d.include_wigle_ble),
    include_flock: parseBool(params.get('include_flock'), d.include_flock),
    include_cotravel_fires: parseBool(params.get('include_cotravel_fires'), d.include_cotravel_fires),
    include_cotravel_suspects: parseBool(params.get('include_cotravel_suspects'), d.include_cotravel_suspects),
    flock_methods: parseFlockMethods(params.get('flock_methods'), d.flock_methods),
    flock_signal: parseFlockSignal(params.get('flock_signal'), d.flock_signal),
    followGps: parseBool(params.get('follow_gps'), d.followGps),
  }
}

export function mapFiltersToSearchParams(filters: MapLayerFilters): URLSearchParams {
  const p = new URLSearchParams()
  const setBool = (key: string, v: boolean) => p.set(key, v ? '1' : '0')
  setBool('include_track', filters.include_track)
  setBool('include_grid', filters.include_grid)
  setBool('include_wigle_wifi', filters.include_wigle_wifi)
  setBool('include_wigle_probe', filters.include_wigle_probe)
  setBool('include_wigle_ble', filters.include_wigle_ble)
  setBool('include_flock', filters.include_flock)
  setBool('include_cotravel_fires', filters.include_cotravel_fires)
  setBool('include_cotravel_suspects', filters.include_cotravel_suspects)
  setBool('follow_gps', filters.followGps)
  if (filters.flock_methods.length > 0 && filters.flock_methods.length < ALL_FLOCK_METHOD_IDS.length) {
    p.set('flock_methods', filters.flock_methods.slice().sort((a, b) => a - b).join(','))
  } else if (filters.flock_methods.length === 0) {
    p.set('flock_methods', '')
  }
  if (filters.flock_signal !== 'all') {
    p.set('flock_signal', filters.flock_signal)
  }
  return p
}

export function buildMapLayersQuery(z: number, filters: MapLayerFilters): string {
  const p = new URLSearchParams()
  p.set('z', String(z))
  const setBool = (key: string, v: boolean) => p.set(key, v ? '1' : '0')
  setBool('include_track', filters.include_track)
  setBool('include_grid', filters.include_grid)
  setBool('include_wigle_wifi', filters.include_wigle_wifi)
  setBool('include_wigle_probe', filters.include_wigle_probe)
  setBool('include_wigle_ble', filters.include_wigle_ble)
  setBool('include_flock', filters.include_flock)
  setBool('include_cotravel_fires', filters.include_cotravel_fires)
  setBool('include_cotravel_suspects', filters.include_cotravel_suspects)
  if (filters.flock_methods.length > 0) {
    p.set('flock_methods', filters.flock_methods.slice().sort((a, b) => a - b).join(','))
  }
  p.set('flock_signal', filters.flock_signal)
  return p.toString()
}
