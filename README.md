# DISCLAIMER
This is a tool for legal wardriving and privacy auditing.  The intended use is education, observation, logging observations, and sharing those observations to raise awareness in the user's community.

By itself, this software is not capable of unlawful activity.  

The repo has a strong preference for passive observations. However, users should not use it in furtherance of unlawful activity. 
Any such use is not condoned by the maintainer(s).  Modification of the software for unlawful use is similarly not condoned. 
**Use this software only in accordance with your local laws and regulations.**

## Status

Many of the features are WIP or experimental. What is definitely working well today is: Wardriving, Flock Detection, Logging, Uploads, and Home Zone. All other features should be considered in progress and unfinished.  All field testing was done on Kali Linux.


# P.A.C.K. — Passive Acquisition & Capture Kit

**P.A.C.K.** is a **wardriving** and **electronic awareness** application for Linux: it captures **802.11 management frames** in monitor mode, **BLE advertisements** via BlueZ, reads **GPS** from `gpsd` (for WiGLE-style WiFi logging, surveillance auditing, and co-travel detection). It also serves a **web dashboard** (status, adapters, channel plan, offline map, nearby devices, uploads, recon PCAP jobs, and more). It writes **WiGLE CSV** and **DeFlock-compatible alert CSV** (`*.deflockcsv`) under a configurable data directory and can upload to WiGLE.net and wdgwars.pl.

Flock detection is powered by a new detection method developed for this repo (method #3 in the code).  It looks for wildcard probes and matches unique combinations of IE fields that Flock cameras advertise. Other detection methods are included for reference and testing.  Credit for those detection methods and research goes to the original developers of those methods.

This repository is a **Rust** backend (`pack`) plus a **React + Vite** frontend. A **React + Vite** dashboard lives under `frontend/`; build `frontend/dist/` before first run and [docs/FRONTEND_DEVELOPMENT.md](docs/FRONTEND_DEVELOPMENT.md)). To change the UI or use hot reload, see **[docs/FRONTEND_DEVELOPMENT.md](docs/FRONTEND_DEVELOPMENT.md)**.

**Platform:** Linux only.

---

## Table of contents

1. [What you need (overview)](#what-you-need-overview)  
2. [Quick start](#quick-start)  
3. [Install system packages from zero](#install-system-packages-from-zero)  
4. [Install Rust (from zero)](#install-rust-from-zero)  
5. [Clone this repository](#clone-this-repository)  
6. [Build the backend](#build-the-backend)  
7. [Run the project](#run-the-project)  
8. [Configuration and data layout](#configuration-and-data-layout)  
9. [Map basemap](#map-basemap)  
10. [WiFi, BLE, and GPS](#wifi-ble-and-gps)  
11. [Laptop / battery](#laptop--battery)  
12. [Environment variables](#environment-variables)  
13. [Useful commands](#useful-commands)  
14. [Project layout](#project-layout)  
15. [Further documentation](#further-documentation)  
16. [License](#license)

---

## What you need (overview)

| Component | Purpose |
|-----------|---------|
| **64-bit Linux** | Host OS (tested workflow is Debian/Ubuntu-style; other distros untested, but should work). |
| **C toolchain + CMake** | Compiling native dependencies (e.g. SQLite, crypto stacks pulled in by crates). |
| **`pkg-config`** | Helps the build find `libpcap`. |
| **`libpcap` development headers** | Required by the `pcap` crate for WiFi capture. |
| **Rust (`rustup`, stable)** | Build and run the daemon. |
| **`iw`** | Channel hopping for monitor interfaces (runtime). |
| **`gpsd`** (optional but typical) | GPS fixes for wardriving CSV and map. |
| **BlueZ / `bluetoothd`** (optional) | BLE scanning when `scan_ble` is enabled. |
| **sudo for WiFi capture** | Run **`pack` with sudo** (see [WiFi, BLE, and GPS](#wifi-ble-and-gps)). |

You do **not** need Docker or a cloud account for a basic local wardriving and alerting run. **Node.js 20+** is required to build the dashboard (`frontend/dist/`); see [docs/FRONTEND_DEVELOPMENT.md](docs/FRONTEND_DEVELOPMENT.md) for `npm run dev`. 

---

## Quick start

Assumes you already have [system packages](#install-system-packages-from-zero) and [Rust](#install-rust-from-zero). Place your adapters in monitor mode (airmon-ng or similar) and then run:

```bash
git clone https://github.com/DeflockJoplin/pack pack
cd pack
cargo build -p pack --release

cd frontend
npm install
npm ci
npm run build

cd ..
sudo ./target/release/pack
```

Open **http://127.0.0.1:8787/** — build the dashboard first (`cd frontend && npm ci && npm run build`) — see [PUBLIC_SOURCE.md](PUBLIC_SOURCE.md). For WiFi capture, create **monitor mode** interfaces and select them on the **Adapters** page (see [WiFi (monitor mode)](#wifi-monitor-mode)).

For a full install from zero, continue with the sections below. For UI development, see **[docs/FRONTEND_DEVELOPMENT.md](docs/FRONTEND_DEVELOPMENT.md)**.

---

## Install system packages from zero

The exact package names differ by distribution. Below is a **Debian / Ubuntu** baseline (run with `sudo` where appropriate).

### 1) Update package index

```bash
sudo apt update
```

### 2) Core build tools and libraries

```bash
sudo apt install -y \
  build-essential \
  pkg-config \
  cmake \
  git \
  curl \
  libpcap-dev \
  libssl-dev \
  iw \
  ca-certificates \
  gpsd \
  gpsd-clients \
  bluez
```
Enable/start Bluetooth if you use BLE:

```bash
sudo systemctl enable --now bluetooth
```

- **`build-essential`**: `gcc`, `g++`, `make`.  
- **`pkg-config`**: Locates `libpcap` for the linker.  
- **`cmake`**: Some transitive native builds expect it.  
- **`libpcap-dev`**: Headers and libraries for live capture (`pcap` crate).  
- **`libssl-dev`**: Often pulled in for TLS-related tooling; safe to install even if most Rust TLS here uses `rustls`.  
- **`iw`**: Used at runtime to set channels on WiFi interfaces.  
- **`git`**, **`curl`**: Clone repo and download installers.



## Install Rust (from zero)

### 1) Install `rustup` (official Rust toolchain manager)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
```

### 2) Load Cargo into your current shell

```bash
source "$HOME/.cargo/env"
```

### 3) Install the stable toolchain (default)

```bash
rustup default stable
rustup update
```

### 4) Verify

```bash
cargo --version
rustc --version
```

Add `source "$HOME/.cargo/env"` to your `~/.bashrc` or `~/.profile` so new terminals find `cargo`.

---

## Clone this repository

```bash
git clone <repository-url> LinuxWardriver
cd LinuxWardriver
```

Replace `<repository-url>` with the actual Git remote you use (HTTPS or SSH).

---

## Build the backend

The Rust crate lives under `backend/` and is a workspace member named **`pack`**.

From the **repository root** (where this `README.md` and the root `Cargo.toml` are):

```bash
cargo build -p pack --release
```

- **Debug (faster compile, slower binary):** omit `--release`:

  ```bash
  cargo build -p pack
  ```

- **Release binary path:** `target/release/pack`  
- **Debug binary path:** `target/debug/pack`

The build runs `backend/build.rs`, which reads `backend/data/oui_prefixes.txt` and generates OUI lookup code for the “nearby devices” fingerprinting feature. That file is included in the repo; no extra download is required.

Build **`frontend/dist/`** before running (see [PUBLIC_SOURCE.md](PUBLIC_SOURCE.md)). To rebuild the UI after changing `frontend/src/`, see **[docs/FRONTEND_DEVELOPMENT.md](docs/FRONTEND_DEVELOPMENT.md)**.

---

## Run the project

### 1) Create / choose a data directory

By default the app uses a directory named **`data`** under your **current working directory** when you start the process (unless overridden; see [Environment variables](#environment-variables)).

First run creates:

- `data/config/app.json` — settings (HTTP bind, interfaces, channel plan, uploads, …)  
- `data/wigle/`, `data/recon/`, etc. — as features write files  

Example:

```bash
mkdir -p data
cd /path/to/LinuxWardriver
# run from repo root so ./data is used, or set PACK_DATA
```

### 2) Start the daemon

From repository root (recommended), after a release build:

```bash
sudo ./target/release/pack
```

Or run via Cargo (debug build):

```bash
sudo cargo run -p pack
```

### 3) Open the dashboard

Default listen address is **`127.0.0.1:8787`** (see `DEFAULT_HTTP_ADDR` in `backend/src/config.rs`).

- **With built frontend:** open **http://127.0.0.1:8787/**  
- **API-only check:** `curl -s http://127.0.0.1:8787/api/stats | head`

**Boot order when `upload_on_boot` is enabled** (default in `data/config/app.json`):

1. The HTTP server starts immediately so the dashboard is reachable during startup.
2. If upload destinations are configured, the daemon probes connectivity to WiGLE/WDGwars hosts (~2 s cap). With no route, boot upload is skipped (`boot_upload_phase: skipped_offline`) and capture starts without hanging on timeouts.
3. Pending WiGLE CSVs under `data/wigle/pending/` upload on a background task; WiFi monitor setup and capture start **after** that task finishes (success, failure, or offline skip). BLE scanning starts with capture (ingest channel).
4. The layout banner and **Uploads** page show live boot upload status via `GET /api/stats` (`boot_upload_in_progress`, `capture_started`, session counters).

Set `upload_on_boot: false` in config (or on the Uploads page) to start monitor mode and capture immediately, without waiting for uploads.

### 4) Stop the daemon

Press **Ctrl+C** in the terminal where the daemon runs (foreground). Shutdown sets `wardriver_shutdown` immediately so background tasks and open **Nearby** SSE streams (`GET /api/nearby/stream`) close within about a second instead of blocking graceful HTTP teardown.

If Ctrl+C seems to do nothing, you can also use **Quit daemon** under **System** in the dashboard (calls `POST /api/shutdown`) or `curl -X POST http://127.0.0.1:8787/api/shutdown`.
Default HTTP bind is **127.0.0.1** only. If you override listen to `0.0.0.0`, any host on the LAN can stop the daemon via `/api/shutdown` — keep the default for untrusted networks.

For **hot-reload UI development** (`npm run dev`), see **[docs/FRONTEND_DEVELOPMENT.md](docs/FRONTEND_DEVELOPMENT.md)**.

---

## Configuration and data layout

| Path (under data root) | Role |
|------------------------|------|
| `config/app.json` | Main JSON config: `http_listen`, `data_root`, `capture_wifi`, `active_capture_interfaces`, monitor setup (`monitor_setup_on_startup`, `monitor_parent_interfaces`, `monitor_suffix`, `monitor_teardown_on_exit`), `channel_plan`, `gpsd_host`, `persist_gps_track_sqlite` (optional GPS rows in `wardrive.sqlite`; default off), `scan_ble`, `mbtiles_path`, `map_basemap` (`dark` \| `light`, proxy only), uploads, `home_geo` (Privacy page), `wigle_exclude_bssids` and `privacy_exclude_ssids` (Privacy), `flock_ignore_macs` (Detections), co-travel tuning, … |
| `wigle/pending/` | Pending WiGLE CSV sessions |
| `wigle/uploaded/` | Uploaded sessions (after successful uploads, per app rules) |
| `recon/wifiprobes/` | WiFi PCAP-NG + `*_channels.jsonl` sidecar |
| `recon/ble/` | BLE HCI PCAP-NG (`btmon`) |
| … | Other buckets created as features run |

You can edit `config/app.json` by hand or use **`GET /api/config`** and **`POST /api/config`** from the UI or `curl`. **`POST /api/files/databases/reset`** deletes all top-level `*.sqlite` files under the data directory (wardrive, co-travel, etc.); CSV and PCAP exports are not removed.

### Map basemap

Without **`mbtiles_path`**, raster tiles are proxied at `/api/map/tiles/{z}/{x}/{y}`. Set **`map_basemap`** in `config/app.json` to `"dark"` (default) or `"light"`. Dark uses [CARTO Dark Matter](https://carto.com/attributions/) via the local proxy; light uses OpenStreetMap. Attribution is returned on **`GET /api/map/layers`** as `tile_attribution`.

For fully offline maps, point **`mbtiles_path`** at a raster `.mbtiles` file (any style you export, including dark themes). User-supplied MBTiles are served as-is—the daemon does not recolor them.

The **Map** page (`/map`) filters layers server-side via **`GET /api/map/layers`** query parameters: `include_track`, `include_grid`, `include_wigle_wifi`, `include_wigle_probe`, `include_wigle_ble`, `include_flock`, `include_cotravel_fires`, `include_cotravel_suspects`, plus optional `flock_methods` (comma-separated IDs) and `flock_signal` (`wifi` \| `ble` \| `all`). The response includes `layer_counts` per layer. Filter state is reflected in the URL for bookmarks. The **Dashboard** does not embed the map (use Map when parked).

**Important:** If you set environment variable **`PACK_DATA`** to an absolute path, the daemon uses that directory as the data root (and still writes `config/app.json` there). This overrides a relative `data_root` in older configs in the way described in `backend/src/main.rs`.

---

## WiFi, BLE, and GPS

### WiFi (monitor mode)

In **this release**, PACK does **not** create, delete, or convert interfaces to monitor mode. Create monitor netdevs yourself, then select them in the app.

1. Put each interface in monitor mode before starting PACK (examples):

   ```bash
   sudo iw dev wlan0 set type monitor
   sudo ip link set wlan0 up
   # or: airmon-ng start wlan0  (etc.)
   ```

2. **Adapters** page — select interfaces that show **`monitor`** link type and **Save selection**.

3. **Channel plan** — assign each selected interface to 2.4 or 5 GHz so the hopper runs.

4. Run PACK with **sudo**:

   ```bash
   sudo ./target/release/pack
   ```

Legacy config fields (`monitor_setup_on_startup`, `monitor_parent_interfaces`, `PACK_MONITOR_*`) are **ignored** in this release. **`POST /api/monitor/setup`** returns **410 Gone**.

If capture fails: confirm monitor mode (`iw dev wlan0 info`), **NetworkManager** (`nmcli`), **`rfkill`**, and that **`iw`** is in PATH for channel hopping.

### BLE

- Requires **BlueZ** (`bluetoothd`) and a powered adapter. Select HCI adapters on the **Adapters** page (`active_ble_adapters` in config); nothing is auto-selected on a fresh install.  
- Set **`scan_ble`** in config to enable/disable BLE scanning.

### Recon (PCAP-NG jobs)

The **Recon** dashboard page runs on-demand capture jobs (one at a time):

| Job | Output | Notes |
|-----|--------|--------|
| Probe / raw WiFi | `data/recon/wifiprobes/*.pcapng` | Uses **active capture interfaces**; wardriving pcap pauses during the job. |
| BLE HCI | `data/recon/ble/*.pcapng` | Requires **`btmon`** in PATH and at least one **`active_ble_adapters`** entry (first is used); wardriving BLE scan pauses during the job. |

**WiFi channels:** By default, recon hops through your saved **channel plan** with a **200 ms** dwell (independent of wardriving `dwell_ms`). Each capture interface is assigned to **2.4 GHz or 5 GHz** on the **Channel plan** page; channels on that band are **split evenly** across adapters on the same band (each adapter maintains its own hop index). Wardriving uses the same per-band partition hopper. Optionally pass a comma-separated channel list in the UI (or `channels` in `POST /api/recon/start`): up to **one channel per adapter** can stay fixed without hopping; extra channels are visited on a synchronized hop schedule while earlier channels stay pinned per adapter.

**Channel visibility:** Poll **`GET /api/recon/status`** while a job runs (per-interface channel, hop mode, sequence). Each WiFi PCAP is accompanied by **`*_channels.jsonl`** (timestamped channel changes) for offline analysis. Radiotap may also include channel when the driver provides it.

### GPS

PACK connects to **[gpsd](https://gpsd.io/)** over **TCP** (JSON streaming). It does not talk to the serial port itself—you run `gpsd`, which owns the receiver and exposes fixes on port **2947** by default.

1. **Install packages** (see [Install system packages from zero](#install-system-packages-from-zero), step **3) Optional runtime services**): `gpsd` and `gpsd-clients` on Debian/Ubuntu (`cgps`, `gpspipe`, etc.).

2. **Find the receiver device** (common for USB GPS dongles):

   ```bash
   ls /dev/ttyUSB* /dev/ttyACM* 2>/dev/null
   dmesg | tail   # plug in the GPS and look for ttyUSB / ttyACM
   ```

   Your user usually needs membership in the **`dialout`** group (or run `gpsd` as root) so `gpsd` can read the serial device:

   ```bash
   sudo usermod -aG dialout "$USER"   # then log out and back in
   ```

3. **Run `gpsd` attached to that port.** Examples:

   - **Quick test (foreground):** stops any existing `gpsd` on the socket first if needed, then:

     ```bash
     sudo killall gpsd 2>/dev/null || true
     sudo gpsd -N -n /dev/ttyACM0
     ```

     `-N` keeps it in the foreground; `-n` does not wait for clients before polling the GPS.

   - **Typical install (systemd):** on Debian/Ubuntu, edit **`/etc/default/gpsd`** so `START_DAEMON="true"`, set `DEVICES="/dev/ttyACM0"` (or your device), and `GPSD_OPTIONS="-n"`. Then:

     ```bash
     sudo systemctl restart gpsd
     sudo systemctl status gpsd
     ```

     Ensure **`gpsd`** is listening on **`127.0.0.1:2947`** (default). The daemon’s **`gpsd_host`** must match that socket; default in config is **`127.0.0.1:2947`** (see `gpsd_host` in `config/app.json` or **`GET /api/config`** / **`POST /api/config`**).

4. **Verify a fix before relying on the wardriver:**

   ```bash
   cgps -s
   # or: gpspipe -w | head
   ```

   You want a **3D fix** (or at least 2D) and sensible lat/lon. The app treats fixes as usable when mode ≥ 2 and HDOP is within range (see `backend/src/gpsd.rs`).

5. **Behavior in PACK:** with no usable GPS fix, **WiGLE WiFi CSV rows are gated** (location/time quality), while BLE and other paths may still run. The dashboard shows GPS connection/fix status via **`/api/stats`**.

6. **WiGLE row throttle:** repeat rows for the same BSSID (or probe SSID in probe CSV) require **≥1 s** since the last logged row **and** at least **30 m** movement from that row’s position (`REVISIT_RADIUS_M` in `backend/src/geodedup.rs`). This does **not** apply to Flock/DeFlock alerts. Compare **`geo_dedup_allowed`** vs **`geo_dedup_suppressed`** on **`/api/stats`**.

### Regulatory domain

- Respect local laws for RF scanning, logging, and transmission. This software is a tool; **you** are responsible for compliant use.

---

## Laptop / battery

The capture daemon does most of the work; the web UI mainly polls HTTP (and **Nearby** can hold an SSE stream). On a laptop, reducing idle UI load helps battery life without changing capture behavior.

- **Headless wardriving:** run only the backend (`cargo run -p pack` or your release binary). No browser tabs means no dashboard/map/nearby polling. Use **`GET /api/stats`** or logs if you need status.
- **Nearby SSE:** the **Nearby** page with **SSE stream** enabled keeps a long-lived connection (~4 updates/s). Close that tab when you do not need a live device table.
- **BLE when unused:** set **`scan_ble`: `false`** in `data/config/app.json` (or **`POST /api/config`**) if you do not need BLE logging or BLE-based detections for this session.
- **USB WiFi adapters:** more dongles means more scanning capability, but more power usage.  
- **Background browser tabs:** Dashboard, Map, and Nearby (poll mode) slow their refresh to about **30s** while the tab is hidden (Page Visibility API), then return to normal when you focus the tab again.

---

## Environment variables

| Variable | Effect |
|----------|--------|
| **`PACK_DATA`** | Absolute path to the data directory (config, wigle, recon, …). Default: `./data` relative to the process working directory. |
| **`PACK_LISTEN`** | Overrides HTTP bind **address:port** for this process (see `backend/src/http/mod.rs`). Default comes from `http_listen` in config (typically `127.0.0.1:8787`). |
| **`PACK_MONITOR_*`** | **Ignored** in this release (no auto monitor setup). |
| **`PACK_UI`** | Absolute path to the directory that contains **`index.html`** (overrides automatic lookup beside the binary). |
| **`RUST_LOG`** | Standard `tracing` filter (e.g. `RUST_LOG=info`, `RUST_LOG=pack=debug`). |

---

## Useful commands

```bash
# One-shot bootstrap (apt on Debian/Ubuntu, release build; UI optional)
bash scripts/bootstrap.sh
bash scripts/bootstrap.sh --no-apt          # skip apt
bash scripts/bootstrap.sh --skip-frontend   # backend only (no Node)

# Optional Makefile wrappers
make deps           # system packages + cargo check
make build          # release binary (+ UI if dist missing or --force-frontend)
make build-backend  # release binary only
make run            # sudo ./target/release/pack

# Format + test Rust (from repo root)
cargo fmt -p pack
cargo test -p pack
```

Frontend build, dev server, and lint: **[docs/FRONTEND_DEVELOPMENT.md](docs/FRONTEND_DEVELOPMENT.md)**.

---

## Project layout

```
LinuxWardriver/
├── Cargo.toml              # Workspace manifest (members: backend)
├── README.md               # This file
├── backend/
│   ├── Cargo.toml          # Crate: pack
│   ├── build.rs            # Generates OUI table from data/oui_prefixes.txt
│   ├── data/
│   │   └── oui_prefixes.txt
│   └── src/                # Rust sources (HTTP, capture, BLE, GPS, …)
├── frontend/
│   ├── package.json
│   ├── vite.config.ts      # Dev server + /api proxy
│   ├── src/                # React pages and styles (source)
│   └── dist/               # Production bundle (`npm run build`; not shipped in public source)
├── scripts/
│   └── bootstrap.sh        # Distro deps + release build (+ optional UI)
└── docs/
    ├── PLAN.md             # Product / architecture plan
    ├── FRONTEND_DEVELOPMENT.md  # Node, npm build, npm run dev
    ├── OUI_DATA.md         # Notes on bundled OUI vendor data
    ├── FINGERPRINTING.md   # Curated IE rules, Flock clustering, licensing
    └── …                   # DEFLOCKCSV, FLOCK methods, etc.
```

Runtime **`data/`** is not committed; it appears next to your working directory (or under `PACK_DATA`) when you run the app.

---

## Further documentation

- **[docs/FRONTEND_DEVELOPMENT.md](docs/FRONTEND_DEVELOPMENT.md)** — Node.js install, rebuild `frontend/dist/`, `npm run dev`, lint.  
- **[docs/PLAN.md](docs/PLAN.md)** — Goals, architecture, WiGLE/DeFlock behavior, and parity notes vs the ESP32 wardriver.  
- **[docs/OUI_DATA.md](docs/OUI_DATA.md)** — OUI vendor table used for fingerprint hints on the Nearby page.  
- **[docs/FINGERPRINTING.md](docs/FINGERPRINTING.md)** — Curated `default.json` rules, Flock probe IE clustering, licensing.
- **[docs/TEST_CAPTURES.md](docs/TEST_CAPTURES.md)** — Local-only test PCAPs (`data/test/`), privacy notes, example JSON regeneration.
- **Privacy page** (`/privacy`) — Home geofence, optional GPS track in `wardrive.sqlite`, and MAC/SSID exclusions (full capture drop).
- **Detections page** (`/detections`) — Flock WiFi/BLE, SSID watch, probe CSV, and `flock_ignore_macs` for detector false positives.
- **Uploads page** (`/uploads`) — Set WiGLE and WDGwars API credentials, `upload_on_boot`, and upload toggles in the dashboard (stored in `data/config/app.json`). Boot-time upload progress appears in the global banner and on this page.

---

## License

PACK **software** is licensed under the [MIT License](LICENSE). Source code is also declared as `license = "MIT"` in [`backend/Cargo.toml`](backend/Cargo.toml).

Vendor **names** bundled in [`backend/data/oui_prefixes.txt`](backend/data/oui_prefixes.txt) (and embedded in release binaries via the build-time OUI table) are IEEE Registration Authority data, not MIT-licensed. See [`docs/OUI_DATA.md`](docs/OUI_DATA.md) and [`backend/NOTICE`](backend/NOTICE).
