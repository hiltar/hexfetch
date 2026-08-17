use chrono::{Datelike, Days, NaiveTime, Utc};
use embedded_svc::http::client::Client as _;
use embedded_svc::http::Method;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::http::client::{Configuration as ClientConfig, EspHttpConnection as ClientConnection};
use esp_idf_svc::http::server::{Configuration as ServerConfig, EspHttpServer, EspHttpConnection as ServerConnection};
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::sys::{ESP_FAIL, EspError};
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{error, info, warn};
use rust_embed::RustEmbed;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

type HttpRequest<'a> = esp_idf_svc::http::server::Request<ServerConnection<'a>>;
type HResult = Result<(), EspError>;
type HttpClient = embedded_svc::http::client::Client<ClientConnection>;

// Helper to cleanly return ESP_FAIL from handlers
fn esp_fail() -> EspError {
    EspError::from(ESP_FAIL).unwrap()
}

// =============================================
// CONFIGURATION & CONSTANTS
// =============================================
const DATA_DIR: &str = "/spiffs";
const WIFI_SSID: &str = match option_env!("HEX_WIFI_SSID") { Some(s) => s, None => "MySSID" };
const WIFI_PASS: &str = match option_env!("HEX_WIFI_PASS") { Some(s) => s, None => "MyPassword" };

const RPC_ENDPOINTS: &[&str] = &[
    "https://rpc.pulsechain.com",
    "https://rpc-pulsechain.g4mm4.io",
    "https://pulsechain-rpc.publicnode.com",
    "https://rpc.pulsechainrpc.com",
];
const HEX_CONTRACT: &str = "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";
const DEXSCREENER_URL: &str = "https://api.dexscreener.com/latest/dex/tokens/0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";
const HEXDAILYSTATS_URL: &str = "https://hexdailystats.com/fulldatapulsechain";
const GLOBALS_SELECTOR: &str = "0xc3124525";
const DAILY_DATA_SELECTOR: &str = "0x90de6871";

const HEARTS_PER_HEX: f64 = 1e8;
const TSHARE_UNIT: f64 = 10000.0;
const BACKFILL_DELAY_MS: u64 = 60;
const BACKFILL_SAVE_INTERVAL: usize = 1000;
const DAILY_RECORD_SETTLE_DELAY_SECS: u64 = 120;
const ENABLE_HDS_BACKFILL: bool = true;
const MAX_HTTP_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

// =============================================
// CUSTOM DESERIALIZERS
// =============================================
fn f64_or_default<'de, D>(deserializer: D) -> Result<f64, D::Error>
where D: Deserializer<'de> {
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or(0.0))
}
fn u64_or_default<'de, D>(deserializer: D) -> Result<u64, D::Error>
where D: Deserializer<'de> {
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or(0))
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
    #[serde(rename = "price_Pulsechain")] pub price_pulsechain: f64,
    #[serde(rename = "tsharePrice_Pulsechain")] pub tshare_price_pulsechain: f64,
    #[serde(rename = "tshareRateHEX_Pulsechain")] pub tshare_rate_hex_pulsechain: f64,
    #[serde(rename = "penaltiesHEX_Pulsechain")] pub penalties_hex_pulsechain: f64,
    #[serde(rename = "payoutPerTshare_Pulsechain")] pub payout_per_tshare_pulsechain: f64,
    #[serde(rename = "beat")] pub beat: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Miner {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub id: Option<u64>,
    #[serde(rename = "startDate")] pub start_date: String,
    #[serde(rename = "endDate")] pub end_date: String,
    #[serde(rename = "tShares")] pub t_shares: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub status: Option<String>,
}

#[derive(Deserialize)]
struct AddMinerRequest {
    #[serde(rename = "startDate")] start_date: String,
    #[serde(rename = "endDate")] end_date: String,
    #[serde(rename = "tShares")] t_shares: f64,
}

#[derive(Deserialize)]
struct MinerIdRequest { id: u64 }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(rename = "liveDataFrequency")] pub live_data_frequency: u64,
    #[serde(rename = "liquidHEX")] pub liquid_hex: f64,
    #[serde(rename = "historicalStartDay")] pub historical_start_day: u64,
}

// =============================================
// APPLICATION STATE
// =============================================
#[derive(Default)]
struct EventFlag { pair: Mutex<bool>, cond: Condvar }
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
            if now >= deadline { break; }
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
}

// =============================================
// HELPERS
// =============================================
fn is_multiple_of(n: usize, divisor: usize) -> bool { divisor != 0 && n % divisor == 0 }

fn sanitize_config(config: Config) -> Config {
    Config {
        live_data_frequency: config.live_data_frequency.clamp(1, 24 * 60),
        liquid_hex: if config.liquid_hex.is_finite() && config.liquid_hex >= 0.0 { config.liquid_hex } else { 0.0 },
        historical_start_day: config.historical_start_day.max(1),
    }
}

fn normalize_miners(mut miners: Vec<Miner>) -> (Vec<Miner>, u64) {
    let mut next_id = miners.iter().filter_map(|m| m.id).max().unwrap_or(0) + 1;
    for miner in miners.iter_mut() {
        if miner.id.is_none() { miner.id = Some(next_id); next_id += 1; }
    }
    (miners, next_id)
}

fn calc_payout_per_tshare(payout_hearts: f64, shares: f64) -> f64 {
    if shares <= 0.0 || !shares.is_finite() || !payout_hearts.is_finite() { return 0.0; }
    (payout_hearts / shares) * TSHARE_UNIT
}

fn is_valid_daily_entry(entry: &HexJsonEntry) -> bool {
    entry.daily_payout_hex > 0.0 || entry.payout_per_tshare_hex > 0.0 || entry.tshare_rate_hex > 0.0
}

fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let date = chrono::NaiveDate::parse_from_str(s, "%d-%m-%Y").ok()?;
    let (y, m, d) = (date.year(), date.month(), date.day());
    if !(2000..=2100).contains(&y) { return None; }
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

// =============================================
// CUSTOM U256
// =============================================
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct U256([u64; 4]);
impl U256 {
    const ZERO: Self = Self([0; 4]);
    fn from_hex(s: &str) -> Self {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let mut limbs = [0u64; 4];
        if s.is_empty() { return Self::ZERO; }
        let mut limb_idx = 0; let mut shift = 0;
        for c in s.chars().rev() {
            let val = match c {
                '0'..='9' => c as u64 - '0' as u64,
                'a'..='f' => c as u64 - 'a' as u64 + 10,
                'A'..='F' => c as u64 - 'A' as u64 + 10,
                _ => continue,
            };
            if limb_idx < 4 { limbs[limb_idx] |= val << shift; }
            shift += 4;
            if shift == 64 { shift = 0; limb_idx += 1; }
        }
        Self(limbs)
    }
    fn to_f64(self) -> f64 {
        let mut result: f64 = 0.0;
        for (i, limb) in self.0.iter().enumerate() {
            if *limb != 0 { result += (*limb as f64) * (2.0_f64).powi(64 * i as i32); }
        }
        result
    }
}
impl std::ops::Sub<u64> for U256 {
    type Output = Self;
    fn sub(self, rhs: u64) -> Self {
        let mut limbs = self.0;
        let (res, overflow) = limbs[0].overflowing_sub(rhs);
        limbs[0] = res;
        let mut borrow = if overflow { 1 } else { 0 };
        for limb in limbs.iter_mut().skip(1) {
            let (res, overflow) = limb.overflowing_sub(borrow);
            *limb = res;
            borrow = if overflow { 1 } else { 0 };
        }
        Self(limbs)
    }
}

// =============================================
// UTC TIME CALCULATION
// =============================================
fn get_duration_until_next_1am_utc() -> Duration {
    let now = Utc::now();
    let today_1am = now.date_naive().and_time(NaiveTime::from_hms_opt(1, 0, 0).unwrap());
    let mut next_1am = today_1am.and_utc();
    if next_1am <= now { next_1am = next_1am.checked_add_days(Days::new(1)).unwrap(); }
    (next_1am - now).to_std().unwrap_or(Duration::from_secs(60))
}

// =============================================
// SPIFFS MOUNT
// =============================================
fn mount_spiffs() {
    let conf = esp_idf_svc::sys::esp_vfs_spiffs_conf_t {
        base_path: b"/spiffs\0".as_ptr() as *const _,
        partition_label: b"storage\0".as_ptr() as *const _,
        max_files: 4,
        format_if_mount_failed: false,
    };
    let ret = unsafe { esp_idf_svc::sys::esp_vfs_spiffs_register(&conf) };
    if ret != 0 { panic!("SPIFFS mount failed: {ret:#x}"); }
    info!("SPIFFS mounted at /spiffs");
}

// =============================================
// PERSISTENCE
// =============================================
fn hexjson_file_path() -> String { format!("{}/hexjson.json", DATA_DIR) }
fn config_file_path() -> String { format!("{}/config.json", DATA_DIR) }
fn miners_file_path() -> String { format!("{}/miners.json", DATA_DIR) }

fn save_hex_json_to_file(data: &[HexJsonEntry]) {
    let path = hexjson_file_path();
    match fs::File::create(&path) {
        Ok(f) => {
            let w = BufWriter::with_capacity(4096, f);
            if let Err(e) = serde_json::to_writer(w, data) { error!("Failed to serialize hexjson: {}", e); }
        }
        Err(e) => error!("Failed to save hexjson to {}: {}", path, e),
    }
}

fn load_hex_json_from_file() -> Vec<HexJsonEntry> {
    let path = hexjson_file_path();
    match fs::File::open(&path) {
        Ok(f) => match serde_json::from_reader::<_, Vec<HexJsonEntry>>(BufReader::with_capacity(4096, f)) {
            Ok(data) => { info!("Loaded {} hexjson entries from file", data.len()); data }
            Err(e) => { warn!("Failed to parse hexjson file: {}. Starting fresh.", e); Vec::new() }
        },
        Err(_) => { info!("No hexjson file found. Will build from RPC + external sources."); Vec::new() }
    }
}

fn save_config_to_file(config: &Config) {
    match fs::write(config_file_path(), serde_json::to_string_pretty(config).unwrap_or_default()) {
        Ok(_) => {} Err(e) => error!("Failed to save config: {}", e),
    }
}

fn save_miners_to_file(miners: &[Miner]) {
    match fs::write(miners_file_path(), serde_json::to_string_pretty(miners).unwrap_or_default()) {
        Ok(_) => {} Err(e) => error!("Failed to save miners: {}", e),
    }
}

// =============================================
// HTTPS CLIENT
// =============================================
fn make_client() -> Result<HttpClient, String> {
    let cfg = ClientConfig {
        buffer_size: Some(2048),
        buffer_size_tx: Some(2048),
        timeout: Some(Duration::from_secs(20)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let conn = ClientConnection::new(&cfg).map_err(|e| format!("client: {}", e))?;
    Ok(HttpClient::wrap(conn))
}

fn http_request(
    client: &mut HttpClient,
    post_body: Option<&[u8]>,
    url: &str,
) -> Result<(u16, Vec<u8>), String> {
    let headers: &[(&str, &str)] = if post_body.is_some() {
        &[("Content-Type", "application/json")]
    } else {
        &[]
    };
    
    let mut req = client.request(
        if post_body.is_some() { Method::Post } else { Method::Get },
        url,
        headers
    ).map_err(|e| format!("request to {} failed: {}", url, e))?;

    if let Some(body) = post_body {
        req.write_all(body).map_err(|e| e.to_string())?;
    }
    
    let mut resp = req.submit().map_err(|e| e.to_string())?;
    let status = resp.status();
    let mut buf = Vec::new();
    resp.take(MAX_HTTP_RESPONSE_BYTES as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read: {}", e))?;
    Ok((status as u16, buf))
}

// =============================================
// RPC LOGIC
// =============================================
fn call_rpc(
    client: &mut HttpClient,
    state: &Arc<AppState>,
    method: &str,
    params: serde_json::Value,
) -> Result<String, String> {
    let req_body = serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": 1 });
    let body_bytes = serde_json::to_vec(&req_body).map_err(|e| e.to_string())?;

    let start_idx = *state.active_rpc_idx.lock().unwrap();
    let mut last_err = String::new();
    for i in 0..RPC_ENDPOINTS.len() {
        let idx = (start_idx + i) % RPC_ENDPOINTS.len();
        let url = RPC_ENDPOINTS[idx];
        match http_request(client, Some(&body_bytes), url) {
            Ok((status, body)) => {
                if !(200..300).contains(&status) { last_err = format!("HTTP {} on {}", status, url); continue; }
                match serde_json::from_slice::<serde_json::Value>(&body) {
                    Ok(json) => {
                        if let Some(err) = json.get("error") {
                            last_err = format!("RPC error on {}: {}", url, err["message"].as_str().unwrap_or("unknown"));
                            continue;
                        }
                        if idx != start_idx {
                            info!("Successfully connected to fallback RPC: {} (index {})", url, idx);
                            *state.active_rpc_idx.lock().unwrap() = idx;
                        }
                        return Ok(json["result"].as_str().unwrap_or("").to_string());
                    }
                    Err(e) => { last_err = format!("JSON parse error on {}: {}", url, e); continue; }
                }
            }
            Err(e) => { last_err = e; continue; }
        }
    }
    Err(format!("All RPC endpoints failed. Last error: {}", last_err))
}

// =============================================
// ON-CHAIN DATA READING
// =============================================
fn read_globals(client: &mut HttpClient, state: &Arc<AppState>) -> Result<(f64, u64, f64), String> {
    let params = serde_json::json!([{ "to": HEX_CONTRACT, "data": GLOBALS_SELECTOR }, "latest"]);
    let hex_result = call_rpc(client, state, "eth_call", params)?;
    let g_str = hex_result.strip_prefix("0x").unwrap_or(&hex_result);
    if g_str.len() < 320 { return Err(format!("globals() too short ({} chars)", g_str.len())); }
    let share_rate_raw = U256::from_hex(&g_str[128..192]);
    let penalty_raw = U256::from_hex(&g_str[192..256]);
    let daily_data_count = U256::from_hex(&g_str[256..320]);
    let tshare_rate = if share_rate_raw > U256::ZERO { share_rate_raw.to_f64() / 10.0 } else { 0.0 };
    let penalties = penalty_raw.to_f64() / 1e8;
    let day_count = daily_data_count.to_f64() as u64;
    Ok((tshare_rate, day_count, penalties))
}

fn read_daily_data(client: &mut HttpClient, state: &Arc<AppState>, day: u64) -> Result<(f64, f64), String> {
    let call_data = format!("{}{:064x}", DAILY_DATA_SELECTOR, day);
    let params = serde_json::json!([{ "to": HEX_CONTRACT, "data": call_data }, "latest"]);
    let hex_result = call_rpc(client, state, "eth_call", params)?;
    let d_str = hex_result.strip_prefix("0x").unwrap_or(&hex_result);
    if d_str.len() < 128 { return Err(format!("dailyData({}) too short ({} chars)", day, d_str.len())); }
    let day_payout = U256::from_hex(&d_str[0..64]);
    let day_shares = U256::from_hex(&d_str[64..128]);
    Ok((day_payout.to_f64(), day_shares.to_f64()))
}

// =============================================
// EXTERNAL DATA SOURCES
// =============================================
fn fetch_price_dexscreener(client: &mut HttpClient) -> Result<f64, String> {
    let (status, body) = http_request(client, None, DEXSCREENER_URL)?;
    if !(200..300).contains(&status) { return Err(format!("DEXScreener HTTP {}", status)); }
    let resp: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("parse: {}", e))?;
    let price = resp.get("pairs")
        .and_then(|p| p.as_array())
        .and_then(|p| p.first())
        .and_then(|pair| pair.get("priceUsd"))
        .and_then(|price| price.as_f64().or_else(|| price.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0.0);
    if price <= 0.0 || !price.is_finite() { return Err("DEXScreener returned zero or invalid price".to_string()); }
    Ok(price)
}

fn fetch_hexdailystats_backfill(client: &mut HttpClient) -> (HashMap<u64, f64>, HashMap<u64, f64>) {
    if !ENABLE_HDS_BACKFILL { return (HashMap::new(), HashMap::new()); }
    info!("Attempting historical fetch from HEXDailyStats...");
    let (status, body) = match http_request(client, None, HEXDAILYSTATS_URL) {
        Ok(r) => r,
        Err(e) => { warn!("HEXDailyStats unreachable: {}", e); return (HashMap::new(), HashMap::new()); }
    };
    if !(200..300).contains(&status) { return (HashMap::new(), HashMap::new()); }
    let entries: Vec<HexJsonEntry> = match serde_json::from_reader(BufReader::with_capacity(4096, &body[..])) {
        Ok(e) => e,
        Err(e) => { warn!("HEXDailyStats JSON parse failed: {}", e); return (HashMap::new(), HashMap::new()); }
    };
    let mut tshare_map: HashMap<u64, f64> = HashMap::with_capacity(entries.len());
    let mut price_map: HashMap<u64, f64> = HashMap::with_capacity(entries.len());
    for entry in &entries {
        if entry.tshare_rate_hex > 0.0 && entry.tshare_rate_hex.is_finite() { tshare_map.insert(entry.current_day, entry.tshare_rate_hex); }
        if entry.price_pulse_x > 0.0 && entry.price_pulse_x.is_finite() { price_map.insert(entry.current_day, entry.price_pulse_x); }
    }
    (tshare_map, price_map)
}

// =============================================
// DAILY ENTRY STORAGE
// =============================================
fn store_daily_entry(state: &Arc<AppState>, entry: HexJsonEntry) -> bool {
    let mut hex_json = state.hex_json.write().unwrap();
    if hex_json.iter().any(|e| e.current_day == entry.current_day) { return false; }
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
// BACKFILL
// =============================================
fn backfill_hex_json(
    client: &mut HttpClient,
    state: &Arc<AppState>,
    existing_data: &[HexJsonEntry],
) -> Vec<HexJsonEntry> {
    info!("Starting HEXJSON backfill...");
    let (current_tshare_rate, day_count, _penalties) = match read_globals(client, state) {
        Ok(v) => v,
        Err(e) => { error!("Backfill failed: cannot read globals(): {}", e); return existing_data.to_vec(); }
    };
    if day_count == 0 { return existing_data.to_vec(); }

    let (hds_tshares, hds_prices) = fetch_hexdailystats_backfill(client);
    let current_price = fetch_price_dexscreener(client).unwrap_or(0.0);

    let mut by_day: HashMap<u64, HexJsonEntry> = HashMap::new();
    for entry in existing_data { by_day.insert(entry.current_day, entry.clone()); }

    let missing_days: Vec<u64> = (1..day_count).filter(|day| !by_day.contains_key(day)).collect();
    if missing_days.is_empty() {
        let mut result: Vec<HexJsonEntry> = by_day.into_values().collect();
        result.sort_by_key(|e| e.current_day);
        return result;
    }

    let total_to_fetch = missing_days.len();
    info!("Backfilling {} missing days up to day {} (sequential)...", total_to_fetch, day_count - 1);

    let mut fetched = 0usize;
    let mut consecutive_errors = 0u32;

    for day in missing_days {
        let result = read_daily_data(client, state, day);
        if BACKFILL_DELAY_MS > 0 { std::thread::sleep(Duration::from_millis(BACKFILL_DELAY_MS)); }
        match result {
            Ok((payout_hearts, shares)) => {
                consecutive_errors = 0;
                let daily_payout_hex = payout_hearts / HEARTS_PER_HEX;
                let payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);
                let price = hds_prices.get(&day).copied().filter(|p| p.is_finite() && *p > 0.0).unwrap_or(current_price);
                let tshare = hds_tshares.get(&day).copied().filter(|t| t.is_finite() && *t > 0.0).unwrap_or(current_tshare_rate);
                by_day.insert(day, HexJsonEntry {
                    current_day: day, tshare_rate_hex: tshare, daily_payout_hex, payout_per_tshare_hex: payout_per_tshare, price_pulse_x: price,
                });
            }
            Err(_) => {
                consecutive_errors += 1;
                if consecutive_errors >= 50 { break; }
            }
        }
        fetched += 1;
        if is_multiple_of(fetched, 100) { info!("Backfill progress: {}/{} days", fetched, total_to_fetch); }
        if is_multiple_of(fetched, BACKFILL_SAVE_INTERVAL) {
            let mut partial: Vec<HexJsonEntry> = by_day.values().cloned().collect();
            partial.sort_by_key(|e| e.current_day);
            save_hex_json_to_file(&partial);
        }
    }

    let mut result: Vec<HexJsonEntry> = by_day.into_values().collect();
    result.sort_by_key(|e| e.current_day);
    result
}

// =============================================
// DAILY RECORDING / LIVE DATA
// =============================================
fn record_daily_entry(client: &mut HttpClient, state: &Arc<AppState>) -> Result<HexJsonEntry, String> {
    let (tshare_rate, day_count, _) = read_globals(client, state)?;
    if day_count == 0 { return Err("dailyDataCount is 0".to_string()); }
    let target_day = day_count - 1;
    let (payout_hearts, shares) = read_daily_data(client, state, target_day)?;
    let daily_payout_hex = payout_hearts / HEARTS_PER_HEX;
    let payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);
    let price = fetch_price_dexscreener(client).unwrap_or_else(|_| state.live_data.read().unwrap().price_pulsechain);
    Ok(HexJsonEntry {
        current_day: target_day, tshare_rate_hex: tshare_rate, daily_payout_hex, payout_per_tshare_hex: payout_per_tshare, price_pulse_x: price,
    })
}

fn fetch_live_data(client: &mut HttpClient, state: &Arc<AppState>) -> Result<LiveData, String> {
    let (status, body) = http_request(client, None, DEXSCREENER_URL)?;
    if !(200..300).contains(&status) { return Err(format!("DEXScreener HTTP {}", status)); }
    let dex_resp: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let price = dex_resp.get("pairs")
        .and_then(|p| p.as_array()).and_then(|p| p.first()).and_then(|pair| pair.get("priceUsd"))
        .and_then(|price| price.as_f64().or_else(|| price.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0.0);

    let gas_price_hex = call_rpc(client, state, "eth_gasPrice", serde_json::json!([]))?;
    let beat = U256::from_hex(&gas_price_hex).to_f64() / 1e9;
    let (tshare_rate, daily_data_count, penalties) = read_globals(client, state)?;

    let mut payout_per_tshare = 0.0;
    if daily_data_count > 0 {
        if let Ok((payout_hearts, shares)) = read_daily_data(client, state, daily_data_count - 1) {
            payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);
        }
    }
    Ok(LiveData {
        price_pulsechain: price, tshare_price_pulsechain: tshare_rate * price, tshare_rate_hex_pulsechain: tshare_rate,
        penalties_hex_pulsechain: penalties, payout_per_tshare_pulsechain: payout_per_tshare, beat,
    })
}

fn fetch_live_data_with_retry(client: &mut HttpClient, state: &Arc<AppState>) -> Result<LiveData, String> {
    let mut delay = Duration::from_secs(1);
    let start = Instant::now();
    loop {
        match fetch_live_data(client, state) {
            Ok(data) => return Ok(data),
            Err(e) => {
                if start.elapsed() > Duration::from_secs(2 * 60) { return Err(e); }
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
    let mut client = make_client().unwrap();
    if let Ok(data) = fetch_live_data_with_retry(&mut client, &state) { *state.live_data.write().unwrap() = data; }
    loop {
        let freq = sanitize_config(state.config.read().unwrap().clone()).live_data_frequency;
        state.config_event.wait_timeout(Duration::from_secs(freq * 60));
        if let Ok(data) = fetch_live_data_with_retry(&mut client, &state) { *state.live_data.write().unwrap() = data; }
    }
}

fn hex_json_updater(state: Arc<AppState>) {
    let mut client = make_client().unwrap();
    let file_data = load_hex_json_from_file();
    let initial_data = if file_data.is_empty() {
        let backfilled = backfill_hex_json(&mut client, &state, &[]);
        save_hex_json_to_file(&backfilled);
        backfilled
    } else {
        match read_globals(&mut client, &state) {
            Ok((_, day_count, _)) => {
                let known_days: HashSet<u64> = file_data.iter().map(|e| e.current_day).collect();
                if (1..day_count).any(|day| !known_days.contains(&day)) {
                    let updated = backfill_hex_json(&mut client, &state, &file_data);
                    save_hex_json_to_file(&updated);
                    updated
                } else { file_data }
            }
            Err(_) => file_data
        }
    };
    {
        let mut hex_json = state.hex_json.write().unwrap();
        *hex_json = Arc::new(initial_data);
        state.hex_json_version.fetch_add(1, Ordering::Relaxed);
    }

    loop {
        std::thread::sleep(get_duration_until_next_1am_utc());
        std::thread::sleep(Duration::from_secs(DAILY_RECORD_SETTLE_DELAY_SECS));
        let mut delay = Duration::from_secs(5);
        for _ in 0..10 {
            if let Ok(entry) = record_daily_entry(&mut client, &state) {
                if is_valid_daily_entry(&entry) { store_daily_entry(&state, entry); break; }
            }
            std::thread::sleep(delay);
            delay = std::cmp::min(delay * 2, Duration::from_secs(120));
        }
    }
}

fn test_rpc(client: &mut HttpClient, url: &str) -> bool {
    let block_req = serde_json::json!({ "jsonrpc": "2.0", "method": "eth_blockNumber", "params": [], "id": 1 });
    let body = serde_json::to_vec(&block_req).unwrap_or_default();
    let basic_ok = match http_request(client, Some(&body), url) {
        Ok((s, b)) if (200..300).contains(&s) => serde_json::from_slice::<serde_json::Value>(&b)
            .map(|j| j.get("error").is_none() && j.get("result").is_some()).unwrap_or(false),
        _ => false,
    };
    if !basic_ok { return false; }
    let call_req = serde_json::json!({
        "jsonrpc": "2.0", "method": "eth_call",
        "params": [{ "to": HEX_CONTRACT, "data": GLOBALS_SELECTOR }, "latest"], "id": 2
    });
    let body = serde_json::to_vec(&call_req).unwrap_or_default();
    match http_request(client, Some(&body), url) {
        Ok((s, b)) if (200..300).contains(&s) => serde_json::from_slice::<serde_json::Value>(&b)
            .map(|j| j.get("error").is_none() && j.get("result").is_some()).unwrap_or(false),
        _ => false,
    }
}

fn rpc_health_checker(state: Arc<AppState>) {
    let mut client = make_client().unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(24 * 60 * 60));
        let current_idx = *state.active_rpc_idx.lock().unwrap();
        if current_idx != 0 && test_rpc(&mut client, RPC_ENDPOINTS[0]) {
            *state.active_rpc_idx.lock().unwrap() = 0;
        }
    }
}

fn heap_monitor() {
    loop {
        std::thread::sleep(Duration::from_secs(300));
        let free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
        info!("HEAP: free={} B", free);
    }
}

// =============================================
// HTTP SERVER HANDLERS
// =============================================
fn query_param<'a>(uri: &'a str, key: &str) -> Option<&'a str> {
    uri.split('?').nth(1)?.split('&')
        .find_map(|kv| { let mut it = kv.splitn(2, '='); (it.next()? == key).then(|| it.next().unwrap_or("")) })
}

fn handle_live_data(state: &Arc<AppState>, req: HttpRequest) -> HResult {
    let data = state.live_data.read().unwrap().clone();
    let mut resp = req.into_response(200, None, &[("Content-Type", "application/json"), ("Cache-Control", "no-cache")])
        .map_err(|_| esp_fail())?;
    serde_json::to_writer(&mut resp, &data).map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_miners(state: &Arc<AppState>, req: HttpRequest) -> HResult {
    let data = state.miners.read().unwrap().clone();
    let mut resp = req.into_response(200, None, &[("Content-Type", "application/json"), ("Cache-Control", "no-cache")])
        .map_err(|_| esp_fail())?;
    serde_json::to_writer(&mut resp, &data).map_err(|_| esp_fail())?;
    Ok(())
}

fn handle_hex_json(state: &Arc<AppState>, req: HttpRequest) -> HResult {
    let uri = req.uri().to_string();
    let from = query_param(&uri, "from").and_then(|v| v.parse::<u64>().ok());
    let limit = query_param(&uri, "limit").and_then(|v| v.parse::<usize>().ok());

    let data = state.hex_json.read().unwrap().clone();
    let version = state.hex_json_version.load(Ordering::Relaxed);
    let full_request = from.is_none() && limit.is_none();
    let etag = if full_request {
        format!("\"hexjson-full-{}-{}\"", data.len(), version)
    } else {
        format!("\"hexjson-filter-{}-{}-{}-{}\"", from.unwrap_or(0), limit.unwrap_or(usize::MAX), data.len(), version)
    };

    let if_none_match = req.header("If-None-Match").map(|s| s.to_string());
    if let Some(inm) = if_none_match {
        if inm == etag || inm.trim_matches('"') == etag.trim_matches('"') {
            let etag_str = etag.as_str();
            req.into_response(304, None, &[("ETag", etag_str), ("Cache-Control", "no-cache")])
                .map_err(|_| esp_fail())?;
            return Ok(());
        }
    }

    let mut resp = req.into_response(200, None, &[
        ("Content-Type", "application/json"), ("Cache-Control", "no-cache"), ("ETag", etag.as_str()),
    ]).map_err(|_| esp_fail())?;

    if full_request {
        serde_json::to_writer(&mut resp, &*data).map_err(|_| esp_fail())?;
    } else {
        let filtered: Vec<HexJsonEntry> = data.iter().filter(|e| e.current_day >= from.unwrap_or(0)).take(limit.unwrap_or(usize::MAX)).cloned().collect();
        serde_json::to_writer(&mut resp, &filtered).map_err(|_| esp_fail())?;
    }
    Ok(())
}

fn handle_get_config(state: &Arc<AppState>, req: HttpRequest) -> HResult {
    let data = state.config.read().unwrap().clone();
    let mut resp = req.into_response(200, None, &[("Content-Type", "application/json")]).map_err(|_| esp_fail())?;
    serde_json::to_writer(&mut resp, &data).map_err(|_| esp_fail())?;
    Ok(())
}

fn read_body(mut req: HttpRequest) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut req, &mut body).map_err(|e| e.to_string())?;
    Ok(body)
}

fn handle_post_config(state: &Arc<AppState>, req: HttpRequest) -> HResult {
    let body = read_body(req).map_err(|_| esp_fail())?;
    let new_config: Config = serde_json::from_slice(&body).map_err(|_| esp_fail())?;
    let new_config = sanitize_config(new_config);
    *state.config.write().unwrap() = new_config.clone();
    state.config_event.notify();
    save_config_to_file(&new_config);
    Ok(())
}

fn handle_add_miner(state: &Arc<AppState>, req: HttpRequest) -> HResult {
    let body = read_body(req).map_err(|_| esp_fail())?;
    let r: AddMinerRequest = serde_json::from_slice(&body).map_err(|_| esp_fail())?;
    let (Some(start), Some(end)) = (parse_date(&r.start_date), parse_date(&r.end_date)) else { return Err(esp_fail()); };
    if end < start || r.t_shares <= 0.0 { return Err(esp_fail()); }
    let id = state.next_miner_id.fetch_add(1, Ordering::SeqCst) as u64;
    let miner = Miner { id: Some(id), start_date: r.start_date, end_date: r.end_date, t_shares: r.t_shares, status: None };
    let mut miners = state.miners.write().unwrap();
    miners.push(miner);
    save_miners_to_file(&miners);
    Ok(())
}

fn handle_end_miner(state: &Arc<AppState>, req: HttpRequest) -> HResult {
    let body = read_body(req).map_err(|_| esp_fail())?;
    let r: MinerIdRequest = serde_json::from_slice(&body).map_err(|_| esp_fail())?;
    let mut miners = state.miners.write().unwrap();
    let Some(miner) = miners.iter_mut().find(|m| m.id == Some(r.id)) else { return Err(esp_fail()); };
    miner.status = Some("completed".to_string());
    save_miners_to_file(&miners);
    Ok(())
}

fn handle_delete_miner(state: &Arc<AppState>, req: HttpRequest) -> HResult {
    let body = read_body(req).map_err(|_| esp_fail())?;
    let r: MinerIdRequest = serde_json::from_slice(&body).map_err(|_| esp_fail())?;
    let mut miners = state.miners.write().unwrap();
    let before = miners.len();
    miners.retain(|m| m.id != Some(r.id));
    if miners.len() == before { return Err(esp_fail()); }
    save_miners_to_file(&miners);
    Ok(())
}

fn serve_asset(req: HttpRequest, path: &'static str) -> HResult {
    let file = Assets::get(path).ok_or_else(|| esp_fail())?;
    let mime = get_mime_type(path);
    let mut resp = req.into_response(200, None, &[
        ("Content-Type", mime), ("Cache-Control", "public, max-age=3600"),
    ]).map_err(|_| esp_fail())?;
    std::io::Write::write_all(&mut resp, file.data.as_ref()).map_err(|_| esp_fail())?;
    Ok(())
}

// =============================================
// BOOT: WIFI + SNTP
// =============================================
fn connect_wifi() -> Result<(), String> {
    use embedded_svc::wifi::ClientConfiguration as WifiClient;
    use esp_idf_svc::wifi::Configuration as WifiConfig;

    let peripherals = Peripherals::take().map_err(|e| e.to_string())?;
    let sys_loop = EspSystemEventLoop::take().map_err(|e| e.to_string())?;
    let nvs = EspDefaultNvsPartition::take().map_err(|e| e.to_string())?;

    let wifi = EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone())).map_err(|e| e.to_string())?;
    let mut wifi = BlockingWifi::wrap(wifi, sys_loop).map_err(|e| e.to_string())?;

    let mut client_conf = WifiClient::default();
    client_conf.ssid = WIFI_SSID.try_into().map_err(|_| "ssid too long".to_string())?;
    client_conf.password = WIFI_PASS.try_into().map_err(|_| "pass too long".to_string())?;
    wifi.set_configuration(&WifiConfig::Client(client_conf)).map_err(|e| e.to_string())?;

    wifi.start().map_err(|e| e.to_string())?;
    info!("WiFi started, connecting to {}...", WIFI_SSID);
    wifi.connect().map_err(|e| e.to_string())?;
    wifi.wait_netif_up().map_err(|e| e.to_string())?;
    info!("WiFi connected, IP assigned.");
    Ok(())
}

fn wait_for_time_sync() {
    for _ in 0..120 {
        if Utc::now().timestamp() > 1_700_000_000 {
            info!("SNTP time synced: {}", Utc::now());
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    warn!("Time not synced after 60 s; continuing anyway.");
}

// =============================================
// MAIN
// =============================================
fn main() {
    EspLogger::initialize_default();
    info!("⬢ HEX Stats (ESP32) booting ⬢");

    mount_spiffs();
    connect_wifi().expect("WiFi must connect");

    let _sntp = EspSntp::new_default().expect("SNTP");
    wait_for_time_sync();

    let initial_config = match fs::read_to_string(config_file_path()) {
        Ok(content) => serde_json::from_str(&content).unwrap_or(Config { live_data_frequency: 15, liquid_hex: 0.0, historical_start_day: 1260 }),
        Err(_) => Config { live_data_frequency: 15, liquid_hex: 0.0, historical_start_day: 1260 },
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
    });

    for (name, f) in [
        ("live-updater", live_data_updater as fn(Arc<AppState>)),
        ("hex-updater", hex_json_updater),
        ("rpc-health", rpc_health_checker),
        ("heap-mon", heap_monitor_wrap),
    ] {
        let st = state.clone();
        std::thread::Builder::new().name(name.into()).stack_size(16 * 1024).spawn(move || f(st)).expect("thread spawn");
    }

    let server_conf = ServerConfig { stack_size: 10 * 1024, max_uri_handlers: 32, ..Default::default() };
    let mut server = EspHttpServer::new(&server_conf).expect("http server");

    server.fn_handler("/api/live-data", Method::Get, move |req| handle_live_data(&state, req)).expect("route");
    server.fn_handler("/api/miners", Method::Get, move |req| handle_miners(&state, req)).expect("route");
    server.fn_handler("/api/hexjson", Method::Get, move |req| handle_hex_json(&state, req)).expect("route");
    server.fn_handler("/api/config", Method::Get, move |req| handle_get_config(&state, req)).expect("route");
    server.fn_handler("/api/config", Method::Post, move |req| handle_post_config(&state, req)).expect("route");
    server.fn_handler("/api/add-miner", Method::Post, move |req| handle_add_miner(&state, req)).expect("route");
    server.fn_handler("/api/end-miner", Method::Post, move |req| handle_end_miner(&state, req)).expect("route");
    server.fn_handler("/api/delete-miner", Method::Post, move |req| handle_delete_miner(&state, req)).expect("route");

    server.fn_handler("/", Method::Get, |req| serve_asset(req, "index.html")).expect("route");
    for name in Assets::iter() {
        let path: &'static str = Box::leak(name.into_owned().into_boxed_str());
        let route = format!("/{}", path);
        let route: &'static str = Box::leak(route.into_boxed_str());
        server.fn_handler(route, Method::Get, move |req| serve_asset(req, path)).expect("route");
    }

    info!("⬢ HEX Stats server ready on http://hexstats.local:80 ⬢");
    loop { std::thread::sleep(Duration::from_secs(3600)); }
}

fn heap_monitor_wrap(state: Arc<AppState>) {
    let _ = state;
    heap_monitor();
}
