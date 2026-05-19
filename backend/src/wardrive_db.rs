//! Wardriving SQLite (`data/wardrive.sqlite`): devices, per-medium observations, GPS track, detections.
//! WiGLE CSV remains the upload path; this DB backs map queries and future analytics.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::json;

use crate::ieee80211::{WifiEapolLinkParsed, WifiMgmtLinkParsed};
use crate::map_data::WigleMapPoint;

/// STA → AP edge: association request seen (intent).
pub const REL_ASSOC_PENDING: &str = "assoc_pending";
/// STA → AP edge: association response status success.
pub const REL_ASSOCIATED: &str = "associated";
/// STA → AP edge: authentication frame observed (weak context).
pub const REL_AUTH_NEGOTIATION: &str = "auth_negotiation";
/// STA → AP edge: EAPOL Key message observed.
pub const REL_EAPOL_PSK: &str = "eapol_psk_observed";

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS device (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  first_seen_ms INTEGER NOT NULL,
  last_seen_ms INTEGER NOT NULL,
  notes TEXT
);

CREATE TABLE IF NOT EXISTS mac_address (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  mac BLOB NOT NULL UNIQUE,
  device_id INTEGER NOT NULL REFERENCES device(id),
  first_seen_ms INTEGER NOT NULL,
  last_seen_ms INTEGER NOT NULL,
  addr_type INTEGER,
  source TEXT
);

CREATE TABLE IF NOT EXISTS wifi_network (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ssid_display TEXT NOT NULL,
  ssid_raw BLOB,
  first_seen_ms INTEGER NOT NULL,
  last_seen_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_wifi_network_display ON wifi_network(ssid_display);

CREATE TABLE IF NOT EXISTS wifi_ap_observation (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  t_ms INTEGER NOT NULL,
  lat REAL NOT NULL,
  lon REAL NOT NULL,
  accuracy_m REAL,
  alt_m INTEGER,
  ap_device_id INTEGER NOT NULL REFERENCES device(id),
  network_id INTEGER REFERENCES wifi_network(id),
  channel INTEGER NOT NULL,
  rssi INTEGER NOT NULL,
  auth_mode TEXT
);

CREATE INDEX IF NOT EXISTS idx_wifi_ap_obs_t ON wifi_ap_observation(t_ms DESC);

CREATE TABLE IF NOT EXISTS wifi_probe_observation (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  t_ms INTEGER NOT NULL,
  lat REAL NOT NULL,
  lon REAL NOT NULL,
  accuracy_m REAL,
  alt_m INTEGER,
  sta_device_id INTEGER NOT NULL REFERENCES device(id),
  network_id INTEGER REFERENCES wifi_network(id),
  is_wildcard INTEGER NOT NULL,
  channel INTEGER NOT NULL,
  rssi INTEGER NOT NULL,
  ie_tag_seq TEXT,
  flock_ie_sig TEXT,
  linked_ap_device_id INTEGER REFERENCES device(id)
);

CREATE INDEX IF NOT EXISTS idx_wifi_probe_obs_t ON wifi_probe_observation(t_ms DESC);

CREATE TABLE IF NOT EXISTS ble_observation (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  t_ms INTEGER NOT NULL,
  lat REAL NOT NULL,
  lon REAL NOT NULL,
  accuracy_m REAL,
  alt_m INTEGER,
  device_id INTEGER NOT NULL REFERENCES device(id),
  rssi INTEGER NOT NULL,
  name TEXT NOT NULL,
  company_id INTEGER NOT NULL,
  extra_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_ble_obs_t ON ble_observation(t_ms DESC);

CREATE TABLE IF NOT EXISTS gps_track_point (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  t_ms INTEGER NOT NULL,
  lat REAL NOT NULL,
  lon REAL NOT NULL,
  accuracy_m REAL
);

CREATE INDEX IF NOT EXISTS idx_gps_track_t ON gps_track_point(t_ms DESC);

CREATE TABLE IF NOT EXISTS detection (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  t_ms INTEGER NOT NULL,
  family TEXT NOT NULL,
  method_id INTEGER NOT NULL,
  lat REAL,
  lon REAL,
  payload_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_detection_t ON detection(t_ms DESC);

CREATE TABLE IF NOT EXISTS detection_participant (
  detection_id INTEGER NOT NULL REFERENCES detection(id),
  device_id INTEGER NOT NULL REFERENCES device(id),
  role TEXT NOT NULL,
  PRIMARY KEY (detection_id, device_id, role)
);

CREATE TABLE IF NOT EXISTS device_relation (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  from_device_id INTEGER NOT NULL REFERENCES device(id),
  to_device_id INTEGER NOT NULL REFERENCES device(id),
  relation TEXT NOT NULL,
  confidence REAL NOT NULL,
  evidence_json TEXT,
  created_ms INTEGER NOT NULL
);
"#;

/// v2: `wifi_link_event` log + `device_relation.updated_ms` + unique edge index.
const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS wifi_link_event (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  t_ms INTEGER NOT NULL,
  iface TEXT NOT NULL,
  frame_kind TEXT NOT NULL,
  sta_mac BLOB NOT NULL,
  bssid_mac BLOB NOT NULL,
  status_code INTEGER,
  reason_code INTEGER,
  auth_alg INTEGER,
  channel INTEGER NOT NULL,
  rssi INTEGER NOT NULL,
  lat REAL NOT NULL,
  lon REAL NOT NULL,
  accuracy_m REAL,
  alt_m INTEGER
);

CREATE INDEX IF NOT EXISTS idx_wifi_link_event_t ON wifi_link_event(t_ms DESC);
CREATE INDEX IF NOT EXISTS idx_wifi_link_event_sta_bssid ON wifi_link_event(sta_mac, bssid_mac);
CREATE UNIQUE INDEX IF NOT EXISTS idx_device_relation_sta_ap_relation
  ON device_relation(from_device_id, to_device_id, relation);
"#;

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let ver: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if ver < 1 {
        conn.execute_batch(SCHEMA_V1)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    if ver < 2 {
        conn.execute_batch(SCHEMA_V2)?;
        let need_updated_ms: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('device_relation') WHERE name='updated_ms'",
            [],
            |r| r.get(0),
        )?;
        if need_updated_ms == 0 {
            conn.execute_batch(
                "ALTER TABLE device_relation ADD COLUMN updated_ms INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        conn.pragma_update(None, "user_version", 2)?;
    }
    if ver < 3 {
        migrate_wifi_probe_sta_nullable(conn)?;
        conn.pragma_update(None, "user_version", 3)?;
    }
    Ok(())
}

/// v3: probe observations may omit STA MAC (MAC-free probe CSV path).
fn migrate_wifi_probe_sta_nullable(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS wifi_probe_observation_new (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  t_ms INTEGER NOT NULL,
  lat REAL NOT NULL,
  lon REAL NOT NULL,
  accuracy_m REAL,
  alt_m INTEGER,
  sta_device_id INTEGER REFERENCES device(id),
  network_id INTEGER REFERENCES wifi_network(id),
  is_wildcard INTEGER NOT NULL,
  channel INTEGER NOT NULL,
  rssi INTEGER NOT NULL,
  ie_tag_seq TEXT,
  flock_ie_sig TEXT,
  linked_ap_device_id INTEGER REFERENCES device(id)
);
INSERT INTO wifi_probe_observation_new (
  id, t_ms, lat, lon, accuracy_m, alt_m, sta_device_id, network_id,
  is_wildcard, channel, rssi, ie_tag_seq, flock_ie_sig, linked_ap_device_id
)
SELECT
  id, t_ms, lat, lon, accuracy_m, alt_m, sta_device_id, network_id,
  is_wildcard, channel, rssi, ie_tag_seq, flock_ie_sig, linked_ap_device_id
FROM wifi_probe_observation;
DROP TABLE wifi_probe_observation;
ALTER TABLE wifi_probe_observation_new RENAME TO wifi_probe_observation;
CREATE INDEX IF NOT EXISTS idx_wifi_probe_obs_t ON wifi_probe_observation(t_ms DESC);
"#,
    )?;
    Ok(())
}

/// Read/write wardriving database (ingest + migrations).
pub struct WardriveStore {
    conn: std::sync::Mutex<Connection>,
}

impl WardriveStore {
    pub fn open(data_root: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_root)
            .with_context(|| format!("mkdir {}", data_root.display()))?;
        let path = data_root.join("wardrive.sqlite");
        let conn = Connection::open(&path)
            .with_context(|| format!("open wardrive sqlite {}", path.display()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        migrate(&conn).with_context(|| "wardrive schema migration")?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    fn ensure_device_for_mac(
        tx: &rusqlite::Transaction<'_>,
        mac: &[u8; 6],
        kind: &str,
        t_ms: i64,
    ) -> rusqlite::Result<i64> {
        let found: Option<i64> = tx
            .query_row(
                "SELECT device_id FROM mac_address WHERE mac = ?",
                params![mac.as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(dev_id) = found {
            tx.execute(
                "UPDATE device SET last_seen_ms = ?1 WHERE id = ?2",
                params![t_ms, dev_id],
            )?;
            tx.execute(
                "UPDATE mac_address SET last_seen_ms = ?1 WHERE mac = ?2",
                params![t_ms, mac.as_slice()],
            )?;
            return Ok(dev_id);
        }
        tx.execute(
            "INSERT INTO device (kind, first_seen_ms, last_seen_ms) VALUES (?1, ?2, ?2)",
            params![kind, t_ms],
        )?;
        let dev_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO mac_address (mac, device_id, first_seen_ms, last_seen_ms)
             VALUES (?1, ?2, ?3, ?3)",
            params![mac.as_slice(), dev_id, t_ms],
        )?;
        Ok(dev_id)
    }

    fn ensure_wifi_network(
        tx: &rusqlite::Transaction<'_>,
        ssid_display: &str,
        t_ms: i64,
    ) -> rusqlite::Result<Option<i64>> {
        let s = ssid_display.trim();
        if s.is_empty() {
            return Ok(None);
        }
        let found: Option<i64> = tx
            .query_row(
                "SELECT id FROM wifi_network WHERE ssid_display = ?",
                params![s],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = found {
            tx.execute(
                "UPDATE wifi_network SET last_seen_ms = ?1 WHERE id = ?2",
                params![t_ms, id],
            )?;
            return Ok(Some(id));
        }
        tx.execute(
            "INSERT INTO wifi_network (ssid_display, first_seen_ms, last_seen_ms)
             VALUES (?1, ?2, ?2)",
            params![s, t_ms],
        )?;
        Ok(Some(tx.last_insert_rowid()))
    }

    /// WiFi AP sighting (same coarse semantics as a WiGLE AP row).
    pub fn insert_wifi_ap_observation(
        &self,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        bssid: &[u8; 6],
        ssid: &str,
        channel: u8,
        rssi: i8,
        auth_mode: &str,
    ) -> Result<()> {
        let mut g = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("wardrive lock: {e}"))?;
        let tx = g.transaction()?;
        let ap_dev = Self::ensure_device_for_mac(&tx, bssid, "wifi_ap", t_ms)?;
        let net_id = Self::ensure_wifi_network(&tx, ssid, t_ms)?;
        tx.execute(
            "INSERT INTO wifi_ap_observation (
               t_ms, lat, lon, accuracy_m, alt_m, ap_device_id, network_id, channel, rssi, auth_mode
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                t_ms,
                lat,
                lon,
                accuracy_m,
                alt_m,
                ap_dev,
                net_id,
                i64::from(channel),
                i64::from(rssi),
                auth_mode
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// WiFi probe sighting (SSID-centric; no STA MAC stored).
    pub fn insert_wifi_probe_observation(
        &self,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        is_wildcard: bool,
        ssid_directed: Option<&str>,
        channel: u8,
        rssi: i8,
        ie_tag_seq: Option<&str>,
        flock_ie_sig: Option<&str>,
    ) -> Result<()> {
        let mut g = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("wardrive lock: {e}"))?;
        let tx = g.transaction()?;
        let net_id = if is_wildcard {
            None
        } else {
            match ssid_directed {
                Some(s) if !s.trim().is_empty() => Self::ensure_wifi_network(&tx, s, t_ms)?,
                _ => None,
            }
        };
        tx.execute(
            "INSERT INTO wifi_probe_observation (
               t_ms, lat, lon, accuracy_m, alt_m, sta_device_id, network_id,
               is_wildcard, channel, rssi, ie_tag_seq, flock_ie_sig, linked_ap_device_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, ?10, ?11, NULL)",
            params![
                t_ms,
                lat,
                lon,
                accuracy_m,
                alt_m,
                net_id,
                if is_wildcard { 1i64 } else { 0i64 },
                i64::from(channel),
                i64::from(rssi),
                ie_tag_seq,
                flock_ie_sig,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// BLE advertisement sighting.
    pub fn insert_ble_observation(
        &self,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        addr: &[u8; 6],
        rssi: i8,
        name: &str,
        company_id: u16,
        extra_json: Option<&str>,
    ) -> Result<()> {
        let mut g = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("wardrive lock: {e}"))?;
        let tx = g.transaction()?;
        let dev = Self::ensure_device_for_mac(&tx, addr, "ble", t_ms)?;
        tx.execute(
            "INSERT INTO ble_observation (
               t_ms, lat, lon, accuracy_m, alt_m, device_id, rssi, name, company_id, extra_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                t_ms,
                lat,
                lon,
                accuracy_m,
                alt_m,
                dev,
                i64::from(rssi),
                name,
                i64::from(company_id),
                extra_json
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_gps_track_point(
        &self,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
    ) -> Result<()> {
        let g = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("wardrive lock: {e}"))?;
        g.execute(
            "INSERT INTO gps_track_point (t_ms, lat, lon, accuracy_m) VALUES (?1, ?2, ?3, ?4)",
            params![t_ms, lat, lon, accuracy_m],
        )?;
        Ok(())
    }

    fn upsert_sta_ap_relation(
        tx: &rusqlite::Transaction<'_>,
        from_sta: i64,
        to_ap: i64,
        relation: &str,
        confidence: f64,
        evidence_json: &str,
        t_ms: i64,
    ) -> rusqlite::Result<()> {
        tx.execute(
            r#"
            INSERT INTO device_relation (from_device_id, to_device_id, relation, confidence, evidence_json, created_ms, updated_ms)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
            ON CONFLICT(from_device_id, to_device_id, relation) DO UPDATE SET
              confidence = MAX(device_relation.confidence, excluded.confidence),
              evidence_json = excluded.evidence_json,
              updated_ms = excluded.updated_ms
            "#,
            params![from_sta, to_ap, relation, confidence, evidence_json, t_ms],
        )?;
        Ok(())
    }

    fn downgrade_sta_ap_on_leave(
        tx: &rusqlite::Transaction<'_>,
        from_sta: i64,
        to_ap: i64,
        evidence_json: &str,
        t_ms: i64,
    ) -> rusqlite::Result<()> {
        tx.execute(
            r#"
            UPDATE device_relation SET
              confidence = MIN(device_relation.confidence, 0.12),
              evidence_json = ?3,
              updated_ms = ?4
            WHERE from_device_id = ?1 AND to_device_id = ?2
              AND relation IN ('associated', 'assoc_pending', 'eapol_psk_observed')
            "#,
            params![from_sta, to_ap, evidence_json, t_ms],
        )?;
        Ok(())
    }

    /// Log a parsed management link frame and refresh STA↔AP `device_relation` edges.
    pub fn record_wifi_mgmt_link(
        &self,
        iface: &str,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        channel: u8,
        rssi: i8,
        ev: &WifiMgmtLinkParsed,
    ) -> Result<()> {
        let mut g = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("wardrive lock: {e}"))?;
        let tx = g.transaction()?;
        let sta_dev = Self::ensure_device_for_mac(&tx, &ev.sta_mac, "wifi_sta", t_ms)?;
        let ap_dev = Self::ensure_device_for_mac(&tx, &ev.bssid_mac, "wifi_ap", t_ms)?;
        tx.execute(
            r#"INSERT INTO wifi_link_event (
              t_ms, iface, frame_kind, sta_mac, bssid_mac, status_code, reason_code, auth_alg,
              channel, rssi, lat, lon, accuracy_m, alt_m
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"#,
            params![
                t_ms,
                iface,
                ev.frame_kind,
                ev.sta_mac.as_slice(),
                ev.bssid_mac.as_slice(),
                ev.status_code.map(i64::from),
                ev.reason_code.map(i64::from),
                ev.auth_alg.map(i64::from),
                i64::from(channel),
                i64::from(rssi),
                lat,
                lon,
                accuracy_m,
                alt_m,
            ],
        )?;
        let event_id = tx.last_insert_rowid();
        let ev_json =
            json!({"wifi_link_event_id": event_id, "frame_kind": ev.frame_kind}).to_string();

        match ev.frame_kind {
            "assoc_req" | "reassoc_req" => {
                Self::upsert_sta_ap_relation(
                    &tx,
                    sta_dev,
                    ap_dev,
                    REL_ASSOC_PENDING,
                    0.45,
                    &ev_json,
                    t_ms,
                )?;
            }
            "assoc_resp" | "reassoc_resp" => {
                if ev.status_code == Some(0) {
                    Self::upsert_sta_ap_relation(
                        &tx,
                        sta_dev,
                        ap_dev,
                        REL_ASSOCIATED,
                        0.95,
                        &ev_json,
                        t_ms,
                    )?;
                }
            }
            "auth" => {
                Self::upsert_sta_ap_relation(
                    &tx,
                    sta_dev,
                    ap_dev,
                    REL_AUTH_NEGOTIATION,
                    0.28,
                    &ev_json,
                    t_ms,
                )?;
            }
            "deauth" | "disassoc" => {
                Self::downgrade_sta_ap_on_leave(&tx, sta_dev, ap_dev, &ev_json, t_ms)?;
            }
            _ => {}
        }
        tx.commit()?;
        Ok(())
    }

    /// Log EAPOL Key (type 3) and add / strengthen `eapol_psk_observed` edge.
    pub fn record_wifi_eapol_key(
        &self,
        iface: &str,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        channel: u8,
        rssi: i8,
        ev: &WifiEapolLinkParsed,
    ) -> Result<()> {
        let mut g = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("wardrive lock: {e}"))?;
        let tx = g.transaction()?;
        let sta_dev = Self::ensure_device_for_mac(&tx, &ev.sta_mac, "wifi_sta", t_ms)?;
        let ap_dev = Self::ensure_device_for_mac(&tx, &ev.bssid_mac, "wifi_ap", t_ms)?;
        tx.execute(
            r#"INSERT INTO wifi_link_event (
              t_ms, iface, frame_kind, sta_mac, bssid_mac, status_code, reason_code, auth_alg,
              channel, rssi, lat, lon, accuracy_m, alt_m
            ) VALUES (?1, ?2, 'eapol_key', ?3, ?4, NULL, NULL, NULL, ?5, ?6, ?7, ?8, ?9, ?10)"#,
            params![
                t_ms,
                iface,
                ev.sta_mac.as_slice(),
                ev.bssid_mac.as_slice(),
                i64::from(channel),
                i64::from(rssi),
                lat,
                lon,
                accuracy_m,
                alt_m,
            ],
        )?;
        let event_id = tx.last_insert_rowid();
        let ev_json =
            json!({"wifi_link_event_id": event_id, "frame_kind": "eapol_key"}).to_string();
        Self::upsert_sta_ap_relation(&tx, sta_dev, ap_dev, REL_EAPOL_PSK, 0.85, &ev_json, t_ms)?;
        tx.commit()?;
        Ok(())
    }

    /// Apply a batch of pending writes in one SQLite transaction.
    pub fn apply_pending_writes(
        &self,
        writes: &[crate::wardrive_batch::WardrivePendingWrite],
    ) -> Result<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let mut g = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("wardrive lock: {e}"))?;
        let tx = g.transaction()?;
        for w in writes {
            match w {
                crate::wardrive_batch::WardrivePendingWrite::WifiAp {
                    t_ms,
                    lat,
                    lon,
                    accuracy_m,
                    alt_m,
                    bssid,
                    ssid,
                    channel,
                    rssi,
                    auth_mode,
                } => {
                    let ap_dev = Self::ensure_device_for_mac(&tx, bssid, "wifi_ap", *t_ms)?;
                    let net_id = Self::ensure_wifi_network(&tx, ssid, *t_ms)?;
                    tx.execute(
                        "INSERT INTO wifi_ap_observation (
                           t_ms, lat, lon, accuracy_m, alt_m, ap_device_id, network_id, channel, rssi, auth_mode
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            t_ms,
                            lat,
                            lon,
                            accuracy_m,
                            alt_m,
                            ap_dev,
                            net_id,
                            i64::from(*channel),
                            i64::from(*rssi),
                            auth_mode
                        ],
                    )?;
                }
                crate::wardrive_batch::WardrivePendingWrite::WifiProbe {
                    t_ms,
                    lat,
                    lon,
                    accuracy_m,
                    alt_m,
                    is_wildcard,
                    ssid_directed,
                    channel,
                    rssi,
                    ie_tag_seq,
                    flock_ie_sig,
                } => {
                    let net_id = if *is_wildcard {
                        None
                    } else {
                        match ssid_directed.as_deref() {
                            Some(s) if !s.trim().is_empty() => {
                                Self::ensure_wifi_network(&tx, s, *t_ms)?
                            }
                            _ => None,
                        }
                    };
                    tx.execute(
                        "INSERT INTO wifi_probe_observation (
                           t_ms, lat, lon, accuracy_m, alt_m, sta_device_id, network_id,
                           is_wildcard, channel, rssi, ie_tag_seq, flock_ie_sig, linked_ap_device_id
                         ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, ?10, ?11, NULL)",
                        params![
                            t_ms,
                            lat,
                            lon,
                            accuracy_m,
                            alt_m,
                            net_id,
                            if *is_wildcard { 1i64 } else { 0i64 },
                            i64::from(*channel),
                            i64::from(*rssi),
                            ie_tag_seq,
                            flock_ie_sig,
                        ],
                    )?;
                }
                crate::wardrive_batch::WardrivePendingWrite::Ble {
                    t_ms,
                    lat,
                    lon,
                    accuracy_m,
                    alt_m,
                    addr,
                    rssi,
                    name,
                    company_id,
                    extra_json,
                } => {
                    let dev = Self::ensure_device_for_mac(&tx, addr, "ble", *t_ms)?;
                    tx.execute(
                        "INSERT INTO ble_observation (
                           t_ms, lat, lon, accuracy_m, alt_m, device_id, rssi, name, company_id, extra_json
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            t_ms,
                            lat,
                            lon,
                            accuracy_m,
                            alt_m,
                            dev,
                            i64::from(*rssi),
                            name,
                            i64::from(*company_id),
                            extra_json
                        ],
                    )?;
                }
                crate::wardrive_batch::WardrivePendingWrite::WifiMgmtLink {
                    iface,
                    t_ms,
                    lat,
                    lon,
                    accuracy_m,
                    alt_m,
                    channel,
                    rssi,
                    ev,
                } => {
                    Self::record_wifi_mgmt_link_tx(
                        &tx, iface, *t_ms, *lat, *lon, *accuracy_m, *alt_m, *channel, *rssi, ev,
                    )?;
                }
                crate::wardrive_batch::WardrivePendingWrite::WifiEapol {
                    iface,
                    t_ms,
                    lat,
                    lon,
                    accuracy_m,
                    alt_m,
                    channel,
                    rssi,
                    eap,
                } => {
                    Self::record_wifi_eapol_key_tx(
                        &tx, iface, *t_ms, *lat, *lon, *accuracy_m, *alt_m, *channel, *rssi, eap,
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn record_wifi_mgmt_link_tx(
        tx: &rusqlite::Transaction<'_>,
        iface: &str,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        channel: u8,
        rssi: i8,
        ev: &WifiMgmtLinkParsed,
    ) -> Result<()> {
        let sta_dev = Self::ensure_device_for_mac(tx, &ev.sta_mac, "wifi_sta", t_ms)?;
        let ap_dev = Self::ensure_device_for_mac(tx, &ev.bssid_mac, "wifi_ap", t_ms)?;
        tx.execute(
            r#"INSERT INTO wifi_link_event (
              t_ms, iface, frame_kind, sta_mac, bssid_mac, status_code, reason_code, auth_alg,
              channel, rssi, lat, lon, accuracy_m, alt_m
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"#,
            params![
                t_ms,
                iface,
                ev.frame_kind,
                ev.sta_mac.as_slice(),
                ev.bssid_mac.as_slice(),
                ev.status_code.map(i64::from),
                ev.reason_code.map(i64::from),
                ev.auth_alg.map(i64::from),
                i64::from(channel),
                i64::from(rssi),
                lat,
                lon,
                accuracy_m,
                alt_m,
            ],
        )?;
        let event_id = tx.last_insert_rowid();
        let ev_json = json!({"wifi_link_event_id": event_id, "frame_kind": ev.frame_kind}).to_string();
        match ev.frame_kind {
            "assoc_req" | "reassoc_req" => {
                Self::upsert_sta_ap_relation(
                    tx,
                    sta_dev,
                    ap_dev,
                    REL_ASSOC_PENDING,
                    0.45,
                    &ev_json,
                    t_ms,
                )?;
            }
            "assoc_resp" | "reassoc_resp" => {
                if ev.status_code == Some(0) {
                    Self::upsert_sta_ap_relation(
                        tx,
                        sta_dev,
                        ap_dev,
                        REL_ASSOCIATED,
                        0.95,
                        &ev_json,
                        t_ms,
                    )?;
                }
            }
            "auth" => {
                Self::upsert_sta_ap_relation(
                    tx,
                    sta_dev,
                    ap_dev,
                    REL_AUTH_NEGOTIATION,
                    0.28,
                    &ev_json,
                    t_ms,
                )?;
            }
            "deauth" | "disassoc" => {
                Self::downgrade_sta_ap_on_leave(tx, sta_dev, ap_dev, &ev_json, t_ms)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn record_wifi_eapol_key_tx(
        tx: &rusqlite::Transaction<'_>,
        iface: &str,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        channel: u8,
        rssi: i8,
        ev: &WifiEapolLinkParsed,
    ) -> Result<()> {
        let sta_dev = Self::ensure_device_for_mac(tx, &ev.sta_mac, "wifi_sta", t_ms)?;
        let ap_dev = Self::ensure_device_for_mac(tx, &ev.bssid_mac, "wifi_ap", t_ms)?;
        tx.execute(
            r#"INSERT INTO wifi_link_event (
              t_ms, iface, frame_kind, sta_mac, bssid_mac, status_code, reason_code, auth_alg,
              channel, rssi, lat, lon, accuracy_m, alt_m
            ) VALUES (?1, ?2, 'eapol_key', ?3, ?4, NULL, NULL, NULL, ?5, ?6, ?7, ?8, ?9, ?10)"#,
            params![
                t_ms,
                iface,
                ev.sta_mac.as_slice(),
                ev.bssid_mac.as_slice(),
                i64::from(channel),
                i64::from(rssi),
                lat,
                lon,
                accuracy_m,
                alt_m,
            ],
        )?;
        let event_id = tx.last_insert_rowid();
        let ev_json = json!({"wifi_link_event_id": event_id, "frame_kind": "eapol_key"}).to_string();
        Self::upsert_sta_ap_relation(tx, sta_dev, ap_dev, REL_EAPOL_PSK, 0.85, &ev_json, t_ms)?;
        Ok(())
    }
}

/// Map-layer WiGLE-shaped points from wardrive DB (newest first). Returns empty if DB missing or error.
pub fn map_points_from_wardrive(data_root: &Path, limit: usize) -> Result<Vec<WigleMapPoint>> {
    let path = data_root.join("wardrive.sqlite");
    if !path.exists() {
        return Ok(vec![]);
    }
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("wardrive read {}", path.display()))?;
    let lim = i64::try_from(limit.max(1)).unwrap_or(800);
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT lat, lon, ssid, row_type, rssi FROM (
          SELECT o.lat AS lat, o.lon AS lon,
            COALESCE((SELECT n.ssid_display FROM wifi_network n WHERE n.id = o.network_id), '') AS ssid,
            'WIFI' AS row_type, o.rssi AS rssi, o.t_ms AS t_ms
          FROM wifi_ap_observation o
          UNION ALL
          SELECT o.lat, o.lon,
            CASE WHEN o.is_wildcard != 0 THEN ''
              ELSE COALESCE((SELECT n.ssid_display FROM wifi_network n WHERE n.id = o.network_id), '')
            END,
            'WIFI-PROBE', o.rssi, o.t_ms
          FROM wifi_probe_observation o
          UNION ALL
          SELECT o.lat, o.lon, o.name, 'BLE', o.rssi, o.t_ms
          FROM ble_observation o
        )
        ORDER BY t_ms DESC
        LIMIT ?1
        "#,
    )?;
    let rows = stmt.query_map(params![lim], |r| {
        Ok(WigleMapPoint {
            lat: r.get(0)?,
            lon: r.get(1)?,
            ssid: r.get(2)?,
            row_type: r.get(3)?,
            rssi: r.get::<_, i64>(4)? as i8,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn wardrive_migrate_and_map_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "lw-wardrive-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let db = WardriveStore::open(&dir).expect("open");
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        db.insert_wifi_ap_observation(
            t,
            45.0,
            -93.0,
            Some(5.0),
            None,
            &[1, 2, 3, 4, 5, 6],
            "TestNet",
            6,
            -70,
            "[WPA2-PSK-CCMP][ESS]",
        )
        .expect("ap");
        let pts = map_points_from_wardrive(&dir, 10).expect("map");
        assert_eq!(pts.len(), 1);
        assert!((pts[0].lat - 45.0).abs() < 1e-9);
        assert_eq!(pts[0].ssid, "TestNet");
        assert_eq!(pts[0].row_type, "WIFI");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wardrive_wifi_link_assoc_creates_relation() {
        let dir = std::env::temp_dir().join(format!(
            "lw-wardrive-link-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let db = WardriveStore::open(&dir).expect("open");
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let ev_req = crate::ieee80211::WifiMgmtLinkParsed {
            frame_kind: "assoc_req",
            sta_mac: [0x02, 0, 0, 0, 0, 1],
            bssid_mac: [0x00, 0x11, 0x22, 0x33, 0x44, 0x01],
            status_code: None,
            reason_code: None,
            auth_alg: None,
        };
        db.record_wifi_mgmt_link("wlan0mon", t, 44.0, -92.0, Some(4.0), None, 6, -66, &ev_req)
            .expect("req");
        let ev_ok = crate::ieee80211::WifiMgmtLinkParsed {
            frame_kind: "assoc_resp",
            sta_mac: [0x02, 0, 0, 0, 0, 1],
            bssid_mac: [0x00, 0x11, 0x22, 0x33, 0x44, 0x01],
            status_code: Some(0),
            reason_code: None,
            auth_alg: None,
        };
        db.record_wifi_mgmt_link(
            "wlan0mon",
            t + 10,
            44.0,
            -92.0,
            Some(4.0),
            None,
            6,
            -65,
            &ev_ok,
        )
        .expect("resp");
        let path = dir.join("wardrive.sqlite");
        let conn = rusqlite::Connection::open(&path).expect("conn");
        let n_events: i64 = conn
            .query_row("SELECT COUNT(*) FROM wifi_link_event", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n_events, 2);
        let n_rel: i64 = conn
            .query_row("SELECT COUNT(*) FROM device_relation", [], |r| r.get(0))
            .unwrap();
        assert!(n_rel >= 2);
        let ver: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
