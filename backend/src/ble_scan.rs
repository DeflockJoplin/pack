//! Passive BLE scan via BlueZ (`bluer`), feeding the unified ingest channel.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bluer::{Adapter, AdapterEvent, Address, DiscoveryFilter, DiscoveryTransport, Session};
use futures::StreamExt;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::ingest::{BleObservation, WardriveIngest};
use crate::ingest_pipeline;
use crate::state::AppState;

const XUNTONG_COMPANY_ID: u16 = 0x09C8;

fn pick_company_id(mfd: &HashMap<u16, Vec<u8>>) -> u16 {
    if mfd.contains_key(&XUNTONG_COMPANY_ID) {
        return XUNTONG_COMPANY_ID;
    }
    mfd.keys().copied().next().unwrap_or(0)
}

/// Runs forever: supervises one LE scanner task per selected HCI adapter.
pub async fn ble_scan_task(state: Arc<AppState>, tx: crossbeam_channel::Sender<WardriveIngest>) {
    let mut running: HashMap<String, JoinHandle<()>> = HashMap::new();
    loop {
        if state.wardriver_shutdown.load(Ordering::Acquire) {
            for (_, h) in running.drain() {
                h.abort();
            }
            break;
        }
        if state.recon_hold_ble.load(Ordering::Relaxed) {
            for (_, h) in running.drain() {
                h.abort();
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }

        let (enabled, desired): (bool, Vec<String>) = state
            .capture
            .read()
            .map(|c| (c.scan_ble, c.active_ble_adapters.clone()))
            .unwrap_or((false, vec![]));

        if !enabled || desired.is_empty() {
            for (_, h) in running.drain() {
                h.abort();
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }

        let desired_set: std::collections::HashSet<String> = desired.into_iter().collect();
        let stale: Vec<String> = running
            .keys()
            .filter(|k| !desired_set.contains(*k))
            .cloned()
            .collect();
        for name in stale {
            if let Some(h) = running.remove(&name) {
                h.abort();
            }
        }

        for adapter_name in desired_set {
            if running.contains_key(&adapter_name) {
                continue;
            }
            let st = state.clone();
            let tx_c = tx.clone();
            let name = adapter_name.clone();
            let h = tokio::spawn(async move {
                ble_scanner_loop(&st, &name, &tx_c).await;
            });
            running.insert(adapter_name, h);
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn ble_scanner_loop(
    state: &Arc<AppState>,
    adapter_name: &str,
    tx: &crossbeam_channel::Sender<WardriveIngest>,
) {
    loop {
        if state.wardriver_shutdown.load(Ordering::Acquire) {
            break;
        }
        if state.recon_hold_ble.load(Ordering::Relaxed) {
            break;
        }
        let still_wanted = state
            .capture
            .read()
            .map(|c| c.scan_ble && c.active_ble_adapters.iter().any(|a| a == adapter_name))
            .unwrap_or(false);
        if !still_wanted {
            break;
        }
        if let Err(e) = run_ble_scanner(state, adapter_name, tx).await {
            let msg = format!("{adapter_name}: {e:#}");
            warn!(target: "ble", "{msg}");
            if let Ok(mut w) = state.stats.ble_last_error.write() {
                *w = Some(msg);
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}

async fn run_ble_scanner(
    state: &Arc<AppState>,
    adapter_name: &str,
    tx: &crossbeam_channel::Sender<WardriveIngest>,
) -> anyhow::Result<()> {
    let session = Session::new().await?;
    let adapter = session.adapter(adapter_name)?;
    adapter.set_powered(true).await?;

    let filter = DiscoveryFilter {
        transport: DiscoveryTransport::Le,
        duplicate_data: true,
        ..Default::default()
    };
    adapter.set_discovery_filter(filter).await?;

    if let Ok(mut w) = state.stats.ble_last_error.write() {
        *w = None;
    }
    info!(target: "ble", "LE discovery on {adapter_name} (duplicate_data=true)");

    let mut events = adapter.discover_devices_with_changes().await?;
    while let Some(evt) = events.next().await {
        if state.wardriver_shutdown.load(Ordering::Acquire) {
            break;
        }
        if state.recon_hold_ble.load(Ordering::Relaxed) {
            break;
        }
        let still_wanted = state
            .capture
            .read()
            .map(|c| c.scan_ble && c.active_ble_adapters.iter().any(|a| a == adapter_name))
            .unwrap_or(false);
        if !still_wanted {
            break;
        }
        if let AdapterEvent::DeviceAdded(addr) = evt {
            match observation_for_device(adapter_name, &adapter, addr).await {
                Ok(Some(obs)) => {
                    state.stats.ble_adverts_rx.fetch_add(1, Ordering::Relaxed);
                    ingest_pipeline::send_raw_ble(&tx, &state, adapter_name, obs);
                }
                Ok(None) => {}
                Err(e) => warn!(target: "ble", "{adapter_name} device {addr}: {e:#}"),
            }
        }
    }
    Ok(())
}

fn bytes_to_hex_capped(v: &[u8], max_bytes: usize) -> String {
    let n = v.len().min(max_bytes);
    let mut s = String::with_capacity(n * 2);
    for b in &v[..n] {
        use std::fmt::Write as _;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

async fn observation_for_device(
    adapter_name: &str,
    adapter: &Adapter,
    addr: Address,
) -> anyhow::Result<Option<BleObservation>> {
    let dev = adapter.device(addr)?;
    let name = dev.alias().await.unwrap_or_default();
    let rssi = dev
        .rssi()
        .await?
        .map(|v| v.clamp(i16::from(i8::MIN), i16::from(i8::MAX)) as i8)
        .unwrap_or(-95);
    let mfd = dev.manufacturer_data().await?.unwrap_or_default();
    let company_id = pick_company_id(&mfd);
    let addr_type = dev.address_type().await.map(|t| t as u8).unwrap_or(0);
    let addr_b = addr.0;

    let appearance = dev.appearance().await.ok().flatten();
    let mut service_uuids: Vec<String> = dev
        .uuids()
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .map(|u| u.to_string())
        .collect();
    service_uuids.sort();
    service_uuids.dedup();

    let service_data = dev.service_data().await.ok().flatten().unwrap_or_default();
    let mut service_data_keys: Vec<String> = service_data
        .keys()
        .map(std::string::ToString::to_string)
        .collect();
    service_data_keys.sort();
    service_data_keys.dedup();

    let mfg_payload_hex = mfd
        .get(&company_id)
        .filter(|p| !p.is_empty())
        .map(|p| bytes_to_hex_capped(p, 16));

    let mut mfg_keys: Vec<u16> = mfd.keys().copied().collect();
    mfg_keys.sort_unstable();
    let mut mfg_other_sigs: Vec<String> = Vec::new();
    for k in mfg_keys {
        if k == company_id {
            continue;
        }
        if let Some(p) = mfd.get(&k) {
            if p.is_empty() {
                continue;
            }
            mfg_other_sigs.push(format!("0x{:04x}:{}", k, bytes_to_hex_capped(p, 8)));
            if mfg_other_sigs.len() >= 8 {
                break;
            }
        }
    }

    let tx_power = dev.tx_power().await.ok().flatten();
    let advertising_flags = dev
        .advertising_flags()
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    Ok(Some(BleObservation {
        adapter: adapter_name.to_string(),
        addr: addr_b,
        addr_type,
        rssi,
        name,
        company_id,
        appearance,
        service_uuids,
        service_data_keys,
        mfg_payload_hex,
        mfg_other_sigs,
        tx_power,
        advertising_flags,
    }))
}
