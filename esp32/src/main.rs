use chrono::{Datelike, Days, NaiveTime, Utc};
use embedded_svc::http::Method;
use embedded_svc::io::{Read as ERead, Write as EWrite};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::http::client::{
    Configuration as ClientConfig, EspHttpConnection as ClientConnection,
};
use esp_idf_svc::http::server::{Configuration as ServerConfig, EspHttpServer};
use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::sys::{EspError, ESP_FAIL};
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use flate2::write::GzEncoder;
use flate2::Compression;
use log::{error, info, warn};
use rust_embed::RustEmbed;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write as StdWrite;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

type HResult = Result<(), EspError>;
type HttpClient = embedded_svc::http::client::Client<ClientConnection>;

fn esp_fail() -> EspError {
    EspError::from(ESP_FAIL).unwrap()
}

// =============================================
// CONFIGURATION & CONSTANTS
// =============================================
const DATA_DIR: &str = "/spiffs";
const WIFI_SSID: &str = match option_env!("HEX_WIFI_SSID") {
    Some(s) => s,
    None => "MySSID",
};
const WIFI_PASS: &str = match option_env!("HEX_WIFI_PASS") {
    Some(s) => s,
    None => "MyPassword",
};
const RPC_ENDPOINTS: &[&str] = &[
    "https://rpc.pulsechain.com",
    "https://rpc-pulsechain.g4mm4.io",
    "https://pulsechain-rpc.publicnode.com",
    "https://rpc.pulsechainrpc.com",
];
const HEX_CONTRACT: &str = "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";
const DEXSCREENER_URL: &str = "https://api.dexscreener.com/latest/dex/tokens/0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";
const GECKOTERMINAL_URL: &str = "https://api.geckoterminal.com/api/v2/networks/pulsechain/tokens/0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";
const HEXDAILYSTATS_URL: &str = "https://hexdailystats.com/fulldatapulsechain";
const GLOBALS_SELECTOR: &str = "0xc3124525";
const DAILY_DATA_SELECTOR: &str = "0x90de6871";
const HEARTS_PER_HEX: f64 = 1e8;
const TSHARE_UNIT: f64 = 10000.0;
const ENABLE_HDS_BACKFILL: bool = true;
const MAX_HTTP_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const DAILY_RECORD_SETTLE_DELAY_SECS: u64 = 120;
const DEFAULT_RPC_BATCH_SIZE: usize = 25;
const DEFAULT_BACKFILL_SAVE_INTERVAL: usize = 500;
const WIFI_MONITOR_INTERVAL_SECS: u64 = 15;
const NTP_CHECK_INTERVAL_SECS: u64 = 3600;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

// =============================================
// LOGGING
// =============================================
const LOG_RING_CAPACITY: usize = 100;
const LOG_LINE_MAX: usize = 128;
static LOG_RING: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

struct RingLogger;
impl log::Log for RingLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ts = unsafe { esp_idf_svc::sys::esp_log_timestamp() };
        let line: String = format!("[{:>8} ms] {:5} {}", ts, record.level(), record.args())
            .chars()
            .take(LOG_LINE_MAX)
            .collect();
        if let Ok(mut g) = LOG_RING.lock() {
            g.push_back(line);
            while g.len() > LOG_RING_CAPACITY {
                g.pop_front();
            }
        }
        let level: u32 = match record.level() {
            log::Level::Error => 1,
            log::Level::Warn => 2,
            log::Level::Info => 3,
            log::Level::Debug => 4,
            log::Level::Trace => 5,
        };
        let msg = format!("{}\0", record.args());
        let tag = format!("{}\0", record.target());
        let fmt = b"%s\0";
        unsafe {
            esp_idf_svc::sys::esp_log_write(
                level,
                tag.as_ptr() as *const std::os::raw::c_char,
                fmt.as_ptr() as *const std::os::raw::c_char,
                msg.as_ptr() as *const std::os::raw::c_char,
            );
        }
    }
    fn flush(&self) {}
}
static RING_LOGGER: RingLogger = RingLogger;

// =============================================
// CUSTOM DESERIALIZERS
// =============================================
fn f64_or_default<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or(0.0))
}
fn u64_or_default<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or(0))
}

// =============================================
// DATA STRUCTURES
// =============================================
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HexJsonEntry {
    #[serde(rename = "currentDay", default, deserialize_with = "u64_or_default")]
    pub current_day: u64,
    #[serde(rename = "tshareRateHEX", default, deserialize_with = "f64_or_default")]
    pub tshare_rate_hex: f64,
    #[serde(rename = "dailyPayoutHEX", default, deserialize_with = "f64_or_default")]
    pub daily_payout_hex: f64,
    #[serde(rename = "payoutPerTshareHEX", default, deserialize_with = "f64_or_default")]
    pub payout_per_tshare_hex: f64,
    #[serde(rename = "pricePulseX", default, deserialize_with = "f64_or_default")]
    pub price_pulse_x: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LiveData {
    #[serde(rename = "price_Pulsechain")]
    pub price_pulsechain: f64,
    #[serde(rename = "tsharePrice_Pulsechain")]
    pub tshare_price_pulsechain: f64,
    #[serde(rename = "tshareRateHEX_Pulsechain")]
    pub tshare_rate_hex_pulsechain: f64,
    #[serde(rename = "penaltiesHEX_Pulsechain")]
    pub penalties_hex_pulsechain: f64,
    #[serde(rename = "payoutPerTshare_Pulsechain")]
    pub payout_per_tshare_pulsechain: f64,
    #[serde(rename = "beat")]
    pub beat: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Miner {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(rename = "startDate")]
    pub start_date: String,
    #[serde(rename = "endDate")]
    pub end_date: String,
    #[serde(rename = "tShares")]
    pub t_shares: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Deserialize)]
struct AddMinerRequest {
    #[serde(rename = "startDate")]
    start_date: String,
    #[serde(rename = "endDate")]
    end_date: String,
    #[serde(rename = "tShares")]
    t_shares: f64,
}

#[derive(Deserialize)]
struct MinerIdRequest {
    id: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(rename = "liveDataFrequency")]
    pub live_data_frequency: u64,
    #[serde(rename = "liquidHEX")]
    pub liquid_hex: f64,
    #[serde(rename = "historicalStartDay")]
    pub historical_start_day: u64,
    #[serde(rename = "backfillDelayMs", default = "default_backfill_delay")]
    pub backfill_delay_ms: u64,
    #[serde(rename = "backfillSaveInterval", default = "default_backfill_save_interval")]
    pub backfill_save_interval: usize,
}
fn default_backfill_delay() -> u64 { 100 }
fn default_backfill_save_interval() -> usize { DEFAULT_BACKFILL_SAVE_INTERVAL }

#[derive(Serialize, Clone, Debug, Default)]
pub struct WifiDiagnostics {
    pub connected: bool,
    pub rssi: i32,
    #[serde(rename = "ipAddr")]
    pub ip_addr: String,
    #[serde(rename = "disconnectCount")]
    pub disconnect_count: u32,
    #[serde(rename = "ssid")]
    pub ssid: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct BackfillCheckpoint {
    last_backfilled_day: u64,
    total_days_known: u64,
}

// =============================================
// APPLICATION STATE
// =============================================
#[derive(Default)]
struct EventFlag {
    pair: Mutex<bool>,
    cond: Condvar,
}
impl EventFlag {
    fn notify(&self) {
        *self.pair.lock().unwrap() = true;
        self.cond.notify_all();
    }
    fn wait_timeout(&self, dur: Duration) -> bool {
        let mut g = self.pair.lock().unwrap();
        let deadline = Instant::now() + dur;
        while !*g {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (ng, _) = self.cond.wait_timeout(g, deadline - now).unwrap();
            g = ng;
        }
        let fired = *g;
        *g = false;
        fired
    }
}

struct AppState {
    live_data: RwLock<LiveData>,
    hex_json: RwLock<Arc<Vec<HexJsonEntry>>>,
    miners: RwLock<Vec<Miner>>,
    config: RwLock<Config>,
    config_event: EventFlag,
    active_rpc_idx: Mutex<usize>,
    hex_json_version: AtomicU32,
    next_miner_id: AtomicU32,
    wifi_diag: RwLock<WifiDiagnostics>,
    backfill_in_progress: AtomicBool,
    time_synced: AtomicBool,
}

// =============================================
// HELPERS
// =============================================
fn sanitize_config(config: Config) -> Config {
    Config {
        live_data_frequency: config.live_data_frequency.clamp(1, 24 * 60),
        liquid_hex: if config.liquid_hex.is_finite() && config.liquid_hex >= 0.0 {
            config.liquid_hex
        } else {
            0.0
        },
        historical_start_day: config.historical_start_day.max(1),
        backfill_delay_ms: config.backfill_delay_ms.clamp(0, 5000),
        backfill_save_interval: config.backfill_save_interval.clamp(50, 5000),
    }
}

fn normalize_miners(mut miners: Vec<Miner>) -> (Vec<Miner>, u64) {
    let mut next_id = miners.iter().filter_map(|m| m.id).max().unwrap_or(0) + 1;
    for miner in miners.iter_mut() {
        if miner.id.is_none() {
            miner.id = Some(next_id);
            next_id += 1;
        }
    }
    (miners, next_id)
}

fn calc_payout_per_tshare(payout_hearts: f64, shares: f64) -> f64 {
    if shares <= 0.0 || !shares.is_finite() || !payout_hearts.is_finite() {
        return 0.0;
    }
    (payout_hearts / shares) * TSHARE_UNIT
}

fn is_valid_daily_entry(entry: &HexJsonEntry) -> bool {
    entry.daily_payout_hex > 0.0
        || entry.payout_per_tshare_hex > 0.0
        || entry.tshare_rate_hex > 0.0
}

fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let date = chrono::NaiveDate::parse_from_str(s, "%d-%m-%Y").ok()?;
    let (y, m, d) = (date.year(), date.month(), date.day());
    if !(2000..=2100).contains(&y) {
        return None;
    }
    Some((y, m, d))
}

fn get_mime_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

fn is_compressible_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.contains("javascript")
        || mime.contains("json")
        || mime.contains("svg+xml")
}

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// =============================================
// CUSTOM U256
// =============================================
#[derive(Clone, Copy, PartialEq, Eq)]
struct U256([u64; 4]);

impl U256 {
    const ZERO: Self = Self([0; 4]);

    fn from_hex(s: &str) -> Self {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let mut limbs = [0u64; 4];
        if s.is_empty() {
            return Self::ZERO;
        }
        let mut limb_idx = 0;
        let mut shift = 0;
        for c in s.chars().rev() {
            let val = match c {
                '0'..='9' => c as u64 - '0' as u64,
                'a'..='f' => c as u64 - 'a' as u64 + 10,
                'A'..='F' => c as u64 - 'A' as u64 + 10,
                _ => continue,
            };
            if limb_idx < 4 {
                limbs[limb_idx] |= val << shift;
            }
            shift += 4;
            if shift == 64 {
                shift = 0;
                limb_idx += 1;
            }
        }
        Self(limbs)
    }

    fn to_f64(self) -> f64 {
        let mut result: f64 = 0.0;
        for (i, limb) in self.0.iter().enumerate() {
            if *limb != 0 {
                result += (*limb as f64) * (2.0_f64).powi(64 * i as i32);
            }
        }
        result
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for U256 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        for i in (0..4).rev() {
            match self.0[i].cmp(&other.0[i]) {
                std::cmp::Ordering::Equal => continue,
                ord => return ord,
            }
        }
        std::cmp::Ordering::Equal
    }
}

impl std::ops::Sub<u64> for U256 {
    type Output = Self;
    fn sub(self, rhs: u64) -> Self {
        let mut limbs = self.0;
        let (res, overflow) = limbs[0].overflowing_sub(rhs);
        limbs[0] = res;
        let mut borrow: u64 = if overflow { 1 } else { 0 };
        for limb in limbs.iter_mut().skip(1) {
            if borrow == 0 {
                break;
            }
            let (res, ov) = limb.overflowing_sub(borrow);
            *limb = res;
            borrow = if ov { 1 } else { 0 };
        }
        Self(limbs)
    }
}

// =============================================
// UTC TIME, SPIFFS, PERSISTENCE
// =============================================
fn get_duration_until_next_1am_utc() -> Duration {
    let now = Utc::now();
    let today_1am = now
        .date_naive()
        .and_time(NaiveTime::from_hms_opt(1, 0, 0).unwrap());
    let mut next_1am = today_1am.and_utc();
    if next_1am <= now {
        next_1am = next_1am.checked_add_days(Days::new(1)).unwrap();
    }
    (next_1am - now)
        .to_std()
        .unwrap_or(Duration::from_secs(60))
}

fn mount_spiffs() {
    let conf = esp_idf_svc::sys::esp_vfs_spiffs_conf_t {
        base_path: b"/spiffs\0".as_ptr() as *const _,
        partition_label: b"storage\0".as_ptr() as *const _,
        max_files: 4,
        format_if_mount_failed: true,
    };
    let ret = unsafe { esp_idf_svc::sys::esp_vfs_spiffs_register(&conf) };
    if ret != 0 {
        panic!("SPIFFS mount failed: {ret:#x}");
    }
    info!("SPIFFS mounted at /spiffs");
}

fn hexjson_file_path() -> String {
    format!("{}/hexjson.json", DATA_DIR)
}
fn config_file_path() -> String {
    format!("{}/config.json", DATA_DIR)
}
fn miners_file_path() -> String {
    format!("{}/miners.json", DATA_DIR)
}
fn checkpoint_file_path() -> String {
    format!("{}/backfill_cp.json", DATA_DIR)
}

fn save_hex_json_to_file(data: &[HexJsonEntry]) {
    let path = hexjson_file_path();
    match fs::File::create(&path) {
        Ok(f) => {
            let w = std::io::BufWriter::with_capacity(4096, f);
            if let Err(e) = serde_json::to_writer(w, data) {
                error!("Failed to serialize hexjson: {}", e);
            }
        }
        Err(e) => error!("Failed to save hexjson to {}: {}", path, e),
    }
}

fn load_hex_json_from_file() -> Vec<HexJsonEntry> {
    let path = hexjson_file_path();
    match fs::File::open(&path) {
        Ok(f) => {
            match serde_json::from_reader::<_, Vec<HexJsonEntry>>(
                std::io::BufReader::with_capacity(4096, f),
            ) {
                Ok(data) => {
                    info!("Loaded {} hexjson entries from file", data.len());
                    data
                }
                Err(e) => {
                    warn!("Failed to parse hexjson file: {}. Starting fresh.", e);
                    Vec::new()
                }
            }
        }
        Err(_) => {
            info!("No hexjson file found. Will build from RPC + external sources.");
            Vec::new()
        }
    }
}

fn save_config_to_file(config: &Config) {
    if let Err(e) = fs::write(
        config_file_path(),
        serde_json::to_string_pretty(config).unwrap_or_default(),
    ) {
        error!("Failed to save config: {}", e);
    }
}

fn save_miners_to_file(miners: &[Miner]) {
    if let Err(e) = fs::write(
        miners_file_path(),
        serde_json::to_string_pretty(miners).unwrap_or_default(),
    ) {
        error!("Failed to save miners: {}", e);
    }
}

fn save_checkpoint(cp: &BackfillCheckpoint) {
    if let Ok(json) = serde_json::to_string(cp) {
        let _ = fs::write(checkpoint_file_path(), json);
    }
}

fn load_checkpoint() -> Option<BackfillCheckpoint> {
    let content = fs::read_to_string(checkpoint_file_path()).ok()?;
    serde_json::from_str(&content).ok()
}

// =============================================
// HTTPS CLIENT & STREAMING READER
// =============================================
struct EspStdReader<'a, T> {
    inner: &'a mut T,
}
impl<'a, T: embedded_svc::io::Read> std::io::Read for EspStdReader<'a, T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner
            .read(buf)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "ESP read error"))
    }
}

fn make_http_config() -> ClientConfig {
    ClientConfig {
        buffer_size: Some(4096),
        buffer_size_tx: Some(2048),
        timeout: Some(Duration::from_secs(30)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    }
}

fn http_request(
    method: Method,
    url: &str,
    post_body: Option<&[u8]>,
) -> Result<(u16, Vec<u8>), String> {
    let cfg = make_http_config();
    let conn = ClientConnection::new(&cfg).map_err(|e| format!("client: {}", e))?;
    let mut client = HttpClient::wrap(conn);
    let headers: &[(&str, &str)] = if post_body.is_some() {
        &[
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
            ("User-Agent", "hexfetch-esp32/1.0"),
        ]
    } else {
        &[
            ("Accept", "application/json"),
            ("User-Agent", "hexfetch-esp32/1.0"),
        ]
    };
    let mut req = client
        .request(method, url, headers)
        .map_err(|e| format!("request to {} failed: {}", url, e))?;
    if let Some(body) = post_body {
        EWrite::write_all(&mut req, body).map_err(|e| e.to_string())?;
    }
    let mut resp = req.submit().map_err(|e| e.to_string())?;
    let status = resp.status();
    let mut body = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = ERead::read(&mut resp, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        if body.len() >= MAX_HTTP_RESPONSE_BYTES {
            break;
        }
    }
    drop(resp);
    Ok((status as u16, body))
}

struct RpcSession {
    client: Option<HttpClient>,
    url: String,
}

impl RpcSession {
    fn new(url: &str) -> Self {
        Self {
            client: None,
            url: url.to_string(),
        }
    }

    fn ensure_client(&mut self) -> Result<&mut HttpClient, String> {
        if self.client.is_none() {
            let cfg = make_http_config();
            let conn =
                ClientConnection::new(&cfg).map_err(|e| format!("RpcSession client: {}", e))?;
            self.client = Some(HttpClient::wrap(conn));
        }
        Ok(self.client.as_mut().unwrap())
    }

    fn reset(&mut self) {
        self.client = None;
    }

    /// Send a POST with automatic retry on stale connections
    fn post(&mut self, body: &[u8]) -> Result<(u16, Vec<u8>), String> {
        match self.try_post(body) {
            Ok(r) => Ok(r),
            Err(first_err) => {
                self.reset();
                self.try_post(body)
                    .map_err(|e| format!("retry failed: {} (original: {})", e, first_err))
            }
        }
    }

    fn try_post(&mut self, body: &[u8]) -> Result<(u16, Vec<u8>), String> {
        let url = self.url.clone();
        let client = self.ensure_client()?;
        let headers: &[(&str, &str)] = &[
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
            ("User-Agent", "hexfetch-esp32/1.0"),
        ];
        let mut req = client
            .request(Method::Post, &url, headers)
            .map_err(|e| format!("request failed: {}", e))?;
        EWrite::write_all(&mut req, body).map_err(|e| e.to_string())?;
        let mut resp = req.submit().map_err(|e| e.to_string())?;
        let status = resp.status();
        let mut response_body = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = ERead::read(&mut resp, &mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            response_body.extend_from_slice(&buf[..n]);
            if response_body.len() >= MAX_HTTP_RESPONSE_BYTES {
                break;
            }
        }
        drop(resp);
        Ok((status as u16, response_body))
    }
}

// =============================================
// RPC & DATA FETCHING
// =============================================
fn call_rpc(
    state: &Arc<AppState>,
    method: &str,
    params: serde_json::Value,
) -> Result<String, String> {
    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });
    let body_bytes = serde_json::to_vec(&req_body).map_err(|e| e.to_string())?;
    let start_idx = *state.active_rpc_idx.lock().unwrap();
    let mut last_err = String::new();
    for i in 0..RPC_ENDPOINTS.len() {
        let idx = (start_idx + i) % RPC_ENDPOINTS.len();
        let url = RPC_ENDPOINTS[idx];
        match http_request(Method::Post, url, Some(&body_bytes)) {
            Ok((status, body)) => {
                if !(200..300).contains(&status) {
                    last_err = format!("HTTP {} on {}", status, url);
                    continue;
                }
                match serde_json::from_slice::<serde_json::Value>(&body) {
                    Ok(json) => {
                        if let Some(err) = json.get("error") {
                            last_err = format!(
                                "RPC error on {}: {}",
                                url,
                                err["message"].as_str().unwrap_or("unknown")
                            );
                            continue;
                        }
                        if idx != start_idx {
                            info!("Switched to fallback RPC: {} (index {})", url, idx);
                            *state.active_rpc_idx.lock().unwrap() = idx;
                        }
                        return Ok(json["result"].as_str().unwrap_or("").to_string());
                    }
                    Err(e) => {
                        last_err = format!("JSON parse error on {}: {}", url, e);
                        continue;
                    }
                }
            }
            Err(e) => {
                last_err = e;
                continue;
            }
        }
    }
    Err(format!(
        "All RPC endpoints failed. Last error: {}",
        last_err
    ))
}

fn read_globals(state: &Arc<AppState>) -> Result<(f64, u64, f64), String> {
    let params = serde_json::json!([{"to": HEX_CONTRACT, "data": GLOBALS_SELECTOR}, "latest"]);
    let hex_result = call_rpc(state, "eth_call", params)?;
    let g_str = hex_result.strip_prefix("0x").unwrap_or(&hex_result);
    if g_str.len() < 320 {
        return Err(format!("globals() too short ({} chars)", g_str.len()));
    }
    let share_rate_raw = U256::from_hex(&g_str[128..192]);
    let penalty_raw = U256::from_hex(&g_str[192..256]);
    let daily_data_count = U256::from_hex(&g_str[256..320]);
    let tshare_rate = if share_rate_raw > U256::ZERO {
        share_rate_raw.to_f64() / 10.0
    } else {
        0.0
    };
    let penalties = penalty_raw.to_f64() / 1e8;
    let day_count = daily_data_count.to_f64() as u64;
    Ok((tshare_rate, day_count, penalties))
}

fn read_daily_data(state: &Arc<AppState>, day: u64) -> Result<(f64, f64), String> {
    let call_data = format!("{}{:064x}", DAILY_DATA_SELECTOR, day);
    let params = serde_json::json!([{"to": HEX_CONTRACT, "data": call_data}, "latest"]);
    let hex_result = call_rpc(state, "eth_call", params)?;
    let d_str = hex_result.strip_prefix("0x").unwrap_or(&hex_result);
    if d_str.len() < 128 {
        return Err(format!(
            "dailyData({}) too short ({} chars)",
            day,
            d_str.len()
        ));
    }
    let day_payout = U256::from_hex(&d_str[0..64]);
    let day_shares = U256::from_hex(&d_str[64..128]);
    Ok((day_payout.to_f64(), day_shares.to_f64()))
}

fn batch_read_daily_data(
    session: &mut RpcSession,
    days: &[u64],
) -> Result<Vec<(u64, f64, f64)>, String> {
    if days.is_empty() {
        return Ok(Vec::new());
    }
    let requests: Vec<serde_json::Value> = days
        .iter()
        .enumerate()
        .map(|(i, day)| {
            let call_data = format!("{}{:064x}", DAILY_DATA_SELECTOR, day);
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_call",
                "params": [{"to": HEX_CONTRACT, "data": call_data}, "latest"],
                "id": i + 1
            })
        })
        .collect();

    let body_bytes = serde_json::to_vec(&requests).map_err(|e| e.to_string())?;
    let (status, resp_body) = session.post(&body_bytes)?;

    if !(200..300).contains(&status) {
        return Err(format!("Batch RPC HTTP {}", status));
    }

    let parsed: serde_json::Value =
        serde_json::from_slice(&resp_body).map_err(|e| format!("Batch parse: {}", e))?;

    let responses = match parsed.as_array() {
        Some(arr) => arr,
        None => return Err("RPC endpoint does not support batch requests".to_string()),
    };

    let mut results = Vec::with_capacity(days.len());
    for (i, day) in days.iter().enumerate() {
        let resp = responses
            .iter()
            .find(|r| r["id"].as_u64() == Some((i + 1) as u64))
            .or_else(|| responses.get(i));

        if let Some(r) = resp {
            if r.get("error").is_some() {
                continue;
            }
            let hex_result = r["result"].as_str().unwrap_or("");
            let d_str = hex_result.strip_prefix("0x").unwrap_or(hex_result);
            if d_str.len() >= 128 {
                let day_payout = U256::from_hex(&d_str[0..64]);
                let day_shares = U256::from_hex(&d_str[64..128]);
                results.push((*day, day_payout.to_f64(), day_shares.to_f64()));
            }
        }
    }
    Ok(results)
}

fn fetch_price_dexscreener() -> Result<f64, String> {
    let (status, body) = http_request(Method::Get, DEXSCREENER_URL, None)?;
    if !(200..300).contains(&status) {
        return Err(format!("DEXScreener HTTP {}", status));
    }
    let resp: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("parse: {}", e))?;

    let price = resp
        .get("pairs")
        .and_then(|pairs| pairs.as_array())
        .and_then(|pairs| {
            pairs.iter().find(|pair| {
                pair.get("chainId")
                    .and_then(|c| c.as_str())
                    .map(|c| c == "pulsechain")
                    .unwrap_or(false)
            })
        })
        .and_then(|pair| pair.get("priceUsd"))
        .and_then(|price| {
            price
                .as_f64()
                .or_else(|| price.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0.0);

    if price <= 0.0 || !price.is_finite() {
        return Err("DEXScreener returned zero or invalid price".to_string());
    }

    Ok(price)
}

fn fetch_price_geckoterminal() -> Result<f64, String> {
    let (status, body) = http_request(Method::Get, GECKOTERMINAL_URL, None)?;
    if !(200..300).contains(&status) {
        return Err(format!("GeckoTerminal HTTP {}", status));
    }
    let resp: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("parse: {}", e))?;

    let price = resp
        .get("data")
        .and_then(|data| data.get("attributes"))
        .and_then(|attrs| attrs.get("price_usd"))
        .and_then(|price| price.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    if price <= 0.0 || !price.is_finite() {
        return Err("GeckoTerminal returned zero or invalid price".to_string());
    }

    Ok(price)
}

fn fetch_price() -> Result<f64, String> {
    match fetch_price_dexscreener() {
        Ok(price) => return Ok(price),
        Err(e) => {
            warn!("DexScreener failed: {}. Falling back to GeckoTerminal...", e);
        }
    }

    fetch_price_geckoterminal()
}

fn fetch_hexdailystats_backfill() -> (HashMap<u64, f64>, HashMap<u64, f64>) {
    if !ENABLE_HDS_BACKFILL {
        return (HashMap::new(), HashMap::new());
    }
    info!("Attempting historical fetch from HEXDailyStats...");
    let cfg = make_http_config();
    let conn = match ClientConnection::new(&cfg) {
        Ok(c) => c,
        Err(e) => {
            warn!("HEXDailyStats client failed: {}", e);
            return (HashMap::new(), HashMap::new());
        }
    };
    let mut client = HttpClient::wrap(conn);
    let headers: &[(&str, &str)] = &[
        ("Accept", "application/json"),
        ("User-Agent", "hexfetch-esp32/1.0"),
    ];
    let req = match client.request(Method::Get, HEXDAILYSTATS_URL, headers) {
        Ok(r) => r,
        Err(e) => {
            warn!("HEXDailyStats request failed: {}", e);
            return (HashMap::new(), HashMap::new());
        }
    };
    let mut resp = match req.submit() {
        Ok(r) => r,
        Err(e) => {
            warn!("HEXDailyStats submit failed: {}", e);
            return (HashMap::new(), HashMap::new());
        }
    };
    let status = resp.status();
    if !(200..300).contains(&status) {
        warn!("HEXDailyStats HTTP {}", status);
        drop(resp);
        return (HashMap::new(), HashMap::new());
    }
    let mut esp_reader = EspStdReader { inner: &mut resp };
    let buf_reader = std::io::BufReader::with_capacity(4096, &mut esp_reader);
    let entries: Vec<HexJsonEntry> = match serde_json::from_reader(buf_reader) {
        Ok(e) => e,
        Err(e) => {
            warn!("HEXDailyStats JSON parse failed: {}", e);
            drop(resp);
            return (HashMap::new(), HashMap::new());
        }
    };
    drop(resp);
    let mut tshare_map: HashMap<u64, f64> = HashMap::with_capacity(entries.len());
    let mut price_map: HashMap<u64, f64> = HashMap::with_capacity(entries.len());
    for entry in &entries {
        if entry.tshare_rate_hex > 0.0 && entry.tshare_rate_hex.is_finite() {
            tshare_map.insert(entry.current_day, entry.tshare_rate_hex);
        }
        if entry.price_pulse_x > 0.0 && entry.price_pulse_x.is_finite() {
            price_map.insert(entry.current_day, entry.price_pulse_x);
        }
    }
    (tshare_map, price_map)
}

// =============================================
// BACKFILL & DAILY RECORDING
// =============================================
fn store_daily_entry(state: &Arc<AppState>, entry: HexJsonEntry) -> bool {
    let mut hex_json = state.hex_json.write().unwrap();
    if hex_json.iter().any(|e| e.current_day == entry.current_day) {
        return false;
    }
    let mut new_data = (**hex_json).clone();
    new_data.push(entry);
    new_data.sort_by_key(|e| e.current_day);
    *hex_json = Arc::new(new_data);
    state.hex_json_version.fetch_add(1, Ordering::Relaxed);
    let data_clone = hex_json.clone();
    drop(hex_json);
    save_hex_json_to_file(&data_clone);
    true
}

// =============================================
// IMPROVED BACKFILL
// =============================================
fn backfill_hex_json(state: &Arc<AppState>, existing_data: &[HexJsonEntry]) -> Vec<HexJsonEntry> {
    info!("Starting HEXJSON backfill...");
    state.backfill_in_progress.store(true, Ordering::Relaxed);

    let config = state.config.read().unwrap().clone();
    let backfill_delay_ms = config.backfill_delay_ms;
    let save_interval = config.backfill_save_interval;

    let (current_tshare_rate, day_count, _penalties) = match read_globals(state) {
        Ok(v) => v,
        Err(e) => {
            error!("Backfill failed: cannot read globals(): {}", e);
            state.backfill_in_progress.store(false, Ordering::Relaxed);
            return existing_data.to_vec();
        }
    };
    if day_count == 0 {
        state.backfill_in_progress.store(false, Ordering::Relaxed);
        return existing_data.to_vec();
    }

    let (hds_tshares, hds_prices) = fetch_hexdailystats_backfill();
    let current_price = fetch_price().unwrap_or_else(|e| {
        warn!("Price fetch failed: {}. Using 0.0.", e);
        0.0
    });
    let mut by_day: BTreeMap<u64, HexJsonEntry> = BTreeMap::new();
    for entry in existing_data {
        by_day.insert(entry.current_day, entry.clone());
    }

    let checkpoint = load_checkpoint();
    let resume_from = checkpoint
        .as_ref()
        .map(|cp| cp.last_backfilled_day + 1)
        .unwrap_or(1);

    let missing_days: Vec<u64> = (resume_from..day_count)
        .filter(|day| !by_day.contains_key(day))
        .collect();

    if missing_days.is_empty() {
        info!("No missing days to backfill.");
        state.backfill_in_progress.store(false, Ordering::Relaxed);
        return by_day.into_values().collect();
    }

    let total_to_fetch = missing_days.len();
    info!(
        "Backfilling {} missing days (day {}..{}) using batch size {}...",
        total_to_fetch,
        missing_days.first().unwrap_or(&0),
        missing_days.last().unwrap_or(&0),
        DEFAULT_RPC_BATCH_SIZE
    );

    let active_idx = *state.active_rpc_idx.lock().unwrap();
    let mut session = RpcSession::new(RPC_ENDPOINTS[active_idx]);
    let mut use_batching = true;
    let mut fetched = 0usize;
    let mut consecutive_errors = 0u32;
    let mut last_checkpoint_day: u64 = resume_from.saturating_sub(1);

    for chunk in missing_days.chunks(DEFAULT_RPC_BATCH_SIZE) {
        let results = if use_batching {
            match batch_read_daily_data(&mut session, chunk) {
                Ok(r) => r,
                Err(e) => {
                    warn!("Batch RPC failed ({}), falling back to sequential", e);
                    use_batching = false;
                    session.reset();
                    sequential_fetch_chunk(state, chunk)
                }
            }
        } else {
            sequential_fetch_chunk(state, chunk)
        };

        if results.is_empty() && !chunk.is_empty() {
            consecutive_errors += 1;
            if consecutive_errors >= 10 {
                warn!("Too many consecutive batch errors, pausing backfill.");
                break;
            }
        } else {
            consecutive_errors = 0;
        }

        for (day, payout_hearts, shares) in &results {
            let daily_payout_hex = payout_hearts / HEARTS_PER_HEX;
            let payout_per_tshare = calc_payout_per_tshare(*payout_hearts, *shares);
            let price = hds_prices
                .get(day)
                .copied()
                .filter(|p| p.is_finite() && *p > 0.0)
                .unwrap_or(current_price);
            let tshare = hds_tshares
                .get(day)
                .copied()
                .filter(|t| t.is_finite() && *t > 0.0)
                .unwrap_or(current_tshare_rate);
            by_day.insert(
                *day,
                HexJsonEntry {
                    current_day: *day,
                    tshare_rate_hex: tshare,
                    daily_payout_hex,
                    payout_per_tshare_hex: payout_per_tshare,
                    price_pulse_x: price,
                },
            );
            if *day > last_checkpoint_day {
                last_checkpoint_day = *day;
            }
        }

        fetched += chunk.len();
        if fetched % 100 == 0 || fetched == total_to_fetch {
            info!("Backfill progress: {}/{} days", fetched, total_to_fetch);
        }

        if fetched % save_interval == 0 {
            let partial: Vec<HexJsonEntry> = by_day.values().cloned().collect();
            save_hex_json_to_file(&partial);
            save_checkpoint(&BackfillCheckpoint {
                last_backfilled_day: last_checkpoint_day,
                total_days_known: day_count,
            });
            // Update live state so API serves latest data during backfill
            let mut hex_json = state.hex_json.write().unwrap();
            *hex_json = Arc::new(partial);
            state.hex_json_version.fetch_add(1, Ordering::Relaxed);
        }
        if backfill_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(backfill_delay_ms));
        }
    }

    let result: Vec<HexJsonEntry> = by_day.values().cloned().collect();
    save_hex_json_to_file(&result);
    save_checkpoint(&BackfillCheckpoint {
        last_backfilled_day: last_checkpoint_day,
        total_days_known: day_count,
    });

    state.backfill_in_progress.store(false, Ordering::Relaxed);
    info!("Backfill complete. Total entries: {}", result.len());
    result
}

/// Sequential fallback when batch RPC is not supported
fn sequential_fetch_chunk(state: &Arc<AppState>, days: &[u64]) -> Vec<(u64, f64, f64)> {
    let mut results = Vec::new();
    for &day in days {
        match read_daily_data(state, day) {
            Ok((payout, shares)) => results.push((day, payout, shares)),
            Err(_) => {}
        }
    }
    results
}

fn record_daily_entry(state: &Arc<AppState>) -> Result<HexJsonEntry, String> {
    let (tshare_rate, day_count, _) = read_globals(state)?;
    if day_count == 0 {
        return Err("dailyDataCount is 0".to_string());
    }
    let target_day = day_count - 1;
    let (payout_hearts, shares) = read_daily_data(state, target_day)?;
    let daily_payout_hex = payout_hearts / HEARTS_PER_HEX;
    let payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);
    let price = fetch_price().unwrap_or_else(|e| {
        warn!("Price fetch failed during daily recording: {}. Using last live price fallback.", e);
        state.live_data.read().unwrap().price_pulsechain
    });
    Ok(HexJsonEntry {
        current_day: target_day,
        tshare_rate_hex: tshare_rate,
        daily_payout_hex,
        payout_per_tshare_hex: payout_per_tshare,
        price_pulse_x: price,
    })
}

fn fetch_live_data(state: &Arc<AppState>) -> Result<LiveData, String> {
    let price = match fetch_price() {
        Ok(p) => p,
        Err(e) => {
            warn!("Price fetch failed in live data loop: {}. Using 0.0", e);
            0.0
        }
    };

    let gas_price_hex = call_rpc(state, "eth_gasPrice", serde_json::json!([]))?;
    let beat = U256::from_hex(&gas_price_hex).to_f64() / 1e9;
    let (tshare_rate, daily_data_count, penalties) = read_globals(state)?;
    let mut payout_per_tshare = 0.0;
    if daily_data_count > 0 {
        if let Ok((payout_hearts, shares)) = read_daily_data(state, daily_data_count - 1) {
            payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);
        }
    }
    Ok(LiveData {
        price_pulsechain: price,
        tshare_price_pulsechain: tshare_rate * price,
        tshare_rate_hex_pulsechain: tshare_rate,
        penalties_hex_pulsechain: penalties,
        payout_per_tshare_pulsechain: payout_per_tshare,
        beat,
    })
}

fn fetch_live_data_with_retry(state: &Arc<AppState>) -> Result<LiveData, String> {
    let mut delay = Duration::from_secs(1);
    let start = Instant::now();
    loop {
        match fetch_live_data(state) {
            Ok(data) => return Ok(data),
            Err(e) => {
                if start.elapsed() > Duration::from_secs(2 * 60) {
                    return Err(e);
                }
                std::thread::sleep(delay);
                delay = std::cmp::min(delay * 2, Duration::from_secs(30));
            }
        }
    }
}

// =============================================
// BACKGROUND TASKS
// =============================================
fn live_data_updater(state: Arc<AppState>) {
    if let Ok(data) = fetch_live_data_with_retry(&state) {
        *state.live_data.write().unwrap() = data;
    }
    loop {
        let freq = sanitize_config(state.config.read().unwrap().clone()).live_data_frequency;
        state
            .config_event
            .wait_timeout(Duration::from_secs(freq * 60));
        if let Ok(data) = fetch_live_data_with_retry(&state) {
            *state.live_data.write().unwrap() = data;
        }
    }
}

fn hex_json_updater(state: Arc<AppState>) {
    // Load existing data immediately so API can serve it
    let file_data = load_hex_json_from_file();
    if !file_data.is_empty() {
        let mut hex_json = state.hex_json.write().unwrap();
        *hex_json = Arc::new(file_data.clone());
        state.hex_json_version.fetch_add(1, Ordering::Relaxed);
        drop(hex_json);
        info!(
            "Loaded {} entries – serving immediately while backfill runs.",
            file_data.len()
        );
    }

    ensure_time_synced(&state);

    // Backfill in background (API already serving whatever we have)
    let current_data = state.hex_json.read().unwrap().as_ref().clone();
    let need_backfill = match read_globals(&state) {
        Ok((_, day_count, _)) => {
            let known_days: HashSet<u64> = current_data.iter().map(|e| e.current_day).collect();
            (1..day_count).any(|day| !known_days.contains(&day))
        }
        Err(_) => !current_data.is_empty(),
    };

    if need_backfill || current_data.is_empty() {
        let updated = backfill_hex_json(&state, &current_data);
        save_hex_json_to_file(&updated);
        let mut hex_json = state.hex_json.write().unwrap();
        *hex_json = Arc::new(updated);
        state.hex_json_version.fetch_add(1, Ordering::Relaxed);
    }

    // Daily recording loop
    loop {
        std::thread::sleep(get_duration_until_next_1am_utc());
        std::thread::sleep(Duration::from_secs(DAILY_RECORD_SETTLE_DELAY_SECS));

        if !state.time_synced.load(Ordering::Relaxed) {
            warn!("Time not synced – skipping daily record.");
            continue;
        }

        let mut delay = Duration::from_secs(5);
        for _ in 0..10 {
            if let Ok(entry) = record_daily_entry(&state) {
                if is_valid_daily_entry(&entry) {
                    store_daily_entry(&state, entry);
                    break;
                }
            }
            std::thread::sleep(delay);
            delay = std::cmp::min(delay * 2, Duration::from_secs(120));
        }
    }
}

fn ensure_time_synced(state: &Arc<AppState>) {
    for attempt in 0..240 {
        if Utc::now().timestamp() > 1_700_000_000 {
            state.time_synced.store(true, Ordering::Relaxed);
            info!("Time synced: {}", Utc::now());
            return;
        }
        if attempt % 20 == 0 {
            info!("Waiting for NTP time sync...");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    warn!("Time sync timed out after 120s. Daily scheduling may be inaccurate.");
}

fn ntp_resync_checker(state: Arc<AppState>) {
    loop {
        std::thread::sleep(Duration::from_secs(NTP_CHECK_INTERVAL_SECS));
        let now = Utc::now().timestamp();
        let was_synced = state.time_synced.load(Ordering::Relaxed);
        let is_valid = now > 1_700_000_000;
        state.time_synced.store(is_valid, Ordering::Relaxed);
        if was_synced && !is_valid {
            warn!("NTP time lost! UTC timestamp: {}", now);
        } else if !was_synced && is_valid {
            info!("NTP time re-synced: {}", Utc::now());
        }
    }
}

fn test_rpc(url: &str) -> bool {
    let block_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_blockNumber",
        "params": [],
        "id": 1
    });
    let body = serde_json::to_vec(&block_req).unwrap_or_default();
    let basic_ok = match http_request(Method::Post, url, Some(&body)) {
        Ok((s, b)) if (200..300).contains(&s) => serde_json::from_slice::<serde_json::Value>(&b)
            .map(|j| j.get("error").is_none() && j.get("result").is_some())
            .unwrap_or(false),
        _ => false,
    };
    if !basic_ok {
        return false;
    }
    let call_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [{"to": HEX_CONTRACT, "data": GLOBALS_SELECTOR}, "latest"],
        "id": 2
    });
    let body = serde_json::to_vec(&call_req).unwrap_or_default();
    match http_request(Method::Post, url, Some(&body)) {
        Ok((s, b)) if (200..300).contains(&s) => serde_json::from_slice::<serde_json::Value>(&b)
            .map(|j| j.get("error").is_none() && j.get("result").is_some())
            .unwrap_or(false),
        _ => false,
    }
}

fn rpc_health_checker(state: Arc<AppState>) {
    loop {
        std::thread::sleep(Duration::from_secs(24 * 60 * 60));
        let current_idx = *state.active_rpc_idx.lock().unwrap();
        if current_idx != 0 && test_rpc(RPC_ENDPOINTS[0]) {
            *state.active_rpc_idx.lock().unwrap() = 0;
            info!("RPC health check: switched back to primary endpoint.");
        }
    }
}

// =============================================
// WIFI RECONNECTION MONITOR
// =============================================
fn wifi_monitor(state: Arc<AppState>) {
    loop {
        std::thread::sleep(Duration::from_secs(WIFI_MONITOR_INTERVAL_SECS));

        unsafe {
            let netif = esp_idf_svc::sys::esp_netif_get_handle_from_ifkey(
                b"WIFI_STA_DEF\0".as_ptr() as *const _,
            );
            if netif.is_null() {
                continue;
            }

            let mut ip_info: esp_idf_svc::sys::esp_netif_ip_info_t = std::mem::zeroed();
            let connected = esp_idf_svc::sys::esp_netif_get_ip_info(netif, &mut ip_info) == 0
                && ip_info.ip.addr != 0;

            let mut diag = state.wifi_diag.write().unwrap();

            if connected {
                diag.connected = true;
                diag.ip_addr = format!(
                    "{}.{}.{}.{}",
                    ip_info.ip.addr & 0xFF,
                    (ip_info.ip.addr >> 8) & 0xFF,
                    (ip_info.ip.addr >> 16) & 0xFF,
                    (ip_info.ip.addr >> 24) & 0xFF,
                );
                diag.ssid = WIFI_SSID.to_string();

                let mut ap_info: esp_idf_svc::sys::wifi_ap_record_t = std::mem::zeroed();
                if esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut ap_info) == 0 {
                    diag.rssi = ap_info.rssi as i32;
                }
            } else {
                let was_connected = diag.connected;
                diag.connected = false;
                if was_connected {
                    diag.disconnect_count += 1;
                    warn!(
                        "WiFi disconnected (total: {}). Triggering reconnect...",
                        diag.disconnect_count
                    );
                }
                drop(diag);

                let ret = esp_idf_svc::sys::esp_wifi_connect();
                if ret != 0 {
                    warn!("esp_wifi_connect() returned {:#x}", ret);
                }
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

// =============================================
// HTTP SERVER HANDLERS
// =============================================
fn query_param<'a>(uri: &'a str, key: &str) -> Option<&'a str> {
    uri.split('?')
        .nth(1)?
        .split('&')
        .find_map(|kv| {
            let mut it = kv.splitn(2, '=');
            (it.next()? == key).then(|| it.next().unwrap_or(""))
        })
}

fn handle_logs(
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let text = {
        let g = LOG_RING.lock().unwrap();
        let mut s = String::with_capacity(g.len() * (LOG_LINE_MAX + 1));
        for l in g.iter() {
            s.push_str(l);
            s.push('\n');
        }
        s
    };
    let mut resp = req
        .into_response(
            200,
            None,
            &[
                ("Content-Type", "text/plain; charset=utf-8"),
                ("Cache-Control", "no-cache"),
            ],
        )
        .map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, text.as_bytes()).map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_status(
    state: &Arc<AppState>,
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let wifi_diag = state.wifi_diag.read().unwrap().clone();
    let json = serde_json::json!({
        "uptimeSecs": unsafe { esp_idf_svc::sys::esp_log_timestamp() } / 1000,
        "freeHeap": unsafe { esp_idf_svc::sys::esp_get_free_heap_size() },
        "minFreeHeap": unsafe { esp_idf_svc::sys::esp_get_minimum_free_heap_size() },
        "freePsram": unsafe { esp_idf_svc::sys::heap_caps_get_free_size(1 << 15) },
        "resetReason": unsafe { esp_idf_svc::sys::esp_reset_reason() } as i32,
        "hexJsonEntries": state.hex_json.read().unwrap().len(),
        "activeRpc": *state.active_rpc_idx.lock().unwrap(),
        "backfillInProgress": state.backfill_in_progress.load(Ordering::Relaxed),
        "timeSynced": state.time_synced.load(Ordering::Relaxed),
        "wifi": wifi_diag,
    });
    let mut resp = req
        .into_response(
            200,
            None,
            &[
                ("Content-Type", "application/json"),
                ("Cache-Control", "no-cache"),
            ],
        )
        .map_err(|_| esp_fail())?;
    let bytes = serde_json::to_vec(&json).map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, &bytes).map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_live_data(
    state: &Arc<AppState>,
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let data = state.live_data.read().unwrap().clone();
    let mut resp = req
        .into_response(
            200,
            None,
            &[
                ("Content-Type", "application/json"),
                ("Cache-Control", "no-cache"),
            ],
        )
        .map_err(|_| esp_fail())?;
    let json_bytes = serde_json::to_vec(&data).map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, &json_bytes).map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_miners(
    state: &Arc<AppState>,
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let data = state.miners.read().unwrap().clone();
    let mut resp = req
        .into_response(
            200,
            None,
            &[
                ("Content-Type", "application/json"),
                ("Cache-Control", "no-cache"),
            ],
        )
        .map_err(|_| esp_fail())?;
    let json_bytes = serde_json::to_vec(&data).map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, &json_bytes).map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_hex_json(
    state: &Arc<AppState>,
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let uri = req.uri().to_string();
    let from = query_param(&uri, "from").and_then(|v| v.parse::<u64>().ok());
    let limit = query_param(&uri, "limit").and_then(|v| v.parse::<usize>().ok());

    // Clone Arc and drop lock before serializing (avoids blocking writers)
    let data_snapshot = state.hex_json.read().unwrap().clone();
    let version = state.hex_json_version.load(Ordering::Relaxed);

    let full_request = from.is_none() && limit.is_none();
    let etag = if full_request {
        format!("\"hexjson-full-{}-{}\"", data_snapshot.len(), version)
    } else {
        format!(
            "\"hexjson-filter-{}-{}-{}-{}\"",
            from.unwrap_or(0),
            limit.unwrap_or(usize::MAX),
            data_snapshot.len(),
            version
        )
    };

    let if_none_match = req.header("If-None-Match").map(|s| s.to_string());
    if let Some(ref inm) = if_none_match {
        if inm == &etag || inm.trim_matches('"') == etag.trim_matches('"') {
            req.into_response(
                304,
                None,
                &[("ETag", etag.as_str()), ("Cache-Control", "no-cache")],
            )
            .map_err(|_| esp_fail())?;
            return Ok(());
        }
    }

    let mut resp = req
        .into_response(
            200,
            None,
            &[
                ("Content-Type", "application/json"),
                ("Cache-Control", "no-cache"),
                ("ETag", etag.as_str()),
            ],
        )
        .map_err(|_| esp_fail())?;

    if full_request {
        let json_bytes = serde_json::to_vec(&*data_snapshot).map_err(|_| esp_fail())?;
        EWrite::write_all(&mut resp, &json_bytes).map_err(|_| esp_fail())?;
    } else {
        let filtered: Vec<HexJsonEntry> = data_snapshot
            .iter()
            .filter(|e| e.current_day >= from.unwrap_or(0))
            .take(limit.unwrap_or(usize::MAX))
            .cloned()
            .collect();
        let json_bytes = serde_json::to_vec(&filtered).map_err(|_| esp_fail())?;
        EWrite::write_all(&mut resp, &json_bytes).map_err(|_| esp_fail())?;
    }
    Ok(())
}

fn handle_get_config(
    state: &Arc<AppState>,
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let data = state.config.read().unwrap().clone();
    let mut resp = req
        .into_response(200, None, &[("Content-Type", "application/json")])
        .map_err(|_| esp_fail())?;
    let json_bytes = serde_json::to_vec(&data).map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, &json_bytes).map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_post_config(
    state: &Arc<AppState>,
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let mut body = Vec::new();
    let mut buf = [0u8; 512];
    let mut rdr = req;
    loop {
        let n = ERead::read(&mut rdr, &mut buf).map_err(|_| esp_fail())?;
        if n == 0 { break; }
        body.extend_from_slice(&buf[..n]);
        if body.len() > 8192 { return Err(esp_fail()); }
    }
    let new_config: Config = serde_json::from_slice(&body).map_err(|_| esp_fail())?;
    let new_config = sanitize_config(new_config);
    *state.config.write().unwrap() = new_config.clone();
    state.config_event.notify();
    save_config_to_file(&new_config);
    let mut resp = rdr
        .into_response(200, None, &[("Content-Type", "application/json")])
        .map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, b"{\"status\":\"ok\"}").map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_add_miner(
    state: &Arc<AppState>,
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let mut body = Vec::new();
    let mut buf = [0u8; 512];
    let mut rdr = req;
    loop {
        let n = ERead::read(&mut rdr, &mut buf).map_err(|_| esp_fail())?;
        if n == 0 { break; }
        body.extend_from_slice(&buf[..n]);
        if body.len() > 8192 { return Err(esp_fail()); }
    }
    let r: AddMinerRequest = serde_json::from_slice(&body).map_err(|_| esp_fail())?;
    let (Some(start), Some(end)) = (parse_date(&r.start_date), parse_date(&r.end_date)) else {
        return Err(esp_fail());
    };
    if end < start || r.t_shares <= 0.0 || !r.t_shares.is_finite() {
        return Err(esp_fail());
    }
    let id = state.next_miner_id.fetch_add(1, Ordering::SeqCst) as u64;
    let miner = Miner {
        id: Some(id),
        start_date: r.start_date,
        end_date: r.end_date,
        t_shares: r.t_shares,
        status: None,
    };
    let mut miners = state.miners.write().unwrap();
    miners.push(miner);
    save_miners_to_file(&miners);
    drop(miners);
    let mut resp = rdr
        .into_response(200, None, &[("Content-Type", "application/json")])
        .map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, b"{\"status\":\"ok\"}").map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_end_miner(
    state: &Arc<AppState>,
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let mut body = Vec::new();
    let mut buf = [0u8; 512];
    let mut rdr = req;
    loop {
        let n = ERead::read(&mut rdr, &mut buf).map_err(|_| esp_fail())?;
        if n == 0 { break; }
        body.extend_from_slice(&buf[..n]);
        if body.len() > 8192 { return Err(esp_fail()); }
    }
    let r: MinerIdRequest = serde_json::from_slice(&body).map_err(|_| esp_fail())?;
    let mut miners = state.miners.write().unwrap();
    let Some(miner) = miners.iter_mut().find(|m| m.id == Some(r.id)) else {
        return Err(esp_fail());
    };
    miner.status = Some("completed".to_string());
    save_miners_to_file(&miners);
    drop(miners);
    let mut resp = rdr
        .into_response(200, None, &[("Content-Type", "application/json")])
        .map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, b"{\"status\":\"ok\"}").map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_delete_miner(
    state: &Arc<AppState>,
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) -> HResult {
    let mut body = Vec::new();
    let mut buf = [0u8; 512];
    let mut rdr = req;
    loop {
        let n = ERead::read(&mut rdr, &mut buf).map_err(|_| esp_fail())?;
        if n == 0 { break; }
        body.extend_from_slice(&buf[..n]);
        if body.len() > 8192 { return Err(esp_fail()); }
    }
    let r: MinerIdRequest = serde_json::from_slice(&body).map_err(|_| esp_fail())?;
    let mut miners = state.miners.write().unwrap();
    let before = miners.len();
    miners.retain(|m| m.id != Some(r.id));
    if miners.len() == before {
        return Err(esp_fail());
    }
    save_miners_to_file(&miners);
    drop(miners);
    let mut resp = rdr
        .into_response(200, None, &[("Content-Type", "application/json")])
        .map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, b"{\"status\":\"ok\"}").map_err(|_| esp_fail())?;
    Ok(())
}

// =============================================
// COMPRESSED & VERSIONED ASSET
// =============================================
struct CompressedAsset {
    data: Vec<u8>,
}

static COMPRESSED_ASSETS: OnceLock<HashMap<String, CompressedAsset>> = OnceLock::new();

fn init_compressed_assets() -> HashMap<String, CompressedAsset> {
    let mut map = HashMap::new();
    for name in Assets::iter() {
        if let Some(file) = Assets::get(&name) {
            let mime = get_mime_type(&name);
            if is_compressible_mime(mime) && file.data.len() > 128 {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
                if encoder.write_all(file.data.as_ref()).is_ok() {
                    if let Ok(compressed) = encoder.finish() {
                        if compressed.len() < file.data.len() {
                            map.insert(name.to_string(), CompressedAsset { data: compressed });
                        }
                    }
                }
            }
        }
    }
    info!("Pre-compressed {} assets for gzip serving.", map.len());
    map
}

fn get_compressed_assets() -> &'static HashMap<String, CompressedAsset> {
    COMPRESSED_ASSETS.get_or_init(init_compressed_assets)
}

fn serve_asset(
    req: esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
    path: &'static str,
) -> HResult {
    let file = Assets::get(path).ok_or_else(|| esp_fail())?;
    let mime = get_mime_type(path);
    let content_etag = format!("\"{}\"", fnv1a_hash(file.data.as_ref()));

    // 304 Not Modified
    if let Some(inm) = req.header("If-None-Match") {
        if inm == content_etag || inm.trim_matches('"') == content_etag.trim_matches('"') {
            req.into_response(
                304,
                None,
                &[
                    ("ETag", content_etag.as_str()),
                    ("Cache-Control", "public, max-age=86400"),
                ],
            )
            .map_err(|_| esp_fail())?;
            return Ok(());
        }
    }

    let accept_encoding = req.header("Accept-Encoding").unwrap_or("");
    let compressed = get_compressed_assets();
    if accept_encoding.contains("gzip") {
        if let Some(asset) = compressed.get(path) {
            let mut resp = req
                .into_response(
                    200,
                    None,
                    &[
                        ("Content-Type", mime),
                        ("Content-Encoding", "gzip"),
                        ("Cache-Control", "public, max-age=86400"),
                        ("ETag", content_etag.as_str()),
                        ("Vary", "Accept-Encoding"),
                    ],
                )
                .map_err(|_| esp_fail())?;
            EWrite::write_all(&mut resp, &asset.data).map_err(|_| esp_fail())?;
            return Ok(());
        }
    }

    // Serve uncompressed
    let mut resp = req
        .into_response(
            200,
            None,
            &[
                ("Content-Type", mime),
                ("Cache-Control", "public, max-age=86400"),
                ("ETag", content_etag.as_str()),
                ("Vary", "Accept-Encoding"),
            ],
        )
        .map_err(|_| esp_fail())?;
    EWrite::write_all(&mut resp, file.data.as_ref()).map_err(|_| esp_fail())?;
    Ok(())
}

// =============================================
// WIFI
// =============================================
fn connect_wifi() {
    use embedded_svc::wifi::ClientConfiguration as WifiClient;
    use esp_idf_svc::wifi::Configuration as WifiConfig;

    let peripherals = Peripherals::take().expect("peripherals");
    let sys_loop = EspSystemEventLoop::take().expect("event loop");
    let nvs = EspDefaultNvsPartition::take().expect("nvs");

    let wifi =
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs)).expect("EspWifi::new");
    let mut wifi = BlockingWifi::wrap(wifi, sys_loop).expect("BlockingWifi::wrap");

    let mut client_conf = WifiClient::default();
    client_conf.ssid = WIFI_SSID.try_into().expect("ssid fits");
    client_conf.password = WIFI_PASS.try_into().expect("pass fits");
    wifi.set_configuration(&WifiConfig::Client(client_conf))
        .expect("wifi config");

    let mut attempt = 0u32;
    let mut backoff = Duration::from_secs(2);

    loop {
        attempt += 1;
        info!("WiFi connection attempt {}...", attempt);

        if let Err(e) = wifi.start() {
            warn!("WiFi start failed: {}. Retrying in {:?}...", e, backoff);
            std::thread::sleep(backoff);
            backoff = std::cmp::min(backoff * 2, Duration::from_secs(60));
            continue;
        }

        info!("WiFi started, connecting to {}...", WIFI_SSID);
        if let Err(e) = wifi.connect() {
            warn!("WiFi connect failed: {}. Retrying in {:?}...", e, backoff);
            let _ = wifi.stop();
            std::thread::sleep(backoff);
            backoff = std::cmp::min(backoff * 2, Duration::from_secs(60));
            continue;
        }

        match wifi.wait_netif_up() {
            Ok(_) => {
                info!("WiFi connected, IP assigned.");
                std::mem::forget(wifi);
                return;
            }
            Err(e) => {
                warn!(
                    "WiFi wait_netif_up failed: {}. Retrying in {:?}...",
                    e, backoff
                );
                let _ = wifi.stop();
                std::thread::sleep(backoff);
                backoff = std::cmp::min(backoff * 2, Duration::from_secs(60));
            }
        }
    }
}

fn wait_for_time_sync() {
    for _ in 0..120 {
        if Utc::now().timestamp() > 1_700_000_000 {
            info!("SNTP time synced: {}", Utc::now());
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    warn!("Time not synced after 60s; continuing anyway.");
}

// =============================================
// MAIN
// =============================================
fn main() {
    log::set_logger(&RING_LOGGER).expect("logger");
    log::set_max_level(log::LevelFilter::Info);
    info!("⬢ HEX Stats (ESP32) booting ⬢");
    
    mount_spiffs();
    connect_wifi();
    let _sntp = EspSntp::new_default().expect("SNTP");
    wait_for_time_sync();
    
    let _ = get_compressed_assets();

    let initial_config = match fs::read_to_string(config_file_path()) {
        Ok(content) => serde_json::from_str(&content).unwrap_or(Config {
            live_data_frequency: 15,
            liquid_hex: 0.0,
            historical_start_day: 1260,
            backfill_delay_ms: default_backfill_delay(),
            backfill_save_interval: default_backfill_save_interval(),
        }),
        Err(_) => Config {
            live_data_frequency: 15,
            liquid_hex: 0.0,
            historical_start_day: 1260,
            backfill_delay_ms: default_backfill_delay(),
            backfill_save_interval: default_backfill_save_interval(),
        },
    };
    let initial_config = sanitize_config(initial_config);

    let raw_miners = match fs::read_to_string(miners_file_path()) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let (initial_miners, next_miner_id) = normalize_miners(raw_miners);

    let state = Arc::new(AppState {
        live_data: RwLock::new(LiveData::default()),
        hex_json: RwLock::new(Arc::new(Vec::new())),
        miners: RwLock::new(initial_miners),
        config: RwLock::new(initial_config),
        config_event: EventFlag::default(),
        active_rpc_idx: Mutex::new(0),
        hex_json_version: AtomicU32::new(1),
        next_miner_id: AtomicU32::new(next_miner_id as u32),
        wifi_diag: RwLock::new(WifiDiagnostics::default()),
        backfill_in_progress: AtomicBool::new(false),
        time_synced: AtomicBool::new(false),
    });

    // Spawn background tasks
    for (name, f) in [
        ("live-updater", live_data_updater as fn(Arc<AppState>)),
        ("hex-updater", hex_json_updater),
        ("rpc-health", rpc_health_checker),
        ("wifi-monitor", wifi_monitor),
        ("ntp-resync", ntp_resync_checker),
    ] {
        let st = state.clone();
        std::thread::Builder::new()
            .name(name.into())
            .stack_size(16 * 1024)
            .spawn(move || f(st))
            .expect("thread spawn");
    }

    let server_conf = ServerConfig {
        stack_size: 10 * 1024,
        max_uri_handlers: 32,
        ..Default::default()
    };
    let mut server = EspHttpServer::new(&server_conf).expect("http server");

    server.fn_handler("/api/logs", Method::Get, |req| handle_logs(req)).expect("route");
    server.fn_handler("/api/status", Method::Get, { let s = state.clone(); move |req| handle_status(&s, req) }).expect("route");
    server.fn_handler("/api/live-data", Method::Get, { let s = state.clone(); move |req| handle_live_data(&s, req) }).expect("route");
    server.fn_handler("/api/miners", Method::Get, { let s = state.clone(); move |req| handle_miners(&s, req) }).expect("route");
    server.fn_handler("/api/hexjson", Method::Get, { let s = state.clone(); move |req| handle_hex_json(&s, req) }).expect("route");
    server.fn_handler("/api/config", Method::Get, { let s = state.clone(); move |req| handle_get_config(&s, req) }).expect("route");
    server.fn_handler("/api/config", Method::Post, { let s = state.clone(); move |req| handle_post_config(&s, req) }).expect("route");
    server.fn_handler("/api/add-miner", Method::Post, { let s = state.clone(); move |req| handle_add_miner(&s, req) }).expect("route");
    server.fn_handler("/api/end-miner", Method::Post, { let s = state.clone(); move |req| handle_end_miner(&s, req) }).expect("route");
    server.fn_handler("/api/delete-miner", Method::Post, { let s = state.clone(); move |req| handle_delete_miner(&s, req) }).expect("route");

    // Static assets
    server
        .fn_handler("/", Method::Get, |req| serve_asset(req, "index.html"))
        .expect("route");
    for name in Assets::iter() {
        let path: &'static str = Box::leak(name.into_owned().into_boxed_str());
        let route = format!("/{}", path);
        let route: &'static str = Box::leak(route.into_boxed_str());
        server
            .fn_handler(route, Method::Get, move |req| serve_asset(req, path))
            .expect("route");
    }

    info!("⬢ HEX Stats server ready on http://hexstats.local:80 ⬢");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
