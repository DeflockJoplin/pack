# OUI vendor data

The nearby-devices fingerprint uses a static OUI prefix table generated at build time from [`backend/data/oui_prefixes.txt`](../backend/data/oui_prefixes.txt), optionally merged with [`backend/data/oui_overrides.txt`](../backend/data/oui_overrides.txt) when that file exists (same line format; **overrides replace** the same OUI from the base file).

- **Format**: each non-comment line is `AA:BB:CC<TAB>Vendor name` (three-byte OUI in hex with colons).
- **Lookup**: at runtime, `wifi_oui_vendor` matches the **first three octets** of the MAC address to those keys.
- **Provenance**: the checked-in `oui_prefixes.txt` is generated from the [IEEE MA-L public listing](https://standards.ieee.org/products-programs/regauth/) (CSV at `https://standards-oui.ieee.org/oui/oui.csv`). Regenerate with:

  ```bash
  bash scripts/update-oui-from-ieee.sh
  ```

  The script writes a header with the generation date (UTC). Rebuild or run `cargo test -p pack` to refresh `oui_generated.rs` via `backend/build.rs`.
- **License**: PACK **source code** is under the MIT License (see repository root `LICENSE`). Vendor **names** in this table are IEEE “Organization Name” fields and are **not** MIT-licensed. The [IEEE Registration Authority](https://standards.ieee.org/products-programs/regauth/) terms govern the listing you download when running the update script; your use of regenerated data must comply with those terms. PACK is not affiliated with or endorsed by IEEE.
- **Size** (approximate, after a full MA-L import): ~39k prefix lines, ~1.2 MiB on disk (`wc -l` / `wc -c` on `backend/data/oui_prefixes.txt`). Exact counts change when IEEE updates the registry.

Flock infrastructure OUIs live separately in [`backend/src/flock_oui.rs`](../backend/src/flock_oui.rs); they are not part of this vendor table.
