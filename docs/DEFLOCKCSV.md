# DeFlock `deflockcsv` v2

PACK writes Flock alert sessions as **`df-*.deflockcsv`** files (default prefix `df`). Axon OUI (BLE method **35**) hits may be written under **`Axon-*.deflockcsv`** with the same schema. Rows use **WiGLE CSV v1.6 column names and order**, plus trailing Flock extension fields. Wardrive WiGLE captures remain under `wigle/pending/` (see [`README.md`](../README.md)).

## Location

- Root: data directory (`PACK_DATA` or default `./data` relative to the process working directory; see [`backend/src/main.rs`](../backend/src/main.rs)).
- Sessions: `flock/detections/df-<timestamp>.deflockcsv` and, when Axon method fires, `flock/detections/Axon-<timestamp>.deflockcsv` (see [`backend/src/storage.rs`](../backend/src/storage.rs), [`backend/src/runtime.rs`](../backend/src/runtime.rs)).

## File layout

1. **Line 1 — pre-header:** starts with `DeFlock-deflockcsv-2.0`, then comma-separated `key=value` metadata (app release, model, board, …). Same style as WiGLE pre-headers but a different format token.
2. **Line 2 — header:** [`DEFLOCKCSV_HEADER`](../backend/src/deflock_csv.rs) = [`wigle_csv::CSV_HEADER`](../backend/src/wigle_csv.rs) + `,detection_method_id,signal_kind,rssi_min,rssi_max,rssi_avg_q8,wildcard_count,distinct_ch`.
3. **Data rows:** one row per Flock / co-travel alert.

## WiGLE columns (summary)

Same semantics as WiGLE v1.6 WiFi/BLE rows:

- **`MAC`**, **`SSID`**, **`AuthMode`**, **`FirstSeen`**, **`Channel`**, **`Frequency`**, **`RSSI`**, **`CurrentLatitude`**, **`CurrentLongitude`**, **`AltitudeMeters`**, **`AccuracyMeters`**, **`RCOIs`**, **`MfgrId`**, **`Type`**

### `Type` values (DeFlock-specific)

| `Type` | Meaning |
|--------|---------|
| `FLOCK-WIFI` | WiFi Flock detection (methods 1–3) |
| `FLOCK-BLE` | BLE Flock detection (methods 32–35) |
| `FLOCK-COTRAVEL` | Co-travel fire (`detection_method_id` **200**) |

## Extension columns (after `Type`)

- **`detection_method_id`** — numeric ID from [`FlockWifiDetectionMethod` / `FlockBleDetectionMethod`](../backend/src/flock_types.rs) (`as_u8()`): WiFi **1–3**, BLE **32–35**, co-travel **200**. **Human-readable table:** [`DETECTION_METHODS.md`](DETECTION_METHODS.md).
- **`signal_kind`** — `0` = WiFi, `1` = BLE (ESP32 parity).
- **`rssi_min`**, **`rssi_max`**, **`rssi_avg_q8`**, **`wildcard_count`**, **`distinct_ch`** — burst / window stats for WiFi Flock; BLE rows often repeat instantaneous RSSI.

## Follow-up rows

After a MAC has produced at least one alert, the daemon may append **additional** rows for that MAC while it remains in range (same throttle as wardriving WiGLE CSV). Those rows reuse the **original** `detection_method_id` / `signal_kind` from the first alert; `FirstSeen` reflects each observation.

## Version support

- **v2 only** (`DeFlock-deflockcsv-2.0`): map sampling and tooling expect the WiGLE-aligned header (`MAC,SSID,…`). Legacy v1 (`detection_method_id,…` header without WiGLE prefix) and pre-v1 WiGLE-shaped deflock files are **not** read by the map layer.

## Upstream

Derived from the ESP32 wardriver [`docs/DEFLOCKCSV.md`](https://github.com/esp32-wardriver/esp32-wardriver/blob/main/docs/DEFLOCKCSV.md) and adapted for Linux paths, WiGLE column alignment, and v2 extension fields in this tree.
