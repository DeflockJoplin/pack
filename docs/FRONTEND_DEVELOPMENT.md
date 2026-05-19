# Frontend development

The dashboard is a **React + Vite** app under `frontend/`. The repository ships a **prebuilt production bundle** in [`frontend/dist/`](../frontend/dist/) so operators can run PACK without Node.js (see [README](../README.md)).

Use this document when you **change the UI**, run **`npm run dev`**, or need to **refresh the committed `dist/`** output.

---

## When you need Node.js

| Goal | Node required? |
|------|----------------|
| Run `pack` and use the dashboard from a clone | **No** (uses committed `frontend/dist/`) |
| Hot-reload UI development (`npm run dev`) | **Yes** |
| Rebuild `frontend/dist/` after source changes | **Yes** |
| Lint the frontend (`npm run lint`) | **Yes** |

You need **Node.js 20 or newer** and **npm** (the project uses Vite 8 and TypeScript 6).

---

## Install Node.js and npm

### Option A — NodeSource (Debian/Ubuntu)

Check [NodeSource distributions](https://github.com/nodesource/distributions) for the setup command for your release. Example for Node 22.x:

```bash
curl -fsSL https://deb.nodesource.com/setup_22.x | sudo -E bash -
sudo apt install -y nodejs
```

### Option B — `nvm` (user-local, many distros)

```bash
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
source "$HOME/.nvm/nvm.sh"
nvm install 22
nvm use 22
```

### Verify

```bash
node --version
npm --version
```

---

## Rebuild the production bundle

From the repository root:

```bash
cd frontend
npm ci
npm run build
```

- **`npm ci`**: installs dependencies from `package-lock.json` (use `npm install` only when intentionally updating the lockfile).
- **`npm run build`**: runs `tsc -b` then `vite build`, writing output to `frontend/dist/`.

### How the backend finds the UI

When `index.html` exists under a Vite `dist/` root, the Rust HTTP server serves it as static files ([`backend/src/frontend_dist.rs`](../backend/src/frontend_dist.rs), [`backend/src/http/mod.rs`](../backend/src/http/mod.rs)).

Resolution order:

1. Environment variable **`PACK_UI`** (absolute path to the directory that **contains** `index.html`)
2. Walk upward from the **executable** directory, checking `frontend/dist` then `dist`
3. Compile-time path `backend/../frontend/dist` when building from this repository

If nothing matches, the **API still works**; only the embedded dashboard is missing until you build or set `PACK_UI`.

---

## Local development with Vite

Run the backend and the Vite dev server in two terminals.

**Terminal A — backend** (from repo root):

```bash
sudo cargo run -p pack
```

Use **`sudo`** (see [README — WiFi (monitor mode)](../README.md#wifi-monitor-mode)).

**Terminal B — frontend** (first time: `npm ci` in `frontend/`):

```bash
cd frontend
npm ci
npm run dev
```

Vite proxies **`/api`** to **`http://127.0.0.1:8787`** ([`frontend/vite.config.ts`](../frontend/vite.config.ts)). Open the URL Vite prints (typically **http://localhost:5173**); same-origin `/api` calls reach the daemon.

For capture and monitor mode, see [README — Run the project](../README.md#run-the-project) and [WiFi, BLE, and GPS](../README.md#wifi-ble-and-gps).

### Routes and capture APIs

| Route | Page |
|-------|------|
| `/adapters` | WiFi/BLE selection, monitor setup on save (`POST /api/capture/select`), monitor health panel |
| `/channels` | Per-band channel plan (`GET`/`POST /api/channel-plan`) |
| `/monitor` | Redirects to `/adapters` |

**Channel plan POST body** includes `channels_2_4_in_use`, `channels_5_in_use`, `channels_5_dfs_in_use`, `adapters_2_4`, `adapters_5`, band flags, and `dwell_ms`. Effective JSON adds `channels_5_dfs_in_use` separately.

**Capture select** (`POST /api/capture/select`): `wifi_interfaces`, `active_ble_adapters`, `scan_ble` — validates operator-created monitor netdevs and updates `active_capture_interfaces` atomically.

---

## Lint

```bash
cd frontend && npm run lint
```

---

## Updating `frontend/dist/`

If you change files under `frontend/src/**` (or frontend dependencies that affect the bundle), **rebuild and commit** `frontend/dist/` in the same PR so clones stay in sync with the source.

```bash
cd frontend && npm ci && npm run build
git add frontend/dist
```

CI (when enabled) runs the same build and fails if `frontend/dist` drifts from `npm run build`.

---

## Useful commands (summary)

```bash
cd frontend && npm ci          # install deps
cd frontend && npm run build   # production bundle → frontend/dist/
cd frontend && npm run dev     # dev server + HMR
cd frontend && npm run lint    # ESLint
```

From the repo root, **`bash scripts/bootstrap.sh --force-frontend`** rebuilds the UI after a release backend build; **`--skip-frontend`** skips Node entirely (see `scripts/bootstrap.sh`).
