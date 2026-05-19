//! Beacon / probe response parsing (logic from ESP32 `src/ieee80211_parse.rs`).

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMini {
    Open,
    Wep,
    Wpa,
    Wpa2,
    WpaWpa2,
    Wpa2Ent,
    Wpa3,
    Wpa2Wpa3,
    Unknown,
}

#[derive(Clone, Debug, Default)]
pub struct RsnSuiteSummary {
    pub group_cipher: Option<String>,
    pub pairwise_ciphers: Vec<String>,
    /// Raw 802.11 AKM suite selector type bytes (`00:0f:ac` OUI only).
    pub akm_type_bytes: Vec<u8>,
    pub akm_labels: Vec<String>,
    pub mfp_capable: bool,
    pub mfp_required: bool,
}

#[derive(Clone, Debug)]
pub struct ApParsed {
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: i8,
    pub auth: AuthMini,
    pub ssid: String,
    pub wps_manufacturer: Option<String>,
    /// WPS model name attribute (`0x1023`).
    pub wps_model: Option<String>,
    pub wps_device_name: Option<String>,
    pub wps_model_number: Option<String>,
    /// First 8 octets of WPS UUID‑E as hex (privacy‑redacted).
    pub wps_uuid_e_partial: Option<String>,
    pub wps_device_password_id: Option<u16>,
    pub wps_serial_number: Option<String>,
    /// `OUI:payloadhex` for non-WPA/WPS vendor IEs (first bytes only).
    pub vendor_ie_sigs: Vec<String>,
    /// Short PHY summary from IE presence, e.g. `HT,VHT,HE`.
    pub phy_summary: String,
    /// RSN IE (and, when absent, WPA IE) cipher / AKM / MFP hints.
    pub rsn_group_cipher: Option<String>,
    pub rsn_pairwise_ciphers: Vec<String>,
    pub rsn_akm_suites: Vec<String>,
    pub rsn_mfp_capable: bool,
    pub rsn_mfp_required: bool,
    /// Interworking element: Access Network Options field (first octet).
    pub interworking_access: Option<u8>,
}

const WLAN_FC_TYPE_MASK: u16 = 0x000C;
const WLAN_FC_TYPE_MGMT: u16 = 0x0000;
const WLAN_FC_STYPE_MASK: u16 = 0x00F0;
const WLAN_FC_STYPE_ASSOC_REQ: u16 = 0x0000;
const WLAN_FC_STYPE_ASSOC_RESP: u16 = 0x0010;
const WLAN_FC_STYPE_REASSOC_REQ: u16 = 0x0020;
const WLAN_FC_STYPE_REASSOC_RESP: u16 = 0x0030;
const WLAN_FC_STYPE_BEACON: u16 = 0x0080;
const WLAN_FC_STYPE_PROBE_RESP: u16 = 0x0050;
const WLAN_FC_STYPE_PROBE_REQ: u16 = 0x0040;
const WLAN_FC_STYPE_DISASSOC: u16 = 0x00A0;
const WLAN_FC_STYPE_AUTH: u16 = 0x00B0;
const WLAN_FC_STYPE_DEAUTH: u16 = 0x00C0;

const WLAN_FC_TYPE_DATA: u16 = 0x0008;
/// QoS Data + QoS Null + QoS CF-Poll variants share subtype nibble `0x8` with type Data.
const WLAN_FC_STYPE_QOS_DATA: u16 = 0x0080;

/// After the 24-octet management MAC header, probe requests have capability (2) + listen interval (2).
const PROBE_REQ_BODY_OFFSET: usize = 28;

const WLAN_CAP_ESS: u16 = 0x0001;
const WLAN_CAP_IBSS: u16 = 0x0002;
const WLAN_CAP_PRIVACY: u16 = 0x0010;

const IE_SSID: u8 = 0;
const IE_DSPARAMS: u8 = 3;
const IE_RSN: u8 = 48;
const IE_VENDOR: u8 = 221;

const WPA_OUI: [u8; 4] = [0x00, 0x50, 0xf2, 0x01];
/// Wi‑Fi Alliance WPS element (vendor IE type 0x04).
const WPS_OUI_TYPE: [u8; 4] = [0x00, 0x50, 0xf2, 0x04];

const WPS_ATTR_DEVICE_NAME: u16 = 0x1011;
const WPS_ATTR_MANUFACTURER: u16 = 0x1021;
const WPS_ATTR_MODEL_NAME: u16 = 0x1023;
const WPS_ATTR_MODEL_NUMBER: u16 = 0x1024;
const WPS_ATTR_DEVICE_PASSWORD_ID: u16 = 0x1012;
/// UUID‑E (16 octets); only a short prefix is surfaced for privacy.
const WPS_ATTR_UUID_E: u16 = 0x1047;
const WPS_ATTR_SERIAL_NUMBER: u16 = 0x1042;

/// HT capabilities (802.11n).
const IE_HT_CAP: u8 = 45;
/// HT operation.
const IE_HT_OPERATION: u8 = 61;
/// VHT capabilities (802.11ac).
const IE_VHT_CAP: u8 = 191;
/// Interworking (802.11u) / Hotspot 2.0 advertisement (first octets only here).
const IE_INTERWORKING: u8 = 107;
/// Element ID Extension (802.11).
const IE_EXTENSION: u8 = 255;
/// Extension ID: HE PHY capabilities / HE operation (802.11ax).
const EXT_HE_CAPABILITIES: u8 = 35;
const EXT_HE_OPERATION: u8 = 36;

const RSN_AKM_8021X: u8 = 1;
const RSN_AKM_PSK: u8 = 2;
const RSN_AKM_SAE: u8 = 8;

/// Strips a trailing 4-octet FCS when the shorter buffer still parses as a full IE walk.
/// Tail information elements can be lost if the last four octets happen to complete a bogus parse; keep critical IEs earlier when synthesizing frames.
fn trim_fcs_if_needed(mpdu: &[u8]) -> &[u8] {
    if mpdu.len() > 4 {
        let without = &mpdu[..mpdu.len() - 4];
        if beacon_ies_parseable(without) {
            return without;
        }
    }
    mpdu
}

fn beacon_ies_parseable(mpdu: &[u8]) -> bool {
    if mpdu.len() < 36 {
        return false;
    }
    let mut i = 36usize;
    let buf = mpdu;
    while i + 2 <= buf.len() {
        let elen = buf[i + 1] as usize;
        if i + 2 + elen > buf.len() {
            return false;
        }
        i += 2 + elen;
    }
    true
}

fn cipher_suite_label(suite: &[u8; 4]) -> String {
    if suite[0..3] == [0x00, 0x0f, 0xac] {
        return match suite[3] {
            1 => "WEP40".into(),
            2 => "TKIP".into(),
            4 => "CCMP".into(),
            8 => "GCMP".into(),
            9 => "GCMP256".into(),
            _ => format!("cipher#{}", suite[3]),
        };
    }
    format!(
        "{:02x}-{:02x}-{:02x}:{}",
        suite[0], suite[1], suite[2], suite[3]
    )
}

fn akm_suite_label(suite: &[u8; 4]) -> String {
    if suite[0..3] == [0x00, 0x0f, 0xac] {
        return match suite[3] {
            1 => "8021X".into(),
            2 => "PSK".into(),
            3 => "FT_8021X".into(),
            4 => "FT_PSK".into(),
            5 => "8021X_SHA256".into(),
            8 => "SAE".into(),
            _ => format!("akm#{}", suite[3]),
        };
    }
    format!(
        "{:02x}-{:02x}-{:02x}:{}",
        suite[0], suite[1], suite[2], suite[3]
    )
}

fn akm_suite_type_byte(suite: &[u8; 4]) -> Option<u8> {
    if suite[0..3] == [0x00, 0x0f, 0xac] {
        Some(suite[3])
    } else {
        None
    }
}

/// Pairwise + AKM lists and optional MFP bits from the 2‑octet RSN Capabilities field.
fn parse_pairwise_akm_and_caps(
    buf: &[u8],
    mut o: usize,
) -> (Vec<String>, Vec<u8>, Vec<String>, bool, bool, usize) {
    let mut pairwise: Vec<String> = Vec::new();
    let mut akm_bytes: Vec<u8> = Vec::new();
    let mut akm_labels: Vec<String> = Vec::new();
    let mut mfp_cap = false;
    let mut mfp_req = false;
    if o + 2 > buf.len() {
        return (pairwise, akm_bytes, akm_labels, mfp_cap, mfp_req, o);
    }
    let n_ptk = u16::from_le_bytes([buf[o], buf[o + 1]]) as usize;
    o += 2;
    if o + n_ptk.saturating_mul(4) > buf.len() {
        return (pairwise, akm_bytes, akm_labels, mfp_cap, mfp_req, o);
    }
    for _ in 0..n_ptk {
        if o + 4 > buf.len() {
            break;
        }
        let suite = [buf[o], buf[o + 1], buf[o + 2], buf[o + 3]];
        o += 4;
        pairwise.push(cipher_suite_label(&suite));
    }
    if o + 2 > buf.len() {
        return (pairwise, akm_bytes, akm_labels, mfp_cap, mfp_req, o);
    }
    let n_akm = u16::from_le_bytes([buf[o], buf[o + 1]]) as usize;
    o += 2;
    if o + n_akm.saturating_mul(4) > buf.len() {
        return (pairwise, akm_bytes, akm_labels, mfp_cap, mfp_req, o);
    }
    for _ in 0..n_akm {
        if o + 4 > buf.len() {
            break;
        }
        let suite = [buf[o], buf[o + 1], buf[o + 2], buf[o + 3]];
        o += 4;
        if let Some(b) = akm_suite_type_byte(&suite) {
            akm_bytes.push(b);
        }
        akm_labels.push(akm_suite_label(&suite));
    }
    if o + 2 <= buf.len() {
        let caps = u16::from_le_bytes([buf[o], buf[o + 1]]);
        mfp_cap = (caps & 0x0040) != 0;
        mfp_req = (caps & 0x0080) != 0;
        o += 2;
    }
    (pairwise, akm_bytes, akm_labels, mfp_cap, mfp_req, o)
}

fn parse_rsn_ie(body: &[u8]) -> RsnSuiteSummary {
    let mut s = RsnSuiteSummary::default();
    if body.len() < 2 + 4 + 2 {
        return s;
    }
    let mut o = 2usize;
    let group = [body[o], body[o + 1], body[o + 2], body[o + 3]];
    o += 4;
    s.group_cipher = Some(cipher_suite_label(&group));
    let (ptk, akmb, akml, cap, req, no) = parse_pairwise_akm_and_caps(body, o);
    o = no;
    s.pairwise_ciphers = ptk;
    s.akm_type_bytes = akmb;
    s.akm_labels = akml;
    s.mfp_capable = cap;
    s.mfp_required = req;
    let _ = o;
    s
}

/// WPA IE (`00:50:F2:01`) — multicast + unicast + AKM suites (no RSN Capabilities in legacy WPA).
fn parse_wpa_ie(body: &[u8]) -> RsnSuiteSummary {
    let mut s = RsnSuiteSummary::default();
    if body.len() < 4 + 4 + 2 {
        return s;
    }
    if body[..4] != WPA_OUI {
        return s;
    }
    let mut o = 4usize;
    let group = [body[o], body[o + 1], body[o + 2], body[o + 3]];
    o += 4;
    s.group_cipher = Some(cipher_suite_label(&group));
    let (ptk, akmb, akml, cap, req, _) = parse_pairwise_akm_and_caps(body, o);
    s.pairwise_ciphers = ptk;
    s.akm_type_bytes = akmb;
    s.akm_labels = akml;
    s.mfp_capable = cap;
    s.mfp_required = req;
    s
}

/// Extract AP metadata from a captured management MPDU (beacon or probe response).
pub fn try_ap_from_mgmt_mpdu(mpdu: &[u8], rssi: i8, rx_channel: u8) -> Option<ApParsed> {
    let mpdu = trim_fcs_if_needed(mpdu);
    if mpdu.len() < 36 {
        return None;
    }
    let fc = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    if fc & WLAN_FC_TYPE_MASK != WLAN_FC_TYPE_MGMT {
        return None;
    }
    let st = fc & WLAN_FC_STYPE_MASK;
    if st != WLAN_FC_STYPE_BEACON && st != WLAN_FC_STYPE_PROBE_RESP {
        return None;
    }

    let cap = u16::from_le_bytes([mpdu[34], mpdu[35]]);
    if (cap & WLAN_CAP_IBSS) != 0 && (cap & WLAN_CAP_ESS) == 0 {
        return None;
    }

    let mut bssid = [0u8; 6];
    bssid.copy_from_slice(&mpdu[16..22]);

    let privacy = (cap & WLAN_CAP_PRIVACY) != 0;
    let mut ssid_buf = [0u8; 32];
    let mut ssid_len: usize = 0;
    let mut ch = rx_channel;
    let mut has_rsn = false;
    let mut has_wpa_ie = false;
    let mut rsn_sum = RsnSuiteSummary::default();
    let mut wpa_sum = RsnSuiteSummary::default();
    let mut wps = WpsFields::default();
    let mut vendor_ie_sigs: Vec<String> = Vec::new();
    const MAX_VENDOR_SIGS: usize = 24;
    let mut has_ht = false;
    let mut has_vht = false;
    let mut has_he = false;
    let mut interworking_access: Option<u8> = None;

    let mut i = 36usize;
    while i + 2 <= mpdu.len() {
        let id = mpdu[i];
        let elen = mpdu[i + 1] as usize;
        i += 2;
        if i + elen > mpdu.len() {
            break;
        }
        let body = &mpdu[i..i + elen];
        match id {
            IE_SSID => {
                let n = elen.min(32);
                ssid_buf[..n].copy_from_slice(&body[..n]);
                ssid_len = n;
            }
            IE_DSPARAMS => {
                if elen >= 1 {
                    ch = body[0];
                }
            }
            IE_RSN => {
                has_rsn = true;
                rsn_sum = parse_rsn_ie(body);
            }
            IE_HT_CAP | IE_HT_OPERATION => {
                has_ht = true;
            }
            IE_VHT_CAP => {
                has_vht = true;
            }
            IE_EXTENSION if elen >= 2 => {
                let ext = body[0];
                if ext == EXT_HE_CAPABILITIES || ext == EXT_HE_OPERATION {
                    has_he = true;
                }
            }
            IE_INTERWORKING if elen >= 1 => {
                interworking_access.get_or_insert(body[0]);
            }
            IE_VENDOR if elen >= 4 && body[..4] == WPA_OUI => {
                has_wpa_ie = true;
                wpa_sum = parse_wpa_ie(body);
            }
            IE_VENDOR if elen >= 4 && body.len() >= 4 && body[..4] == WPS_OUI_TYPE => {
                merge_wps_attributes(&body[4..], &mut wps);
            }
            IE_VENDOR if elen >= 4 && vendor_ie_sigs.len() < MAX_VENDOR_SIGS => {
                if body.len() >= 4 && body[..4] != WPA_OUI && body[..4] != WPS_OUI_TYPE {
                    if let Some(sig) = format_vendor_ie_sig(body) {
                        vendor_ie_sigs.push(sig);
                    }
                }
            }
            _ => {}
        }
        i += elen;
    }

    let mut akm = if has_rsn {
        rsn_sum.akm_type_bytes.clone()
    } else {
        Vec::new()
    };
    if akm.is_empty() {
        akm = wpa_sum.akm_type_bytes.clone();
    }

    let auth = classify_auth(privacy, has_rsn, has_wpa_ie, &akm);
    let ssid = ssid_bytes_to_string(&ssid_buf[..ssid_len], ssid_len);

    let rsn_group_cipher = if has_rsn {
        rsn_sum.group_cipher.clone()
    } else {
        None
    }
    .or_else(|| wpa_sum.group_cipher.clone());

    let rsn_pairwise_ciphers = if has_rsn && !rsn_sum.pairwise_ciphers.is_empty() {
        rsn_sum.pairwise_ciphers.clone()
    } else if !wpa_sum.pairwise_ciphers.is_empty() {
        wpa_sum.pairwise_ciphers.clone()
    } else {
        rsn_sum.pairwise_ciphers.clone()
    };

    let rsn_akm_suites = if has_rsn && !rsn_sum.akm_labels.is_empty() {
        rsn_sum.akm_labels.clone()
    } else if !wpa_sum.akm_labels.is_empty() {
        wpa_sum.akm_labels.clone()
    } else {
        rsn_sum.akm_labels.clone()
    };

    let (rsn_mfp_capable, rsn_mfp_required) = if has_rsn {
        (rsn_sum.mfp_capable, rsn_sum.mfp_required)
    } else {
        (wpa_sum.mfp_capable, wpa_sum.mfp_required)
    };

    let mut phy_parts: Vec<&'static str> = Vec::new();
    if has_ht {
        phy_parts.push("HT");
    }
    if has_vht {
        phy_parts.push("VHT");
    }
    if has_he {
        phy_parts.push("HE");
    }
    let phy_summary = phy_parts.join(",");

    Some(ApParsed {
        bssid,
        channel: ch,
        rssi,
        auth,
        ssid,
        wps_manufacturer: wps.manufacturer.filter(|s| !s.is_empty()),
        wps_model: wps.model_name.filter(|s| !s.is_empty()),
        wps_device_name: wps.device_name.filter(|s| !s.is_empty()),
        wps_model_number: wps.model_number.filter(|s| !s.is_empty()),
        wps_uuid_e_partial: wps.uuid_e_partial.filter(|s| !s.is_empty()),
        wps_device_password_id: wps.device_password_id,
        wps_serial_number: wps.serial_number.filter(|s| !s.is_empty()),
        vendor_ie_sigs,
        phy_summary,
        rsn_group_cipher,
        rsn_pairwise_ciphers,
        rsn_akm_suites,
        rsn_mfp_capable,
        rsn_mfp_required,
        interworking_access,
    })
}

#[derive(Default)]
struct WpsFields {
    manufacturer: Option<String>,
    model_name: Option<String>,
    device_name: Option<String>,
    model_number: Option<String>,
    uuid_e_partial: Option<String>,
    device_password_id: Option<u16>,
    serial_number: Option<String>,
}

fn merge_wps_attributes(data: &[u8], w: &mut WpsFields) {
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let typ = u16::from_be_bytes([data[i], data[i + 1]]);
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        i += 4;
        if i + len > data.len() {
            break;
        }
        let v = &data[i..i + len];
        match typ {
            WPS_ATTR_DEVICE_NAME if !v.is_empty() => {
                w.device_name.get_or_insert_with(|| wps_utf8_string(v));
            }
            WPS_ATTR_MANUFACTURER if !v.is_empty() => {
                w.manufacturer.get_or_insert_with(|| wps_utf8_string(v));
            }
            WPS_ATTR_MODEL_NAME if !v.is_empty() => {
                w.model_name.get_or_insert_with(|| wps_utf8_string(v));
            }
            WPS_ATTR_MODEL_NUMBER if !v.is_empty() => {
                w.model_number.get_or_insert_with(|| wps_utf8_string(v));
            }
            WPS_ATTR_UUID_E if v.len() == 16 => {
                w.uuid_e_partial.get_or_insert_with(|| {
                    v[..8].iter().fold(String::with_capacity(16), |mut s, b| {
                        use std::fmt::Write as _;
                        let _ = write!(&mut s, "{:02x}", b);
                        s
                    })
                });
            }
            WPS_ATTR_DEVICE_PASSWORD_ID if v.len() >= 2 => {
                w.device_password_id
                    .get_or_insert(u16::from_be_bytes([v[0], v[1]]));
            }
            WPS_ATTR_SERIAL_NUMBER if !v.is_empty() => {
                w.serial_number.get_or_insert_with(|| wps_utf8_string(v));
            }
            _ => {}
        }
        i += len;
        // WPS pads odd-length attribute data to a 2-octet boundary.
        if len % 2 == 1 {
            i = i.saturating_add(1);
        }
    }
}

/// First 3 bytes OUI + type byte as hex, colon, then up to 8 payload bytes hex (no colon in payload).
fn format_vendor_ie_sig(body: &[u8]) -> Option<String> {
    if body.len() < 4 {
        return None;
    }
    let oui = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}",
        body[0], body[1], body[2], body[3]
    );
    let take = (body.len() - 4).min(8);
    let pl = &body[4..4 + take];
    let mut hex = String::with_capacity(pl.len() * 2);
    for b in pl {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{:02x}", b);
    }
    Some(format!("{oui}{hex}"))
}

/// Comma-separated `id:len` for each IE after the management fixed header (offset 28 for probe, 36 for beacon).
fn mgmt_ie_tag_sequence(ies: &[u8]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i + 2 <= ies.len() {
        let id = ies[i];
        let elen = ies[i + 1] as usize;
        i += 2;
        if i + elen > ies.len() {
            break;
        }
        parts.push(format!("{id}:{elen}"));
        i += elen;
    }
    parts.join(",")
}

/// IE tag sequence for a probe request (for STA fingerprinting / curated rules).
pub fn probe_req_ie_tag_sequence(mpdu: &[u8]) -> Option<String> {
    let try_one = |buf: &[u8]| -> Option<String> {
        if buf.len() < 24 {
            return None;
        }
        let fc = u16::from_le_bytes([buf[0], buf[1]]);
        if fc & WLAN_FC_TYPE_MASK != WLAN_FC_TYPE_MGMT {
            return None;
        }
        let st = fc & WLAN_FC_STYPE_MASK;
        if st != WLAN_FC_STYPE_PROBE_REQ {
            return None;
        }
        if buf.len() < PROBE_REQ_BODY_OFFSET {
            return None;
        }
        Some(mgmt_ie_tag_sequence(&buf[PROBE_REQ_BODY_OFFSET..]))
    };
    try_one(mpdu).or_else(|| {
        if mpdu.len() > 4 {
            try_one(&mpdu[..mpdu.len() - 4])
        } else {
            None
        }
    })
}

/// Primary wildcard IE-chain signature from Flock/Lite-On test captures (see `scripts/flock_probe_ie_sig.py`).
pub const FLOCK_PROBE_IE_SIG_PRIMARY_DEFAULT: &str =
    "2,12,127,221:506f9a16030103,45,191,221:0050f208000000";

/// Alternate full signature seen on some Linux-captured Flock wildcard probes: ERP-first,
/// phantom `40 80` before Lite-On vendor IE, no separate FH / Extended IE tags.
pub const FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX: &str =
    "12,221:506f9a16030103,45,191,221:0050f208000000";

const FLOCK_PHANTOM_SKIP_CAP: u8 = 16;
const FLOCK_TLV_RESYNC_SCAN_MAX: usize = 64;
/// Lite-On vendor anchor in the canonical Flock wildcard probe IE signature.
const FLOCK_LITEON_IE_SIG_PREFIX: &str = "221:506f9a16030103";

fn flock_liteon_vendor_at(ies: &[u8], pos: usize) -> bool {
    pos + 9 <= ies.len()
        && ies[pos] == IE_VENDOR
        && ies[pos + 1] == 7
        && ies[pos + 2] == 0x50
        && ies[pos + 3] == 0x6f
        && ies[pos + 4] == 0x9a
}

fn flock_phantom_liteon_resync_ahead(ies: &[u8], pos: usize) -> bool {
    let end = (pos + 2)
        .saturating_add(32)
        .min(ies.len().saturating_sub(1));
    (pos + 2..end).any(|j| flock_liteon_vendor_at(ies, j))
}

fn flock_is_phantom_overflow(ies: &[u8], id: u8, elen: usize, i: usize) -> bool {
    if i + 2 + elen <= ies.len() {
        return false;
    }
    if elen > 200 {
        return true;
    }
    id == 64 && elen == 128 && flock_phantom_liteon_resync_ahead(ies, i)
}

fn flock_scan_tlv_resync(ies: &[u8], start: usize) -> Option<usize> {
    let end = (start + FLOCK_TLV_RESYNC_SCAN_MAX).min(ies.len().saturating_sub(1));
    (start..end).find(|&j| {
        let elen = ies[j + 1] as usize;
        elen <= 200 && j + 2 + elen <= ies.len()
    })
}

/// Normalize Linux mis-parse variants to the canonical Lite-On wildcard tail (`2,12,127,221:506f9a…`).
fn canonicalize_flock_probe_ie_sig(sig: &str) -> String {
    if sig.starts_with("2,12,127,") && sig.contains(FLOCK_LITEON_IE_SIG_PREFIX) {
        return sig.to_string();
    }
    if let Some(idx) = sig.find(FLOCK_LITEON_IE_SIG_PREFIX) {
        return format!("2,12,127,{}", &sig[idx..]);
    }
    sig.to_string()
}

/// Build the comma-separated IE signature used by `scripts/flock_probe_ie_sig.py` / `flock_probe_ie_clusters`.
/// Vendor IE `221` becomes `221:` + hex of the first `min(8, elen)` **body** octets (no separators).
///
/// **SSID (IE 0) is omitted** from the signature. Consecutive **SSID len 0** headers are coalesced (alignment).
/// **Phantom element**: if declared length overflows the buffer, `(id 64, len 128)` with a Lite-On vendor resync
/// ahead, or `len > 200`, skips 2 bytes and retries. Otherwise scan forward for the next valid TLV.
///
/// Returns `(signature, consumed_all_bytes)`; `None` if parsing fails irrecoverably.
fn flock_probe_ie_sig_from_ies_ex(ies: &[u8]) -> Option<(String, bool)> {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0usize;
    let mut phantom_skips: u8 = 0;
    while i + 2 <= ies.len() {
        let id = ies[i];
        let elen = ies[i + 1] as usize;
        if i + 2 + elen > ies.len() {
            if phantom_skips < FLOCK_PHANTOM_SKIP_CAP && flock_is_phantom_overflow(ies, id, elen, i)
            {
                phantom_skips += 1;
                i += 2;
                continue;
            }
            if let Some(j) = flock_scan_tlv_resync(ies, i) {
                if j > i {
                    i = j;
                    continue;
                }
            }
            return None;
        }
        i += 2;
        if id == IE_SSID {
            if elen == 0 {
                while i + 2 <= ies.len() && ies[i] == 0 && ies[i + 1] == 0 {
                    i += 2;
                }
            } else {
                i += elen;
            }
            continue;
        }
        let body = &ies[i..i + elen];
        if id == IE_VENDOR && elen >= 4 {
            let take = elen.min(8);
            let mut hex = String::with_capacity(take * 2);
            for b in &body[..take] {
                use std::fmt::Write as _;
                let _ = write!(&mut hex, "{:02x}", b);
            }
            parts.push(format!("221:{hex}"));
        } else {
            parts.push(format!("{id}"));
        }
        i += elen;
    }
    let complete = i == ies.len();
    Some((parts.join(","), complete))
}

fn pick_better_flock_parse(
    a: Option<(String, bool)>,
    b: Option<(String, bool)>,
) -> Option<(String, bool)> {
    match (a, b) {
        (Some((sa, ca)), Some((sb, cb))) => {
            if ca && !cb {
                Some((sa, ca))
            } else if !ca && cb {
                Some((sb, cb))
            } else if sa.len() >= sb.len() {
                Some((sa, ca))
            } else {
                Some((sb, cb))
            }
        }
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

fn try_probe_req_flock_ie_sig_inner_ex(mpdu: &[u8]) -> Option<(String, bool)> {
    if mpdu.len() < PROBE_REQ_BODY_OFFSET {
        return None;
    }
    let fc = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    if fc & WLAN_FC_TYPE_MASK != WLAN_FC_TYPE_MGMT {
        return None;
    }
    let st = fc & WLAN_FC_STYPE_MASK;
    if st != WLAN_FC_STYPE_PROBE_REQ {
        return None;
    }
    let mut best = flock_probe_ie_sig_from_ies_ex(&mpdu[PROBE_REQ_BODY_OFFSET..]);
    // Linux captures often place explicit SSID len-0 + supported-rates before the nominal IE offset.
    if mpdu.len() >= 28 && mpdu[24] == 0 && mpdu[25] == 0 {
        best = pick_better_flock_parse(best, flock_probe_ie_sig_from_ies_ex(&mpdu[26..]));
    }
    best
}

fn better_flock_sig(a: Option<(String, bool)>, b: Option<(String, bool)>) -> Option<String> {
    match (a, b) {
        (Some((sa, ca)), Some((sb, cb))) => {
            if ca && !cb {
                Some(sa)
            } else if !ca && cb {
                Some(sb)
            } else if sa.len() >= sb.len() {
                Some(sa)
            } else {
                Some(sb)
            }
        }
        (Some((s, _)), None) => Some(s),
        (None, Some((s, _))) => Some(s),
        (None, None) => None,
    }
}

/// Probe-request IE signature for Flock clustering (Python `flock_probe_ie_sig.py` format).
/// Evaluates full MPDU and MPDU without last 4 octets (possible FCS); prefers a **complete** parse, else longer sig.
#[must_use]
pub fn probe_req_flock_ie_sig_for_clustering(mpdu: &[u8]) -> Option<String> {
    let full = try_probe_req_flock_ie_sig_inner_ex(mpdu);
    let stripped = if mpdu.len() > 4 {
        try_probe_req_flock_ie_sig_inner_ex(&mpdu[..mpdu.len() - 4])
    } else {
        None
    };
    better_flock_sig(full, stripped).map(|s| canonicalize_flock_probe_ie_sig(&s))
}

fn wps_utf8_string(v: &[u8]) -> String {
    String::from_utf8_lossy(v).trim().to_string()
}

fn classify_auth(privacy: bool, has_rsn: bool, has_wpa_ie: bool, akm: &[u8]) -> AuthMini {
    let has_psk = akm.contains(&RSN_AKM_PSK);
    let has_sae = akm.contains(&RSN_AKM_SAE);
    let has_1x = akm.contains(&RSN_AKM_8021X);

    if has_rsn {
        if has_1x && !has_psk && !has_sae {
            return AuthMini::Wpa2Ent;
        }
        if has_psk && has_sae {
            return AuthMini::Wpa2Wpa3;
        }
        if has_sae && !has_psk {
            return AuthMini::Wpa3;
        }
        if has_psk {
            if has_wpa_ie {
                return AuthMini::WpaWpa2;
            }
            return AuthMini::Wpa2;
        }
        if privacy {
            return AuthMini::Wpa2;
        }
        return AuthMini::Open;
    }

    if has_wpa_ie {
        return AuthMini::Wpa;
    }

    if privacy {
        return AuthMini::Wep;
    }
    AuthMini::Open
}

fn ssid_bytes_to_string(ssid: &[u8], declared_len: usize) -> String {
    // 802.11 SSID IE is at most 32 octets.
    const MAX_SSID: usize = 32;
    let n = declared_len.min(ssid.len()).min(MAX_SSID);
    let slice = &ssid[..n];
    let slice = slice.split(|&b| b == 0).next().unwrap_or(&[]);
    if slice.is_empty() {
        return String::new();
    }
    if let Ok(s) = std::str::from_utf8(slice) {
        if !s.chars().any(char::is_control) {
            return s.to_string();
        }
    }
    let mut out = String::with_capacity(slice.len().saturating_mul(2).saturating_add(4));
    out.push_str("hex:");
    for &b in slice {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
        out.push(char::from_digit(u32::from(b & 0xf), 16).unwrap());
    }
    out
}

/// Source address and broadcast/directed classification for a **probe request**.
///
/// **Broadcast wildcard** is either:
/// - an **SSID IE (id 0) with length 0** (explicit; no SSID body octets), or
/// - **no SSID IE** after a full parse of all information elements (implicit; some stacks omit the tag
///   while still broadcasting; analyzers often label this as wildcard).
///
/// **Directed** probes carry an SSID IE with length greater than zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeReqWildcard {
    pub sa: [u8; 6],
    pub is_wildcard_ssid: bool,
    /// Decoded SSID when `is_wildcard_ssid` is false (directed probe).
    pub directed_ssid: Option<String>,
}

fn try_probe_req_wildcard_inner(mpdu: &[u8]) -> Option<ProbeReqWildcard> {
    if mpdu.len() < PROBE_REQ_BODY_OFFSET {
        return None;
    }
    let fc = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    if fc & WLAN_FC_TYPE_MASK != WLAN_FC_TYPE_MGMT {
        return None;
    }
    let st = fc & WLAN_FC_STYPE_MASK;
    if st != WLAN_FC_STYPE_PROBE_REQ {
        return None;
    }

    let mut sa = [0u8; 6];
    sa.copy_from_slice(&mpdu[10..16]);

    let mut i = PROBE_REQ_BODY_OFFSET;
    let mut saw_ssid = false;
    let mut wildcard = false;
    let mut directed_ssid: Option<String> = None;
    let mut truncated = false;
    while i + 2 <= mpdu.len() {
        let id = mpdu[i];
        let elen = mpdu[i + 1] as usize;
        i += 2;
        if i + elen > mpdu.len() {
            truncated = true;
            break;
        }
        if id == IE_SSID {
            saw_ssid = true;
            wildcard = elen == 0;
            if !wildcard {
                let n = elen.min(32);
                let body = &mpdu[i..i + n];
                let s = ssid_bytes_to_string(body, n);
                directed_ssid = if s.is_empty() { None } else { Some(s) };
            }
            break;
        }
        i += elen;
    }
    if saw_ssid {
        return Some(ProbeReqWildcard {
            sa,
            is_wildcard_ssid: wildcard,
            directed_ssid,
        });
    }
    if truncated {
        return None;
    }
    // Full IE walk: no SSID element — treat as broadcast wildcard (implicit).
    Some(ProbeReqWildcard {
        sa,
        is_wildcard_ssid: true,
        directed_ssid: None,
    })
}

/// Parse a probe request for broadcast wildcard (explicit zero-length SSID IE or implicit no-SSID IE)
/// vs directed (non-empty SSID IE).
/// `is_wildcard_ssid` is true for explicit **length 0** SSID IE or for **no SSID IE** after a complete parse.
/// Tries again without the last 4 bytes when present (common 802.11 FCS trailer in captures).
pub fn try_probe_req_wildcard_from_mgmt_mpdu(mpdu: &[u8]) -> Option<ProbeReqWildcard> {
    try_probe_req_wildcard_inner(mpdu).or_else(|| {
        if mpdu.len() > 4 {
            try_probe_req_wildcard_inner(&mpdu[..mpdu.len() - 4])
        } else {
            None
        }
    })
}

pub fn authmini_to_caps(a: AuthMini) -> String {
    let base = match a {
        AuthMini::Open => "[OPEN]",
        AuthMini::Wep => "[WEP]",
        AuthMini::Wpa => "[WPA-PSK]",
        AuthMini::Wpa2 => "[WPA2-PSK-CCMP]",
        AuthMini::WpaWpa2 => "[WPA-PSK][WPA2-PSK-CCMP]",
        AuthMini::Wpa2Ent => "[WPA2-EAP-CCMP]",
        AuthMini::Wpa3 => "[WPA3-SAE]",
        AuthMini::Wpa2Wpa3 => "[WPA2-PSK-CCMP][WPA3-SAE]",
        AuthMini::Unknown => "[UNKNOWN]",
    };
    format!("{base}[ESS]")
}

/// Parsed 802.11 management frame for STA↔BSSID link evidence (infra-oriented).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiMgmtLinkParsed {
    /// Wire format token, e.g. `assoc_req`, `assoc_resp`, `eapol_key`.
    pub frame_kind: &'static str,
    pub sta_mac: [u8; 6],
    pub bssid_mac: [u8; 6],
    pub status_code: Option<u16>,
    pub reason_code: Option<u16>,
    pub auth_alg: Option<u16>,
}

fn mac_addrs24(mpdu: &[u8]) -> Option<([u8; 6], [u8; 6], [u8; 6])> {
    if mpdu.len() < 24 {
        return None;
    }
    let mut a1 = [0u8; 6];
    let mut a2 = [0u8; 6];
    let mut a3 = [0u8; 6];
    a1.copy_from_slice(&mpdu[4..10]);
    a2.copy_from_slice(&mpdu[10..16]);
    a3.copy_from_slice(&mpdu[16..22]);
    Some((a1, a2, a3))
}

/// Infra **Association Request**: Addr1 = BSSID, Addr2 = STA, Addr3 = BSSID.
fn sta_bssid_assoc_auth_req(mpdu: &[u8]) -> Option<([u8; 6], [u8; 6])> {
    let (a1, a2, a3) = mac_addrs24(mpdu)?;
    if a1 == a3 && a1 != [0u8; 6] {
        Some((a2, a1))
    } else {
        None
    }
}

/// Infra **Association / Reassociation Response**: Addr1 = STA, Addr2 = BSSID, Addr3 = BSSID.
fn sta_bssid_assoc_resp(mpdu: &[u8]) -> Option<([u8; 6], [u8; 6])> {
    let (a1, a2, a3) = mac_addrs24(mpdu)?;
    if a2 == a3 && a1 != a2 {
        Some((a1, a2))
    } else {
        None
    }
}

/// **Reassociation Request**: Addr1 = current AP, Addr2 = STA, Addr3 = new AP (target BSSID).
fn sta_bssid_reassoc_req(mpdu: &[u8]) -> Option<([u8; 6], [u8; 6])> {
    let (_a1, a2, a3) = mac_addrs24(mpdu)?;
    if a3 != [0u8; 6] {
        Some((a2, a3))
    } else {
        None
    }
}

/// **Deauthentication / Disassociation** (common infra layouts): Addr3 = BSSID, peer in Addr1 or Addr2.
fn sta_bssid_deauth_disassoc(mpdu: &[u8]) -> Option<([u8; 6], [u8; 6])> {
    let (a1, a2, a3) = mac_addrs24(mpdu)?;
    if a2 == a3 && a1 != a2 {
        return Some((a1, a2));
    }
    if a1 == a3 && a1 != a2 {
        return Some((a2, a1));
    }
    None
}

/// Fixed field helpers after 24-octet MAC header.
fn u16_le(mpdu: &[u8], off: usize) -> Option<u16> {
    if off + 2 > mpdu.len() {
        return None;
    }
    Some(u16::from_le_bytes([mpdu[off], mpdu[off + 1]]))
}

/// Parse Auth / Assoc / Reassoc / Deauth / Disassoc for wardrive link evidence.
/// Returns `None` for non-mgmt or unsupported subtypes (including beacon/probe-resp).
pub fn try_wifi_mgmt_link_from_mpdu(mpdu: &[u8]) -> Option<WifiMgmtLinkParsed> {
    let mpdu = trim_fcs_if_needed(mpdu);
    if mpdu.len() < 26 {
        return None;
    }
    let fc = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    if fc & WLAN_FC_TYPE_MASK != WLAN_FC_TYPE_MGMT {
        return None;
    }
    let st = fc & WLAN_FC_STYPE_MASK;

    match st {
        WLAN_FC_STYPE_ASSOC_REQ => {
            let (sta, bssid) = sta_bssid_assoc_auth_req(mpdu)?;
            Some(WifiMgmtLinkParsed {
                frame_kind: "assoc_req",
                sta_mac: sta,
                bssid_mac: bssid,
                status_code: None,
                reason_code: None,
                auth_alg: None,
            })
        }
        WLAN_FC_STYPE_ASSOC_RESP => {
            let (sta, bssid) = sta_bssid_assoc_resp(mpdu)?;
            let status = u16_le(mpdu, 24)?;
            Some(WifiMgmtLinkParsed {
                frame_kind: "assoc_resp",
                sta_mac: sta,
                bssid_mac: bssid,
                status_code: Some(status),
                reason_code: None,
                auth_alg: None,
            })
        }
        WLAN_FC_STYPE_REASSOC_REQ => {
            let (sta, bssid) = sta_bssid_reassoc_req(mpdu)?;
            Some(WifiMgmtLinkParsed {
                frame_kind: "reassoc_req",
                sta_mac: sta,
                bssid_mac: bssid,
                status_code: None,
                reason_code: None,
                auth_alg: None,
            })
        }
        WLAN_FC_STYPE_REASSOC_RESP => {
            let (sta, bssid) = sta_bssid_assoc_resp(mpdu)?;
            let status = u16_le(mpdu, 24)?;
            Some(WifiMgmtLinkParsed {
                frame_kind: "reassoc_resp",
                sta_mac: sta,
                bssid_mac: bssid,
                status_code: Some(status),
                reason_code: None,
                auth_alg: None,
            })
        }
        WLAN_FC_STYPE_AUTH => {
            let (sta, bssid) = sta_bssid_assoc_auth_req(mpdu)?;
            let auth_alg = u16_le(mpdu, 24)?;
            Some(WifiMgmtLinkParsed {
                frame_kind: "auth",
                sta_mac: sta,
                bssid_mac: bssid,
                status_code: u16_le(mpdu, 28),
                reason_code: None,
                auth_alg: Some(auth_alg),
            })
        }
        WLAN_FC_STYPE_DEAUTH => {
            let (sta, bssid) = sta_bssid_deauth_disassoc(mpdu)?;
            let reason = u16_le(mpdu, 24)?;
            Some(WifiMgmtLinkParsed {
                frame_kind: "deauth",
                sta_mac: sta,
                bssid_mac: bssid,
                status_code: None,
                reason_code: Some(reason),
                auth_alg: None,
            })
        }
        WLAN_FC_STYPE_DISASSOC => {
            let (sta, bssid) = sta_bssid_deauth_disassoc(mpdu)?;
            let reason = u16_le(mpdu, 24)?;
            Some(WifiMgmtLinkParsed {
                frame_kind: "disassoc",
                sta_mac: sta,
                bssid_mac: bssid,
                status_code: None,
                reason_code: Some(reason),
                auth_alg: None,
            })
        }
        _ => None,
    }
}

/// RFC 1042 SNAP + EtherType `0x888e` (EAPOL), EAPOL **Key** message (type 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiEapolLinkParsed {
    pub sta_mac: [u8; 6],
    pub bssid_mac: [u8; 6],
}

fn ieee80211_data_header_len(fc: u16) -> Option<usize> {
    if (fc & WLAN_FC_TYPE_MASK) != WLAN_FC_TYPE_DATA {
        return None;
    }
    let to_ds = (fc >> 8) & 1 != 0;
    let from_ds = (fc >> 9) & 1 != 0;
    let qos = (fc & WLAN_FC_STYPE_MASK) == WLAN_FC_STYPE_QOS_DATA;
    let mut len = 24usize;
    if qos {
        len += 2;
    }
    match (to_ds, from_ds) {
        (false, false) => Some(len),
        (false, true) | (true, false) => Some(len),
        (true, true) => Some(len + 6),
    }
}

/// SNAP `AA:AA:03` + type `0x888E` + EAPOL type **3** (Key).
pub fn try_wifi_eapol_key_from_mpdu(mpdu: &[u8]) -> Option<WifiEapolLinkParsed> {
    let mpdu = trim_fcs_if_needed(mpdu);
    if mpdu.len() < 30 {
        return None;
    }
    let fc = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    let hdr_len = ieee80211_data_header_len(fc)?;
    if mpdu.len() < hdr_len + 8 {
        return None;
    }
    let snap = &mpdu[hdr_len..hdr_len + 8];
    if snap[0] != 0xaa || snap[1] != 0xaa || snap[2] != 0x03 {
        return None;
    }
    let etype = u16::from_be_bytes([snap[6], snap[7]]);
    if etype != 0x888e {
        return None;
    }
    let eapol = mpdu.get(hdr_len + 8..)?;
    if eapol.len() < 2 {
        return None;
    }
    if eapol[1] != 3 {
        return None;
    }
    let to_ds = (fc >> 8) & 1 != 0;
    let from_ds = (fc >> 9) & 1 != 0;
    let (a1, a2, _a3) = mac_addrs24(mpdu)?;
    let (sta, bssid) = match (to_ds, from_ds) {
        (true, false) => (a2, a1),
        (false, true) => (a1, a2),
        _ => return None,
    };
    if sta == [0u8; 6] || bssid == [0u8; 6] {
        return None;
    }
    Some(WifiEapolLinkParsed {
        sta_mac: sta,
        bssid_mac: bssid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal beacon: management header + fixed fields + SSID + vendor WPS IE.
    #[test]
    fn wps_parse_in_beacon() {
        let mut f = Vec::new();
        f.extend_from_slice(&[0x80, 0x00]); // beacon
        f.extend_from_slice(&[0x00, 0x00]); // duration
        f.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // DA broadcast
        f.extend_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]); // SA = BSSID
        f.extend_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]); // BSSID (addr3)
        f.extend_from_slice(&[0x00, 0x00]); // seq
        f.extend_from_slice(&[0u8; 8]); // timestamp
        f.extend_from_slice(&100u16.to_le_bytes()); // beacon interval
        f.extend_from_slice(&[0x01, 0x00]); // cap ESS
        f.extend_from_slice(&[0x00, 0x01, b'x']); // SSID "x"
        let mut wps = Vec::new();
        wps.extend_from_slice(&WPS_OUI_TYPE);
        wps.extend_from_slice(&0x1021u16.to_be_bytes());
        wps.extend_from_slice(&4u16.to_be_bytes());
        wps.extend_from_slice(b"Acme");
        wps.extend_from_slice(&0x1023u16.to_be_bytes());
        wps.extend_from_slice(&2u16.to_be_bytes());
        wps.extend_from_slice(b"M1");
        wps.extend_from_slice(&0x1011u16.to_be_bytes());
        wps.extend_from_slice(&3u16.to_be_bytes());
        wps.extend_from_slice(b"Cam");
        wps.push(0u8); // WPS pads odd-length attribute data to 2-octet boundary
        wps.extend_from_slice(&0x1024u16.to_be_bytes());
        wps.extend_from_slice(&2u16.to_be_bytes());
        wps.extend_from_slice(b"N7");
        wps.extend_from_slice(&0x1047u16.to_be_bytes());
        wps.extend_from_slice(&16u16.to_be_bytes());
        wps.extend_from_slice(&[0x33u8; 16]);
        wps.extend_from_slice(&0x1012u16.to_be_bytes());
        wps.extend_from_slice(&2u16.to_be_bytes());
        wps.extend_from_slice(&0x0000u16.to_be_bytes());
        wps.extend_from_slice(&0x1042u16.to_be_bytes());
        wps.extend_from_slice(&4u16.to_be_bytes());
        wps.extend_from_slice(b"S999");
        f.push(IE_VENDOR);
        f.push(wps.len() as u8);
        f.extend_from_slice(&wps);
        // Vendor IE (non-WPA/WPS): Ralink/MediaTek-style OUI + type + 2-byte payload prefix.
        let ven = vec![0x00, 0x0c, 0x43, 0x01, 0xab, 0xcd];
        f.push(IE_VENDOR);
        f.push(ven.len() as u8);
        f.extend_from_slice(&ven);
        // HT capabilities (26-octet body).
        f.push(IE_HT_CAP);
        f.push(26);
        f.extend_from_slice(&[0u8; 26]);

        let ap = try_ap_from_mgmt_mpdu(&f, -42, 6).expect("beacon parses");
        assert_eq!(ap.wps_manufacturer.as_deref(), Some("Acme"));
        assert_eq!(ap.wps_model.as_deref(), Some("M1"));
        assert_eq!(ap.wps_device_name.as_deref(), Some("Cam"));
        assert_eq!(ap.wps_model_number.as_deref(), Some("N7"));
        assert_eq!(ap.wps_uuid_e_partial.as_deref(), Some("3333333333333333"));
        assert_eq!(ap.wps_device_password_id, Some(0));
        assert_eq!(ap.wps_serial_number.as_deref(), Some("S999"));
        assert_eq!(ap.phy_summary, "HT");
        assert!(
            ap.vendor_ie_sigs
                .iter()
                .any(|s| s.starts_with("00:0c:43:01abcd")),
            "{:?}",
            ap.vendor_ie_sigs
        );
    }

    #[test]
    fn beacon_rsn_mfp_he_interworking() {
        let mut f = Vec::new();
        f.extend_from_slice(&[0x80, 0x00]);
        f.extend_from_slice(&[0x00, 0x00]);
        f.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        f.extend_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        f.extend_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        f.extend_from_slice(&[0x00, 0x00]);
        f.extend_from_slice(&[0u8; 8]);
        f.extend_from_slice(&100u16.to_le_bytes());
        f.extend_from_slice(&[0x01, 0x11]); // ESS + privacy
        f.extend_from_slice(&[0x00, 0x01, b'z']);
        // Interworking before RSN so a hypothetical FCS trim cannot drop it from the tail.
        f.push(IE_INTERWORKING);
        f.push(1);
        f.push(0x0f);
        let mut rsn = Vec::new();
        rsn.extend_from_slice(&1u16.to_le_bytes());
        rsn.extend_from_slice(&[0x00, 0x0f, 0xac, 0x04]);
        rsn.extend_from_slice(&1u16.to_le_bytes());
        rsn.extend_from_slice(&[0x00, 0x0f, 0xac, 0x04]);
        rsn.extend_from_slice(&1u16.to_le_bytes());
        rsn.extend_from_slice(&[0x00, 0x0f, 0xac, 0x02]);
        rsn.extend_from_slice(&0x00c0u16.to_le_bytes());
        f.push(IE_RSN);
        f.push(rsn.len() as u8);
        f.extend_from_slice(&rsn);
        let mut he = vec![EXT_HE_CAPABILITIES];
        he.resize(22, 0);
        f.push(IE_EXTENSION);
        f.push(he.len() as u8);
        f.extend_from_slice(&he);

        let ap = try_ap_from_mgmt_mpdu(&f, -50, 1).expect("beacon");
        assert_eq!(ap.rsn_group_cipher.as_deref(), Some("CCMP"));
        assert_eq!(ap.rsn_pairwise_ciphers, vec!["CCMP".to_string()]);
        assert_eq!(ap.rsn_akm_suites, vec!["PSK".to_string()]);
        assert!(ap.rsn_mfp_capable && ap.rsn_mfp_required);
        assert_eq!(ap.interworking_access, Some(0x0f));
        assert_eq!(ap.phy_summary, "HE");
        assert_eq!(ap.auth, AuthMini::Wpa2);
    }

    #[test]
    fn probe_directed_ssid_binary_hex() {
        let mut p = vec![
            0x40, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        // SSID IE: invalid UTF-8 — expect stable hex label.
        p.extend_from_slice(&[0x00, 0x03, 0xff, 0xfe, 0xfd]);
        let pr = try_probe_req_wildcard_from_mgmt_mpdu(&p).expect("probe");
        assert!(!pr.is_wildcard_ssid);
        assert_eq!(pr.directed_ssid.as_deref(), Some("hex:fffefd"));
    }

    #[test]
    fn probe_directed_ssid_len_clamped_to_32() {
        let mut p = vec![
            0x40, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        // Bogus length 40 but only 40 'a' bytes follow — parser only uses first 32 for SSID string.
        p.push(0x00);
        p.push(40);
        p.extend(std::iter::repeat(b'a').take(40));
        let pr = try_probe_req_wildcard_from_mgmt_mpdu(&p).expect("probe");
        assert!(!pr.is_wildcard_ssid);
        let want = "a".repeat(32);
        assert_eq!(pr.directed_ssid.as_deref(), Some(want.as_str()));
    }

    /// Synthetic probe matching `backend/data/fingerprints/default.json` example IE chain.
    #[test]
    fn probe_ie_tag_sequence_curated_placeholder() {
        let mut p = vec![
            0x40, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        p.extend_from_slice(&[0x00, 0x00]); // SSID len 0
        p.extend_from_slice(&[0x01, 0x08, 0, 0, 0, 0, 0, 0, 0, 0]);
        p.push(IE_HT_CAP);
        p.push(26);
        p.extend_from_slice(&[0u8; 26]);
        let seq = probe_req_ie_tag_sequence(&p).expect("seq");
        assert_eq!(seq, "0:0,1:8,45:26");
    }

    #[test]
    fn probe_directed_ssid() {
        let mut p = vec![
            0x40, 0x00, // probe req
            0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
            0xff, // SA
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Capability + listen interval (probe fixed fields before IEs).
            0x00, 0x00, 0x00, 0x00,
        ];
        p.extend_from_slice(&[0x00, 0x07, b'h', b'o', b'm', b'e', b'n', b'e', b't']);
        let pr = try_probe_req_wildcard_from_mgmt_mpdu(&p).expect("probe");
        assert!(!pr.is_wildcard_ssid);
        assert_eq!(pr.directed_ssid.as_deref(), Some("homenet"));
        assert_eq!(pr.sa, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    }

    #[test]
    fn probe_wildcard_ssid_zero_length() {
        let mut p = vec![
            0x40, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, // cap + listen
        ];
        // SSID IE: id 0, length 0 (wildcard broadcast on the air).
        p.extend_from_slice(&[0x00, 0x00]);
        let pr = try_probe_req_wildcard_from_mgmt_mpdu(&p).expect("wildcard probe");
        assert!(pr.is_wildcard_ssid);
        assert_eq!(pr.directed_ssid, None);
    }

    #[test]
    fn probe_implicit_wildcard_no_ssid_ie() {
        let mut p = vec![
            0x40, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        // Supported rates IE only (no SSID) — implicit broadcast wildcard.
        p.extend_from_slice(&[0x01, 0x02, 0x02, 0x04]);
        let pr = try_probe_req_wildcard_from_mgmt_mpdu(&p).expect("implicit wildcard");
        assert!(pr.is_wildcard_ssid);
        assert_eq!(pr.directed_ssid, None);
    }

    #[test]
    fn probe_truncated_does_not_yield_implicit_wildcard() {
        let mut p = vec![
            0x40, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        // Bogus IE: length runs past end of MPDU — must not infer implicit wildcard.
        p.push(0xff);
        p.push(250);
        assert!(try_probe_req_wildcard_from_mgmt_mpdu(&p).is_none());
    }

    #[test]
    fn probe_flock_ie_sig_linux_coalesce_phantom() {
        fn hex_bytes(s: &str) -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        }
        let mpdu = hex_bytes(
            "40000000ffffffffffffd0395786d0b9ffffffffffff8017000001080c1218243048606c\
             dd070050f208001f007f09040000000000004080dd07506f9a160301032d1aef011fffff\
             000000000000000000000000000000000000000000bf0cb2018133faff0c03faff0c03\
             dd070050f208000000",
        );
        let sig = probe_req_flock_ie_sig_for_clustering(&mpdu).expect("sig");
        assert_eq!(sig, FLOCK_PROBE_IE_SIG_PRIMARY_DEFAULT);
    }

    #[test]
    fn probe_flock_ie_sig_pre_canonical_was_alt_linux() {
        fn hex_bytes(s: &str) -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        }
        let mpdu = hex_bytes(
            "40000000ffffffffffffd0395786d0b9ffffffffffff8017000001080c1218243048606c\
             dd070050f208001f007f09040000000000004080dd07506f9a160301032d1aef011fffff\
             000000000000000000000000000000000000000000bf0cb2018133faff0c03faff0c03\
             dd070050f208000000",
        );
        let raw = flock_probe_ie_sig_from_ies_ex(&mpdu[PROBE_REQ_BODY_OFFSET..])
            .map(|(s, _)| s)
            .or_else(|| flock_probe_ie_sig_from_ies_ex(&mpdu[26..]).map(|(s, _)| s));
        assert_eq!(raw.as_deref(), Some(FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX));
    }

    #[test]
    fn flock_ie_sig_vendor_and_tags_match_python_shape() {
        let mut p = vec![
            0x40, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        // Explicit broadcast wildcard: SSID IE length 0 — must not prefix ie_sig with "0,".
        p.extend_from_slice(&[0x00, 0x00]);
        // Supported rates IE id 1, len 2.
        p.extend_from_slice(&[0x01, 0x02, 0x02, 0x04]);
        // Vendor IE: OUI 00:50:F2 type 04 (WPS marker) — first 4 body octets hex.
        p.push(IE_VENDOR);
        p.push(4);
        p.extend_from_slice(&[0x00, 0x50, 0xf2, 0x04]);
        let sig = probe_req_flock_ie_sig_for_clustering(&p).expect("sig");
        assert_eq!(sig, "1,221:0050f204");
    }

    #[test]
    fn wifi_mgmt_assoc_req_parses_sta_bssid() {
        let mut f = Vec::new();
        f.extend_from_slice(&[0x00, 0x00]); // assoc req mgmt
        f.extend_from_slice(&[0x00, 0x00]); // duration
        f.extend_from_slice(&[1, 2, 3, 4, 5, 6]); // BSSID addr1
        f.extend_from_slice(&[0xa, 0xb, 0xc, 0xd, 0xe, 0xf]); // STA addr2
        f.extend_from_slice(&[1, 2, 3, 4, 5, 6]); // addr3 = BSSID
        f.extend_from_slice(&[0x00, 0x00]); // seq
        f.extend_from_slice(&[0x00, 0x00, 0x0a, 0x00]); // cap + listen
        let ev = try_wifi_mgmt_link_from_mpdu(&f).expect("mgmt");
        assert_eq!(ev.frame_kind, "assoc_req");
        assert_eq!(ev.sta_mac, [0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f]);
        assert_eq!(ev.bssid_mac, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn wifi_mgmt_assoc_resp_parses_status() {
        let mut f = Vec::new();
        f.extend_from_slice(&[0x10, 0x00]); // assoc resp
        f.extend_from_slice(&[0x00, 0x00]);
        f.extend_from_slice(&[0xa, 0xb, 0xc, 0xd, 0xe, 0xf]); // DA = STA
        f.extend_from_slice(&[1, 2, 3, 4, 5, 6]); // SA = BSSID
        f.extend_from_slice(&[1, 2, 3, 4, 5, 6]); // BSSID
        f.extend_from_slice(&[0x00, 0x00]);
        f.extend_from_slice(&0u16.to_le_bytes()); // status success
        f.extend_from_slice(&0u16.to_le_bytes()); // aid placeholder
        let ev = try_wifi_mgmt_link_from_mpdu(&f).expect("resp");
        assert_eq!(ev.frame_kind, "assoc_resp");
        assert_eq!(ev.status_code, Some(0));
        assert_eq!(ev.sta_mac, [0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f]);
        assert_eq!(ev.bssid_mac, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn wifi_eapol_key_qos_data_downlink() {
        // QoS Data, FromDS=1 ToDS=0: FC 0x88 0x02
        let mut f = vec![0x88u8, 0x02, 0x00, 0x00];
        f.extend_from_slice(&[0xa, 0xb, 0xc, 0xd, 0xe, 0xf]); // addr1 DA = STA
        f.extend_from_slice(&[1, 2, 3, 4, 5, 6]); // addr2 BSSID
        f.extend_from_slice(&[1, 2, 3, 4, 5, 6]); // addr3
        f.extend_from_slice(&[0x00, 0x00]); // seq
        f.extend_from_slice(&[0x00, 0x00]); // QoS control
        f.extend_from_slice(&[0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00, 0x88, 0x8e]); // SNAP EAPOL
        f.extend_from_slice(&[0x02, 0x03, 0x00, 0x0f]); // EAPOL ver 2, type 3 Key, len, ...
        let ev = try_wifi_eapol_key_from_mpdu(&f).expect("eapol");
        assert_eq!(ev.sta_mac, [0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f]);
        assert_eq!(ev.bssid_mac, [1, 2, 3, 4, 5, 6]);
    }
}
