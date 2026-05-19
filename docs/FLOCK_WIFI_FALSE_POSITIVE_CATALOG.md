# Flock WiFi false-positive catalog (OUI collisions)

Some **Flock infrastructure OUIs** are shared with other products using the same radio modules. When you see a **broadcast wildcard probe** (explicit SSID IE length 0, or **no SSID IE** after a full parse—same rule as the live detector) **plus OUI match**, and the device is **not** a Flock camera, add a row here and consider tightening via:

- `flock_wifi_min_wildcards_in_window`, `flock_wifi_min_distinct_channels`, or `flock_wifi_min_rssi_span` in `data/config/app.json`, and/or
- An **IE signature** allow/deny list (cluster with [`scripts/flock_probe_ie_sig.py`](../scripts/flock_probe_ie_sig.py); see [`docs/FINGERPRINTING.md`](FINGERPRINTING.md)).

| Date (UTC) | SA OUI (3 bytes) | Full example MAC | Device (actual) | Wildcard IE signature (abbrev.) | Confirmed not Flock | Notes |
|------------|------------------|------------------|-----------------|-----------------------------------|----------------------|-------|
| *(template)* | `aa:bb:cc` | | | | yes | Fill from your own captures. |

Keep one representative **IE signature** string per false positive (copy from script JSON `ie_sig`). Link to the PCAP filename if it lives outside git (e.g. under `data/test/`).
