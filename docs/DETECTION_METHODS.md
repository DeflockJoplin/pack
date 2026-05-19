# Detection methods — status (operator draft)



**Status values**:

| Status | Meaning |
|--------|---------|
| `implemented` | Shipped and on by default (or documented default) |
| `partial` | Works with known gaps; note gaps in **Notes** |
| `experimental` | Behind flag, unstable, or needs field validation |
| `planned` | Designed but not implemented here yet |
| `deprecated` | Most devices will not be detectable with these methods, but some devices that have not been udpated may still alert.  Beware of false positives |
| `disabled` | Implemented but intentionally off / deprecated |
| `n/a` | Not applicable on Linux build |

**Outputs** (typical): DeFlock `*.deflockcsv`, dashboard alert banner, `wardrive.sqlite`. See [`DEFLOCKCSV.md`](DEFLOCKCSV.md); schema header in `backend/src/deflock_csv.rs`.

---

## WiFi — Flock-style (probe / burst heuristics)

Config: `flock_disable_wifi_mask` (bit `(method_id - 1)` disables method), gates in `flock_wifi_min_*`, signatures in `flock_wifi_ie_sig_*`.  
Code: [`backend/src/flock_wifi.rs`](../backend/src/flock_wifi.rs), [`backend/src/flock_types.rs`](../backend/src/flock_types.rs).  
Deep tuning: [`FINGERPRINTING.md`](FINGERPRINTING.md), [`FLOCK_WIFI_FALSE_POSITIVE_CATALOG.md`](FLOCK_WIFI_FALSE_POSITIVE_CATALOG.md).

| ID | Name | Status | Summary | Notes |
|----|------|--------|---------|-------|
| 1 | Wildcard probes + Flock infrastructure OUI | `deprecated` | Broadcast wildcard probe requests from MACs matching Flock OUI allowlist, with rolling-window gates | |
| 2 | Wildcard + IE signature + Flock OUI | `implemented` | Method 1 gates plus primary/alternate IE tag signature + OUI | |
| 3 | Wildcard + IE signature (any source MAC) | `Implemented` | Same IE signature path without OUI requirement | |
| 4–31 | _(reserved)_ | `n/a` | Reserved for future methods |
---

## BLE — Flock / Axon

Config: `flock_disable_ble_mask` (bit `(method_id - 32)`), `scan_ble`, `active_ble_adapters`.  
Code: [`backend/src/flock_ble.rs`](../backend/src/flock_ble.rs), [`backend/src/ble_scan.rs`](../backend/src/ble_scan.rs).

| ID | Name | Status | Summary | Notes |
|----|------|--------|---------|-------|
| 32 | Xuntong manufacturer ID | `deprecated` | Company ID in advertisement | |
| 33 | Advertised name keyword | `deprecated` | Local name / pattern match | |
| 34 | MAC OUI (infrastructure) | `deprecated` | Advertiser MAC prefix | |
| 35 | Axon OUI (`00:25:DF`) | `experimental` | Axon infrastructure OUI | |

---

## SSID watch (user-defined)
Users can define SSIDs to watch for and trigger alerts based on their own field research.  These alerts can trigger on AP beacon or STA directed probe.

Config: `ssid_watch_enabled_probe`, `ssid_watch_enabled_beacon`, `ssid_watch_ssids`, `ssid_watch_per_src_cooldown_ms`.  
Code: [`backend/src/ssid_watch.rs`](../backend/src/ssid_watch.rs), optional CSV via `ssid_watch_csv.rs`.  
Dashboard: Detections / config surfaces (confirm routes in `backend/src/http/routes.rs`).

| Kind | Status | Summary | Notes |
|------|--------|---------|-------|
| Probe request SSID match | `experimental` | Directed or wildcard probes whose SSID IE matches watch list | |
| Beacon / probe-response SSID match | `experimental` | SSID from AP beacons or probe responses | |



---

## Co-travel / follower (experimental feature)

Config: `cotravel` block in `app.json` (`enabled`, `wifi_probes`, `ble`, thresholds, etc.).  
Code: [`backend/src/cotravel.rs`](../backend/src/cotravel.rs) — DeFlock method ID **200** (`COTRAVEL_DETECTION_METHOD_ID`).  
Policy: runs inside home geofence; wardriving CSV may still be suppressed there.

| Aspect | Status | Summary | Notes |
|--------|--------|---------|-------|
| WiFi probe TA tracking | `experimental` | | |
| BLE advertiser tracking | `experimental` | | |
| GPS + movement gates | `experimental` | | |
| False-positive guidance | `experimental` | transit, neighbors, MAC randomization | |

---

## User-defined OUI / custom rules



| Rule type | Status | Summary | Notes |
|-----------|--------|---------|-------|
| OUI prefix watch (WiFi) | `planned` | | |
| Custom alert CSV / method ID range | `planned` | | |


## Changelog

| Date | Author | Change |
|------|--------|--------|
| 5/17/26 | Foggy | Initial stub |
