//! Minimal PCAP-NG writer (Section Block, Interface Description Block, Enhanced Packet Block). Aims for wireshark compatibility.

use std::fs::File;
use std::io::Write;

use anyhow::Result;

const BT_SHB: u32 = 0x0a0d0d0a;
const BT_IDB: u32 = 0x0000_0001;
const BT_EPB: u32 = 0x0000_0006;

/// `LINKTYPE_IEEE802_11_RADIO` — radiotap + 802.11 (Linux capture default).
pub const LINKTYPE_IEEE802_11_RADIOTAP: u16 = 127;

fn pad_to_4(n: usize) -> usize {
    (4 - (n % 4)) % 4
}

fn write_block(f: &mut File, block_type: u32, body: &[u8]) -> Result<()> {
    let pad = pad_to_4(body.len());
    let total = (12 + body.len() + pad) as u32;
    f.write_all(&block_type.to_le_bytes())?;
    f.write_all(&total.to_le_bytes())?;
    f.write_all(body)?;
    for _ in 0..pad {
        f.write_all(&[0u8])?;
    }
    f.write_all(&total.to_le_bytes())?;
    Ok(())
}

pub fn write_shb(f: &mut File) -> Result<()> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&0x1a2b3c4du32.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&(-1i64).to_le_bytes());
    write_block(f, BT_SHB, &body)
}

pub fn write_idb(f: &mut File, linktype: u16, snaplen: u32, if_name: &str) -> Result<()> {
    let mut body = Vec::new();
    body.extend_from_slice(&linktype.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&snaplen.to_le_bytes());
    if !if_name.is_empty() {
        let opt: u16 = 2;
        body.extend_from_slice(&opt.to_le_bytes());
        let nl = if_name.len() as u16;
        body.extend_from_slice(&nl.to_le_bytes());
        body.extend_from_slice(if_name.as_bytes());
        let p = pad_to_4(body.len());
        for _ in 0..p {
            body.push(0);
        }
    }
    body.extend_from_slice(&0u16.to_le_bytes());
    write_block(f, BT_IDB, &body)
}

pub fn write_epb(f: &mut File, if_id: u32, ts_usec: u64, pkt: &[u8]) -> Result<()> {
    let mut body = Vec::with_capacity(32 + pkt.len() + 4);
    body.extend_from_slice(&if_id.to_le_bytes());
    let hi = (ts_usec >> 32) as u32;
    let lo = (ts_usec & 0xffff_ffff) as u32;
    body.extend_from_slice(&hi.to_le_bytes());
    body.extend_from_slice(&lo.to_le_bytes());
    let cap = pkt.len() as u32;
    body.extend_from_slice(&cap.to_le_bytes());
    body.extend_from_slice(&cap.to_le_bytes());
    body.extend_from_slice(pkt);
    let p = pad_to_4(pkt.len());
    for _ in 0..p {
        body.push(0);
    }
    write_block(f, BT_EPB, &body)
}
