# Test captures (local only)

802.11 test PCAPs used for offline Flock IE clustering and hunt scripts are **not committed**. They live under `data/test/` (ignored by `.gitignore` via `/data/`). This is where pcaps were places for analysis while developing the new IE detection method.

## Privacy

Captures can contain sensitive data:

- **SSID** strings in beacons and directed probes (often home or business network names).
- **802.11 MAC addresses** that may identify devices or locations when combined with other data.
- **Filenames** — avoid street names, intersections, or neighborhood labels when saving new files.

Use neutral local names such as `fixture_drive_a.pcap`, `fixture_flock_linux_wildcard_01.pcapng`, `probe_0.pcap`, or `raw_3.pcap`.

## Regenerating example cluster JSON

Committed example output: [`backend/data/flock_probe_ie_clusters.example.json`](../backend/data/flock_probe_ie_clusters.example.json).

After analyzing local captures:

```bash
. tools/pcap-venv/bin/activate
pip install -r tools/requirements-pcap.txt
python scripts/flock_probe_ie_sig.py data/test/*.pcapng > /tmp/clusters.json
```

Before committing updated JSON:

- Set `inputs` to **neutral placeholder paths** (illustrative only; files are not in git).
- Replace `example_macs` with anonymized values (preserve flock infrastructure **OUI** prefix where documenting Flock hardware, e.g. `d0:39:57:00:00:01`).
- Do not commit real SSIDs, geographic filenames, or third-party network names in any tracked file.

## Automated tests without PCAPs

These do not read `data/test/`:

- `cargo test -p pack` (embedded synthetic / hex frames in `ieee80211.rs`, `flock_wifi.rs`)
- `python scripts/flock_probe_ie_sig.py --self-test`


