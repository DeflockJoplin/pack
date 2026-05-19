#!/usr/bin/env bash
# Download IEEE MA-L OUI registry and write backend/data/oui_prefixes.txt.
# Overrides: backend/data/oui_overrides.txt (merged at build time; see backend/build.rs).
#
# Usage (from repo root):
#   bash scripts/update-oui-from-ieee.sh
#
# Env:
#   IEEE_OUI_CSV_URL — default https://standards-oui.ieee.org/oui/oui.csv

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${REPO_ROOT}/backend/data/oui_prefixes.txt"
URL="${IEEE_OUI_CSV_URL:-https://standards-oui.ieee.org/oui/oui.csv}"

if ! command -v curl >/dev/null 2>&1; then
  echo "error: curl is required" >&2
  exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required" >&2
  exit 1
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

echo "Downloading ${URL} ..."
curl -fsSL "$URL" -o "$tmp"

IEEE_OUI_CSV_URL="$URL" python3 - "$tmp" "$OUT" <<'PY'
import csv
import os
import sys
from datetime import datetime, timezone

in_path, out_path = sys.argv[1], sys.argv[2]
source_url = os.environ.get("IEEE_OUI_CSV_URL", "")
today = datetime.now(timezone.utc).strftime("%Y-%m-%d")

entries: dict[str, str] = {}
with open(in_path, newline="", encoding="utf-8") as f:
    reader = csv.DictReader(f)
    for row in reader:
        if (row.get("Registry") or "").strip() != "MA-L":
            continue
        assign = (row.get("Assignment") or "").strip().upper()
        if len(assign) != 6:
            continue
        try:
            int(assign, 16)
        except ValueError:
            continue
        org = (row.get("Organization Name") or "").strip()
        if not org:
            continue
        org = org.replace("\t", " ").replace("\n", " ").replace("\r", " ")
        oui = f"{assign[0:2]}:{assign[2:4]}:{assign[4:6]}"
        entries[oui] = org

with open(out_path, "w", encoding="utf-8", newline="\n") as out:
    out.write("# IEEE MA-L OUI prefixes — generated {}\n".format(today))
    out.write("# Source: {}\n".format(source_url))
    out.write("# Format: AA:BB:CC<TAB>Organization Name (see docs/OUI_DATA.md)\n")
    for oui in sorted(entries):
        out.write("{}\t{}\n".format(oui, entries[oui]))

print("Wrote {} ({} prefixes)".format(out_path, len(entries)))
PY
