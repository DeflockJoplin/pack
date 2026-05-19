# Fingerprinting data and licensing

## In-repo curated rules

[`backend/data/fingerprints/default.json`](../backend/data/fingerprints/default.json) is project-authored under the same [MIT License](../LICENSE) as the repo. Replace entries with signatures observed on your own captures—do not copy GPL databases (e.g. Kismet, Wireshark `manuf`) into this file without a compliance plan.

## Custom curated fingerprints (Nearby page)

The dashboard **Nearby** view can show **`curated_hint`** and **`curated_confidence`** when a device’s frame-derived fields match a rule in `default.json`. Rules are compiled into the binary via `include_str!` in [`backend/src/curated_fp.rs`](../backend/src/curated_fp.rs)—**edit the JSON, then rebuild** (`cargo build -p pack` or `bash scripts/bootstrap.sh`).

### `default.json` schema

The file is a **JSON array** of objects:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `ie_sig` | string | yes | Match key (see below). |
| `hint` | string | yes | Short label shown in the UI (your own wording). |
| `confidence` | number | no | `0.0`–`1.0`; default **`0.5`** if omitted. |

**Matching behavior** ([`curated_fp.rs`](../backend/src/curated_fp.rs)):

- **WiFi STA (probe requests):** `ie_sig` must **exactly equal** `probe_ie_tag_seq` — the comma-separated `tag:length` list from [`probe_req_ie_tag_sequence`](../backend/src/ieee80211.rs) (SSID tag **0** is included, e.g. `0:0,1:8,45:26`).
- **WiFi AP / beacon paths:** `ie_sig` is matched as a **substring** of the pipe-joined `vendor_ie_sigs` list (vendor IE summaries), not the probe tag sequence.

Do not confuse this with **Flock** probe clustering signatures (SSID tag omitted, vendor bodies as `221:…` hex)—those belong in `flock_wifi_ie_sig_*` config and `scripts/flock_probe_ie_sig.py`, not in `default.json`.

### Workflow: from capture to rule

1. **Capture** probe traffic legally on hardware you control (monitor mode PCAP-NG under `data/recon/` or your own files).
2. **Optional clustering (Flock-style IE tails):** use the repo venv and [`scripts/flock_probe_ie_sig.py`](../scripts/flock_probe_ie_sig.py) to group wildcard probes and inspect vendor IE tails (see [Flock WiFi probe IE clustering](#flock-wifi-probe-ie-clustering-offline) below). Use that output to **find candidate devices**, not as the literal `ie_sig` for `default.json` unless you translate to `tag:length` form.
3. **Curated `ie_sig` for STA rules:** copy **`probe_ie_tag_seq`** from the Nearby API/device row after a live run, or derive it with the same logic as `probe_req_ie_tag_sequence` (unit test `probe_ie_tag_sequence_curated_placeholder` in [`ieee80211.rs`](../backend/src/ieee80211.rs) shows the placeholder chain).
4. **Edit** [`backend/data/fingerprints/default.json`](../backend/data/fingerprints/default.json) and add/replace objects.
5. **Rebuild** so the embedded JSON updates:

   ```bash
   cargo build --release -p pack
   ```

6. Restart the daemon and confirm hints on **Nearby** for matching STAs.

### Licensing

- Rules you add must be **your own observations** or data you have rights to redistribute under the repo’s [MIT License](../LICENSE).
- Do **not** paste entries from GPL or “all rights reserved” fingerprint databases without a separate compliance review.

### Example: replace the placeholder

The shipped placeholder is a single low-confidence example. After you have a real `probe_ie_tag_seq` from your environment, replace the array entry:

```json
[
  {
    "ie_sig": "0:0,1:8,45:26",
    "hint": "Lab AP test handset (May 2026 capture)",
    "confidence": 0.75
  }
]
```

Rebuild, run `./target/release/pack`, and trigger a probe from that STA on a monitored interface; the Nearby row should show your `hint` and `confidence`.

## OUI and BLE company tables

- **OUI**: Built from [`backend/data/oui_prefixes.txt`](../backend/data/oui_prefixes.txt); see [OUI_DATA.md](OUI_DATA.md) for regen and IEEE terms.
- **BLE company IDs**: Built from [`backend/data/ble_company_ids.txt`](../backend/data/ble_company_ids.txt) (optional). For Bluetooth SIG Assigned Numbers, refresh this file periodically and cite the publication date in your release notes; review [Bluetooth.com terms of use](https://www.bluetooth.com/about-us/terms-of-use/) for your distribution model.

Build output is described in [`backend/NOTICE`](../backend/NOTICE).

## Frame-derived fields

802.11 IE tag sequences, WPS attributes, vendor IE prefixes, RSN/WPA cipher and AKM summaries, MFP bits, Interworking (802.11u) access options, HE/HT/VHT presence, and BLE advertising fields (including secondary manufacturer signatures) are derived from passive observation and do not embed third-party databases.

## Flock WiFi probe IE clustering (offline)

Use a **repo-local virtual environment** (do not install tooling system-wide):

```bash
python3 -m venv tools/pcap-venv
. tools/pcap-venv/bin/activate
pip install -r tools/requirements-pcap.txt
python scripts/flock_probe_ie_sig.py data/test/*.pcapng
```

By default the script counts **broadcast wildcard** probe requests whose **source MAC** matches the Flock infrastructure OUI list in [`backend/src/flock_oui.rs`](../backend/src/flock_oui.rs). A wildcard is either an **SSID IE (tag 0) with length 0**, or—after a full IE walk with no truncation—**no SSID IE at all** (Wireshark often still labels these “SSID=Wildcard (Broadcast)”). Pass `--strict-ssid-only` for analysis that counts **only** explicit zero-length SSID IEs (excludes implicit / no-SSID-IE probes). Pass `--all-probes` to cluster every matching probe regardless of OUI.

The script and `probe_req_flock_ie_sig_for_clustering` in [`backend/src/ieee80211.rs`](../backend/src/ieee80211.rs) share the same IE walk for clustering: **consecutive SSID (tag 0) length-0 headers are coalesced** (alignment after padding), a narrow **phantom overflow** recovery skips two bytes when `(id 64, len 128)` with a Lite-On vendor resync ahead (or `len > 200`) would otherwise truncate the TLV chain, otherwise the walk **scans forward** for the next valid `(id, len)` pair, tries **offset 26** when an explicit zero-length SSID and supported-rates header precede the nominal IE offset, and **canonicalizes** signatures that contain the Lite-On vendor anchor (`221:506f9a16030103`) to the `2,12,127,…` prefix. The better of **full MPDU** vs **last four octets stripped** (possible FCS) is chosen so stripping does not cut off the trailing vendor IE. Run `python scripts/flock_probe_ie_sig.py --self-test` for an offline assertion on a Linux-captured Flock wildcard probe frame (expects the canonical built-in DEFAULT, not the legacy ALT parse variant). Test PCAPs are local-only; see [`TEST_CAPTURES.md`](TEST_CAPTURES.md).

`FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX` is a **parse-variant workaround** (misalignment before phantom `40:80`), not a separate hardware fingerprint. It remains in the runtime allowlist during bake-in; retire it when `flock_wifi_ie_sig_match_builtin_alt_linux` stays near zero on Linux drives.

`/api/stats` (and diagnostics, which embeds the same object) exposes IE signature counters:

| Field | Meaning |
|-------|---------|
| `flock_wifi_ie_sig_match_builtin_default` | Methods **2–3** alert fired with built-in DEFAULT match |
| `flock_wifi_ie_sig_match_builtin_alt_linux` | Methods **2–3** alert with built-in ALT_LINUX match |
| `flock_wifi_ie_sig_match_config` | Methods **2–3** alert with config primary or alternate match |
| `flock_wifi_ie_sig_computed_builtin_default` | Every wildcard probe where computed sig equals DEFAULT |
| `flock_wifi_ie_sig_computed_builtin_alt_linux` | Every wildcard probe where computed sig equals ALT_LINUX |
| `flock_wifi_ie_sig_computed_other` | Computed sig present but neither built-in primary |

Output is JSON: **IE tag signatures** (comma-separated tag numbers; vendor IE `221` entries prefix up to eight body octets as `221:…` hex). **SSID (tag 0) is omitted** from the signature so explicit zero-length SSID and implicit no-SSID wildcards share the same tail—regenerate cluster JSON after this rule change. Per-cluster example MACs and counts split into `explicit_zero_length_ssid_ie` vs `implicit_wildcard_no_ssid_ie`.

Committed example output (regenerate after new captures): [`backend/data/flock_probe_ie_clusters.example.json`](../backend/data/flock_probe_ie_clusters.example.json).

Runtime WiFi Flock uses the same broadcast-wildcard rules as the script’s default (`try_probe_req_wildcard_from_mgmt_mpdu` in [`backend/src/ieee80211.rs`](../backend/src/ieee80211.rs)). Flock alert rows are written to **`*.deflockcsv`** with a WiGLE v1.6 column prefix plus DeFlock extensions — see [`DEFLOCKCSV.md`](DEFLOCKCSV.md). **Method IDs** in DeFlock CSV `detection_method_id`: **1** — wildcard probes + Flock OUI + gates; **2** — same gates + primary IE signature (see `probe_req_flock_ie_sig_for_clustering`) + Flock OUI; **3** — same gates + primary IE signature without OUI (for collision testing). A single probe can emit **multiple rows** if several methods match. `flock_disable_wifi_mask` bit `(method - 1)` disables that method.

`AppConfig` tuning: `flock_wifi_min_wildcards_in_window`, `flock_wifi_min_distinct_channels`, `flock_wifi_min_rssi_span`, `flock_wifi_per_src_cooldown_ms` (per method; **`0` = no cooldown**, useful for tests; can flood CSV), `flock_wifi_ie_sig_primary` (optional pipe `|` separated exact signatures; empty still allows both built-in primaries), `flock_wifi_ie_sig_alternates` (extra exact-match strings). Runtime matching uses `flock_ie_sig_allowlist_matches` in [`backend/src/flock_wifi.rs`](../backend/src/flock_wifi.rs): always `FLOCK_PROBE_IE_SIG_PRIMARY_DEFAULT` and `FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX`, plus config.

OUI-collision notes: [`docs/FLOCK_WIFI_FALSE_POSITIVE_CATALOG.md`](FLOCK_WIFI_FALSE_POSITIVE_CATALOG.md).
