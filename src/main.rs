use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Datelike, Days, NaiveDate, NaiveTime, Utc};
use futures::stream::{self, StreamExt};
use reqwest::Client;
use rust_embed::RustEmbed;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock};
use tower_http::compression::CompressionLayer;
use tracing::{error, info, warn};

// =============================================
// CONFIGURATION & CONSTANTS
// =============================================

const DATA_DIR: &str = "/opt/hexfetch";
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
const BACKFILL_DELAY_MS: u64 = 60;
const BACKFILL_SAVE_INTERVAL: usize = 1000;
const BACKFILL_CONCURRENCY: usize = 6;
const DAILY_RECORD_SETTLE_DELAY_SECS: u64 = 120;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

// =============================================
// CUSTOM DESERIALIZERS
// =============================================

fn f64_or_default<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or(0.0))
}

fn u64_or_default<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
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
}

#[derive(Deserialize)]
struct HexJsonQuery {
    from: Option<u64>,
    limit: Option<usize>,
}

// =============================================
// APPLICATION STATE
// =============================================

struct AppState {
    live_data: RwLock<LiveData>,
    hex_json: RwLock<Arc<Vec<HexJsonEntry>>>,
    miners: RwLock<Vec<Miner>>,
    config: RwLock<Config>,
    config_tx: broadcast::Sender<()>,
    active_rpc_idx: RwLock<usize>,
    hex_json_version: AtomicU64,
    next_miner_id: AtomicU64,
}

// =============================================
// HELPERS
// =============================================

fn is_multiple_of(n: usize, divisor: usize) -> bool {
    divisor != 0 && n % divisor == 0
}

fn sanitize_config(config: Config) -> Config {
    Config {
        live_data_frequency: config.live_data_frequency.clamp(1, 24 * 60),
        liquid_hex: if config.liquid_hex.is_finite() && config.liquid_hex >= 0.0 {
            config.liquid_hex
        } else {
            0.0
        },
        historical_start_day: config.historical_start_day.max(1),
    }
}

fn normalize_miners(mut miners: Vec<Miner>) -> (Vec<Miner>, u64) {
    let mut next_id = miners
        .iter()
        .filter_map(|m| m.id)
        .max()
        .unwrap_or(0)
        + 1;

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

// =============================================
// DATE PARSER
// =============================================

fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let date = NaiveDate::parse_from_str(s, "%d-%m-%Y").ok()?;

    let y = date.year();
    let m = date.month();
    let d = date.day();

    if !(2000..=2100).contains(&y) {
        return None;
    }

    Some((y, m, d))
}

// =============================================
// MIME GUESSER
// =============================================

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

impl std::fmt::LowerHex for U256 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for i in (0..4).rev() {
            write!(f, "{:016x}", self.0[i])?;
        }
        Ok(())
    }
}

// =============================================
// UTC TIME CALCULATION
// =============================================

fn get_duration_until_next_1am_utc() -> std::time::Duration {
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
        .unwrap_or(std::time::Duration::from_secs(60))
}

// =============================================
// FILE PATHS / PERSISTENCE
// =============================================

fn hexjson_file_path() -> String {
    format!("{}/hexjson.json", DATA_DIR)
}

fn config_file_path() -> String {
    format!("{}/config.json", DATA_DIR)
}

fn miners_file_path() -> String {
    format!("{}/miners.json", DATA_DIR)
}

async fn save_hex_json_to_file(data: &[HexJsonEntry]) {
    let path = hexjson_file_path();

    match serde_json::to_string(data) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(&path, json).await {
                error!("Failed to save hexjson to {}: {}", path, e);
            }
        }
        Err(e) => error!("Failed to serialize hexjson: {}", e),
    }
}

async fn load_hex_json_from_file() -> Vec<HexJsonEntry> {
    let path = hexjson_file_path();

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => match serde_json::from_str::<Vec<HexJsonEntry>>(&content) {
            Ok(data) => {
                info!("Loaded {} hexjson entries from file", data.len());
                data
            }
            Err(e) => {
                warn!("Failed to parse hexjson file: {}. Starting fresh.", e);
                Vec::new()
            }
        },
        Err(_) => {
            info!("No hexjson file found. Will build from RPC + external sources.");
            Vec::new()
        }
    }
}

async fn save_config_to_file(config: &Config) {
    let path = config_file_path();

    match serde_json::to_string_pretty(config) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(&path, json).await {
                error!("Failed to save config to {}: {}", path, e);
            }
        }
        Err(e) => error!("Failed to serialize config: {}", e),
    }
}

async fn save_miners_to_file(miners: &[Miner]) {
    let path = miners_file_path();

    match serde_json::to_string_pretty(miners) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(&path, json).await {
                error!("Failed to save miners to {}: {}", path, e);
            }
        }
        Err(e) => error!("Failed to serialize miners: {}", e),
    }
}

// =============================================
// RPC LOGIC
// =============================================

async fn call_rpc(
    client: &Client,
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

    let start_idx = *state.active_rpc_idx.read().await;
    let mut last_err = String::new();

    for i in 0..RPC_ENDPOINTS.len() {
        let idx = (start_idx + i) % RPC_ENDPOINTS.len();
        let url = RPC_ENDPOINTS[idx];

        match client.post(url).json(&req_body).send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    last_err = format!("HTTP {} on {}", resp.status(), url);
                    continue;
                }

                match resp.json::<serde_json::Value>().await {
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
                            info!(
                                "Successfully connected to fallback RPC: {} (index {})",
                                url, idx
                            );
                            *state.active_rpc_idx.write().await = idx;
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
                last_err = format!("Network error on {}: {}", url, e);
                continue;
            }
        }
    }

    Err(format!("All RPC endpoints failed. Last error: {}", last_err))
}

// =============================================
// ON-CHAIN DATA READING
// =============================================

/// Reads globals() → (tshare_rate_hex, daily_data_count, penalties_hex)
async fn read_globals(
    client: &Client,
    state: &Arc<AppState>,
) -> Result<(f64, u64, f64), String> {
    let params = serde_json::json!([
        { "to": HEX_CONTRACT, "data": GLOBALS_SELECTOR },
        "latest"
    ]);

    let hex_result = call_rpc(client, state, "eth_call", params).await?;
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

/// Reads dailyData(day) → (day_payout_hearts, day_stake_shares)
async fn read_daily_data(
    client: &Client,
    state: &Arc<AppState>,
    day: u64,
) -> Result<(f64, f64), String> {
    let call_data = format!("{}{:064x}", DAILY_DATA_SELECTOR, day);

    let params = serde_json::json!([
        { "to": HEX_CONTRACT, "data": call_data },
        "latest"
    ]);

    let hex_result = call_rpc(client, state, "eth_call", params).await?;
    let d_str = hex_result.strip_prefix("0x").unwrap_or(&hex_result);

    if d_str.len() < 128 {
        return Err(format!("dailyData({}) too short ({} chars)", day, d_str.len()));
    }

    let day_payout = U256::from_hex(&d_str[0..64]);
    let day_shares = U256::from_hex(&d_str[64..128]);

    Ok((day_payout.to_f64(), day_shares.to_f64()))
}

// =============================================
// RPC PRICE LOGIC
// =============================================
const USDC_HEX_PAIR: &str = "0xC475332e92561CD58f278E4e2eD76c17D5b50f05";
const GET_RESERVES_SELECTOR: &str = "0x0902f1ac";

async fn fetch_price_rpc(client: &Client, state: &Arc<AppState>) -> Result<f64, String> {
    let params = serde_json::json!([
        { "to": USDC_HEX_PAIR, "data": GET_RESERVES_SELECTOR },
        "latest"
    ]);

    let hex_result = call_rpc(client, state, "eth_call", params).await?;
    let r_str = hex_result.strip_prefix("0x").unwrap_or(&hex_result);

    if r_str.len() < 128 {
        return Err(format!("getReserves() too short ({} chars)", r_str.len()));
    }

    let reserve_usdc = U256::from_hex(&r_str[0..64]).to_f64();
    let reserve_hex = U256::from_hex(&r_str[64..128]).to_f64();

    if reserve_usdc == 0.0 || reserve_hex == 0.0 {
        return Err("Invalid reserves (zero)".to_string());
    }

    let price = (reserve_usdc / reserve_hex) * 100.0;

    if price <= 0.0 || !price.is_finite() {
        return Err("RPC returned zero or invalid price".to_string());
    }

    Ok(price)
}

// =============================================
// EXTERNAL DATA SOURCES
// =============================================

async fn fetch_price_dexscreener(client: &Client) -> Result<f64, String> {
    let resp: serde_json::Value = client
        .get(DEXSCREENER_URL)
        .send()
        .await
        .map_err(|e| format!("DEXScreener request failed: {}", e))?
        .json()
        .await
        .map_err(|e| format!("DEXScreener JSON parse failed: {}", e))?;

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

async fn fetch_price_geckoterminal(client: &Client) -> Result<f64, String> {
    let resp: serde_json::Value = client
        .get(GECKOTERMINAL_URL)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("GeckoTerminal request failed: {}", e))?
        .json()
        .await
        .map_err(|e| format!("GeckoTerminal JSON parse failed: {}", e))?;

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

async fn fetch_price(client: &Client, state: &Arc<AppState>) -> Result<f64, String> {
    match fetch_price_rpc(client, state).await {
        Ok(price) => return Ok(price),
        Err(e) => {
            warn!("RPC price fetch failed: {}. Falling back to DexScreener...", e);
        }
    }

    match fetch_price_dexscreener(client).await {
        Ok(price) => return Ok(price),
        Err(e) => {
            warn!("DexScreener failed: {}. Falling back to GeckoTerminal...", e);
        }
    }

    fetch_price_geckoterminal(client).await
}

async fn fetch_hexdailystats_backfill(
    client: &Client,
) -> (HashMap<u64, f64>, HashMap<u64, f64>) {
    info!("Attempting historical fetch from HEXDailyStats...");

    let resp = match client.get(HEXDAILYSTATS_URL).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("HEXDailyStats unreachable: {}", e);
            return (HashMap::new(), HashMap::new());
        }
    };

    if !resp.status().is_success() {
        warn!("HEXDailyStats returned HTTP {}", resp.status());
        return (HashMap::new(), HashMap::new());
    }

    let entries: Vec<HexJsonEntry> = match resp.json().await {
        Ok(e) => e,
        Err(e) => {
            warn!("HEXDailyStats JSON parse failed: {}", e);
            return (HashMap::new(), HashMap::new());
        }
    };

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

    info!(
        "HEXDailyStats: loaded {} T-Share rate points, {} price points",
        tshare_map.len(),
        price_map.len()
    );

    (tshare_map, price_map)
}

// =============================================
// DAILY ENTRY STORAGE
// =============================================

async fn store_daily_entry(state: &Arc<AppState>, entry: HexJsonEntry) -> bool {
    let mut hex_json = state.hex_json.write().await;

    if hex_json.iter().any(|e| e.current_day == entry.current_day) {
        info!("Day {} already recorded. Skipping.", entry.current_day);
        return false;
    }

    info!(
        "Recorded day {}: payout={:.2} HEX, payout/tshare={:.4}, tshareRate={:.1}, price=${:.8}",
        entry.current_day,
        entry.daily_payout_hex,
        entry.payout_per_tshare_hex,
        entry.tshare_rate_hex,
        entry.price_pulse_x
    );

    let mut new_data = (**hex_json).clone();
    new_data.push(entry);
    new_data.sort_by_key(|e| e.current_day);

    *hex_json = Arc::new(new_data);
    state.hex_json_version.fetch_add(1, Ordering::Relaxed);

    let data_clone = hex_json.clone();
    drop(hex_json);

    save_hex_json_to_file(&data_clone).await;
    true
}

// =============================================
// BACKFILL
// =============================================

async fn backfill_hex_json(
    client: &Client,
    state: &Arc<AppState>,
    existing_data: &[HexJsonEntry],
) -> Vec<HexJsonEntry> {
    info!("Starting HEXJSON backfill...");

    let (current_tshare_rate, day_count, _penalties) =
        match read_globals(client, state).await {
            Ok(v) => v,
            Err(e) => {
                error!("Backfill failed: cannot read globals(): {}", e);
                return existing_data.to_vec();
            }
        };

    info!(
        "globals(): tshareRate={:.1} HEX, dailyDataCount={}",
        current_tshare_rate, day_count
    );

    if day_count == 0 {
        warn!("dailyDataCount is 0. Nothing to backfill.");
        return existing_data.to_vec();
    }

    let (hds_tshares, hds_prices) = fetch_hexdailystats_backfill(client).await;
        let current_price = match fetch_price(client, state).await {
        Ok(p) => {
            info!("Using current price: ${:.8}", p);
            p
        }
        Err(e) => {
            warn!("Price fetch failed: {}. Using 0.0.", e);
            0.0
        }
    };

    let mut by_day: HashMap<u64, HexJsonEntry> = HashMap::new();

    for entry in existing_data {
        by_day.insert(entry.current_day, entry.clone());
    }

    let missing_days: Vec<u64> = (1..day_count)
        .filter(|day| !by_day.contains_key(day))
        .collect();

    if missing_days.is_empty() {
        let mut result: Vec<HexJsonEntry> = by_day.into_values().collect();
        result.sort_by_key(|e| e.current_day);
        info!("Backfill complete. Total HEXJSON entries: {}", result.len());
        return result;
    }

    let total_to_fetch = missing_days.len();

    info!(
        "Backfilling {} missing days up to day {}...",
        total_to_fetch,
        day_count - 1
    );

    let mut fetched = 0usize;
    let mut consecutive_errors = 0u32;
    let max_consecutive_errors = 50u32;

    let mut results_stream = stream::iter(missing_days)
        .map(|day| {
            let client = client.clone();
            let state = state.clone();

            async move {
                let result = read_daily_data(&client, &state, day).await;

                if BACKFILL_DELAY_MS > 0 {
                    tokio::time::sleep(Duration::from_millis(BACKFILL_DELAY_MS)).await;
                }

                (day, result)
            }
        })
        .buffer_unordered(BACKFILL_CONCURRENCY);

    while let Some((day, result)) = results_stream.next().await {
        match result {
            Ok((payout_hearts, shares)) => {
                consecutive_errors = 0;

                let daily_payout_hex = payout_hearts / HEARTS_PER_HEX;
                let payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);

                let price = hds_prices
                    .get(&day)
                    .copied()
                    .filter(|p| p.is_finite() && *p > 0.0)
                    .unwrap_or(current_price);

                let tshare = hds_tshares
                    .get(&day)
                    .copied()
                    .filter(|t| t.is_finite() && *t > 0.0)
                    .unwrap_or(current_tshare_rate);

                by_day.insert(
                    day,
                    HexJsonEntry {
                        current_day: day,
                        tshare_rate_hex: tshare,
                        daily_payout_hex,
                        payout_per_tshare_hex: payout_per_tshare,
                        price_pulse_x: price,
                    },
                );
            }
            Err(e) => {
                consecutive_errors += 1;

                if consecutive_errors >= max_consecutive_errors {
                    error!(
                        "Backfill aborted at day {}: {} consecutive errors. Last: {}",
                        day, consecutive_errors, e
                    );
                    break;
                }

                if consecutive_errors <= 3 || is_multiple_of(consecutive_errors as usize, 20) {
                    warn!("Backfill: error reading day {}: {}", day, e);
                }
            }
        }

        fetched += 1;

        if is_multiple_of(fetched, 100) || fetched == total_to_fetch {
            info!(
                "Backfill progress: {}/{} days ({:.1}%)",
                fetched,
                total_to_fetch,
                (fetched as f64 / total_to_fetch as f64) * 100.0
            );
        }

        if is_multiple_of(fetched, BACKFILL_SAVE_INTERVAL) {
            let mut partial: Vec<HexJsonEntry> = by_day.values().cloned().collect();
            partial.sort_by_key(|e| e.current_day);
            save_hex_json_to_file(&partial).await;
            info!("Backfill: intermediate save ({} total entries)", partial.len());
        }
    }

    let mut result: Vec<HexJsonEntry> = by_day.into_values().collect();
    result.sort_by_key(|e| e.current_day);

    info!("Backfill complete. Total HEXJSON entries: {}", result.len());
    result
}

// =============================================
// DAILY RECORDING
// =============================================

async fn record_daily_entry(
    client: &Client,
    state: &Arc<AppState>,
) -> Result<HexJsonEntry, String> {
    let (tshare_rate, day_count, _) = read_globals(client, state).await?;

    if day_count == 0 {
        return Err("dailyDataCount is 0, cannot record".to_string());
    }

    let target_day = day_count - 1;

    let (payout_hearts, shares) = read_daily_data(client, state, target_day).await?;

    let daily_payout_hex = payout_hearts / HEARTS_PER_HEX;
    let payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);

    let price = match fetch_price(client, state).await {
        Ok(p) => p,
        Err(e) => {
            let fallback = state.live_data.read().await.price_pulsechain;
            warn!(
                "Price fetch failed during daily recording: {}. Using last live price fallback: {:.8}",
                e, fallback
            );
            fallback
        }
    };

    Ok(HexJsonEntry {
        current_day: target_day,
        tshare_rate_hex: tshare_rate,
        daily_payout_hex,
        payout_per_tshare_hex: payout_per_tshare,
        price_pulse_x: price,
    })
}

// =============================================
// LIVE DATA FETCHING
// =============================================

async fn fetch_live_data(
    client: &Client,
    state: &Arc<AppState>,
) -> Result<LiveData, String> {
    let price = match fetch_price(client, state).await {
        Ok(p) => p,
        Err(e) => {
            warn!("Price fetch failed in live data loop: {}. Using 0.0", e);
            0.0
        }
    };

    let gas_price_hex = call_rpc(client, state, "eth_gasPrice", serde_json::json!([])).await?;
    let gas_price_wei = U256::from_hex(&gas_price_hex);
    let beat = gas_price_wei.to_f64() / 1e9;

    let (tshare_rate, daily_data_count, penalties) = read_globals(client, state).await?;

    let mut payout_per_tshare = 0.0;

    if daily_data_count > 0 {
        let day_to_query = daily_data_count - 1;

        if let Ok((payout_hearts, shares)) =
            read_daily_data(client, state, day_to_query).await
        {
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

async fn fetch_live_data_with_retry(
    client: &Client,
    state: &Arc<AppState>,
) -> Result<LiveData, String> {
    let mut delay = Duration::from_secs(1);
    let max_delay = Duration::from_secs(30);
    let start = Instant::now();
    let max_elapsed = Duration::from_secs(2 * 60);

    loop {
        match fetch_live_data(client, state).await {
            Ok(data) => return Ok(data),
            Err(e) => {
                let is_network_error = e.to_lowercase().contains("connection")
                    || e.to_lowercase().contains("dns")
                    || e.to_lowercase().contains("timeout")
                    || e.to_lowercase().contains("request")
                    || e.to_lowercase().contains("all rpc endpoints failed");

                if !is_network_error || start.elapsed() > max_elapsed {
                    return Err(format!("Live data fetch failed after retries: {}", e));
                }

                tokio::time::sleep(delay).await;
                delay = std::cmp::min(delay * 2, max_delay);
            }
        }
    }
}

// =============================================
// BACKGROUND TASKS
// =============================================

async fn live_data_updater(state: Arc<AppState>, client: Client) {
    let mut rx = state.config_tx.subscribe();

    match fetch_live_data_with_retry(&client, &state).await {
        Ok(data) => {
            *state.live_data.write().await = data;
            info!("Initial live data fetched successfully");
        }
        Err(e) => {
            error!("Initial live data fetch failed: {}", e);
        }
    }

    loop {
        let freq = {
            let config = state.config.read().await.clone();
            sanitize_config(config).live_data_frequency
        };

        let sleep = tokio::time::sleep(Duration::from_secs(freq * 60));
        tokio::pin!(sleep);

        tokio::select! {
            _ = &mut sleep => {
                match fetch_live_data_with_retry(&client, &state).await {
                    Ok(data) => {
                        *state.live_data.write().await = data;
                    }
                    Err(e) => {
                        error!("Background live data fetch failed: {}", e);
                    }
                }
            }
            result = rx.recv() => {
                match result {
                    Ok(_) => {
                        info!("Config changed, restarting live updater loop...");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Config receiver lagged by {}", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        warn!("Config receiver closed unexpectedly");
                    }
                }
            }
        }
    }
}

async fn hex_json_updater(state: Arc<AppState>, client: Client) {
    let file_data = load_hex_json_from_file().await;

    let initial_data = if file_data.is_empty() {
        info!("No persisted HEXJSON data. Building from RPC + external sources...");
        let backfilled = backfill_hex_json(&client, &state, &[]).await;
        save_hex_json_to_file(&backfilled).await;
        backfilled
    } else {
        match read_globals(&client, &state).await {
            Ok((_, day_count, _)) => {
                let known_days: HashSet<u64> =
                    file_data.iter().map(|e| e.current_day).collect();

                let missing_count = (1..day_count)
                    .filter(|day| !known_days.contains(day))
                    .count();

                if missing_count > 0 {
                    info!(
                        "HEXJSON has {} missing days. Backfilling/catching up...",
                        missing_count
                    );

                    let updated = backfill_hex_json(&client, &state, &file_data).await;
                    save_hex_json_to_file(&updated).await;
                    updated
                } else {
                    file_data
                }
            }
            Err(e) => {
                warn!(
                    "Cannot check chain state for catch-up: {}. Using file data as-is.",
                    e
                );
                file_data
            }
        }
    };

    {
        let mut hex_json = state.hex_json.write().await;
        *hex_json = Arc::new(initial_data);
        state.hex_json_version.fetch_add(1, Ordering::Relaxed);
        info!("HEXJSON loaded into memory: {} entries", hex_json.len());
    }

    loop {
        let sleep_duration = get_duration_until_next_1am_utc();

        info!(
            "HEXJSON updater sleeping for {:?} until next 1 AM UTC recording...",
            sleep_duration
        );

        tokio::time::sleep(sleep_duration).await;

        info!(
            "Waiting {} seconds for chain rollover to settle...",
            DAILY_RECORD_SETTLE_DELAY_SECS
        );

        tokio::time::sleep(Duration::from_secs(DAILY_RECORD_SETTLE_DELAY_SECS)).await;

        info!("Running daily HEXJSON recording...");

        let mut delay = Duration::from_secs(5);
        let max_retries = 10;
        let mut recorded = false;
        let mut last_entry: Option<HexJsonEntry> = None;

        for attempt in 1..=max_retries {
            match record_daily_entry(&client, &state).await {
                Ok(entry) => {
                    if is_valid_daily_entry(&entry) {
                        recorded = store_daily_entry(&state, entry).await;
                        break;
                    } else {
                        warn!(
                            "Daily entry for day {} appears incomplete/zero. Retrying...",
                            entry.current_day
                        );
                        last_entry = Some(entry);
                    }
                }
                Err(e) => {
                    warn!(
                        "Daily recording attempt {}/{} failed: {}. Retrying in {:?}...",
                        attempt, max_retries, e, delay
                    );
                }
            }

            tokio::time::sleep(delay).await;
            delay = std::cmp::min(delay * 2, Duration::from_secs(120));
        }

        if !recorded {
            if let Some(entry) = last_entry {
                warn!(
                    "Recording incomplete entry for day {} anyway to avoid a gap.",
                    entry.current_day
                );
                store_daily_entry(&state, entry).await;
            } else {
                error!(
                    "Daily HEXJSON recording failed after {} attempts. Will retry next cycle.",
                    max_retries
                );
            }
        }
    }
}

async fn test_rpc(client: &Client, url: &str) -> bool {
    let block_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_blockNumber",
        "params": [],
        "id": 1
    });

    let basic_ok = match client.post(url).json(&block_req).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(json) => json.get("error").is_none() && json.get("result").is_some(),
            Err(_) => false,
        },
        Err(_) => false,
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

    match client.post(url).json(&call_req).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(json) => json.get("error").is_none() && json.get("result").is_some(),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

async fn rpc_health_checker(state: Arc<AppState>, client: Client) {
    loop {
        tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;

        let current_idx = *state.active_rpc_idx.read().await;

        if current_idx != 0 {
            info!(
                "24h RPC health check: Testing primary RPC endpoint ({})...",
                RPC_ENDPOINTS[0]
            );

            if test_rpc(&client, RPC_ENDPOINTS[0]).await {
                info!("Primary RPC endpoint is back online. Switching back.");
                *state.active_rpc_idx.write().await = 0;
            } else {
                info!("Primary RPC endpoint still down. Will check again in 24h.");
            }
        }
    }
}

// =============================================
// API HANDLERS
// =============================================

async fn handle_live_data(State(state): State<Arc<AppState>>) -> Json<LiveData> {
    Json(state.live_data.read().await.clone())
}

async fn handle_miners(State(state): State<Arc<AppState>>) -> Json<Vec<Miner>> {
    Json(state.miners.read().await.clone())
}

async fn handle_hex_json(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HexJsonQuery>,
    headers: HeaderMap,
) -> Response {
    let data = state.hex_json.read().await.clone();
    let version = state.hex_json_version.load(Ordering::Relaxed);

    let full_request = query.from.is_none() && query.limit.is_none();

    let etag = if full_request {
        format!("\"hexjson-full-{}-{}\"", data.len(), version)
    } else {
        format!(
            "\"hexjson-filter-{}-{}-{}-{}\"",
            query.from.unwrap_or(0),
            query.limit.unwrap_or(usize::MAX),
            data.len(),
            version
        )
    };

    if let Some(if_none_match) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        if if_none_match == etag
            || if_none_match.trim_matches('"') == etag.trim_matches('"')
        {
            return match Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(
                    header::ETAG,
                    header::HeaderValue::from_str(&etag)
                        .unwrap_or_else(|_| header::HeaderValue::from_static("")),
                )
                .header(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-cache"),
                )
                .body(axum::body::Body::empty())
            {
                Ok(resp) => resp,
                Err(_) => StatusCode::NOT_MODIFIED.into_response(),
            };
        }
    }

    let mut response = if full_request {
        Json(data).into_response()
    } else {
        let from = query.from.unwrap_or(0);
        let limit = query.limit.unwrap_or(usize::MAX);

        let filtered: Vec<HexJsonEntry> = data
            .iter()
            .filter(|entry| entry.current_day >= from)
            .take(limit)
            .cloned()
            .collect();

        Json(filtered).into_response()
    };

    if let Ok(etag_value) = header::HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, etag_value);
    }

    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );

    response
}

async fn handle_get_config(State(state): State<Arc<AppState>>) -> Json<Config> {
    Json(state.config.read().await.clone())
}

async fn handle_post_config(
    State(state): State<Arc<AppState>>,
    Json(new_config): Json<Config>,
) -> impl IntoResponse {
    let new_config = sanitize_config(new_config);

    {
        let mut config = state.config.write().await;
        *config = new_config.clone();
    }

    let _ = state.config_tx.send(());
    save_config_to_file(&new_config).await;

    StatusCode::OK
}

async fn handle_add_miner(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddMinerRequest>,
) -> impl IntoResponse {
    let start = parse_date(&req.start_date);
    let end = parse_date(&req.end_date);

    let (Some(start), Some(end)) = (start, end) else {
        return StatusCode::BAD_REQUEST;
    };

    if end < start {
        return StatusCode::BAD_REQUEST;
    }

    if req.t_shares <= 0.0 || !req.t_shares.is_finite() {
        return StatusCode::BAD_REQUEST;
    }

    let id = state.next_miner_id.fetch_add(1, Ordering::SeqCst);

    let miner = Miner {
        id: Some(id),
        start_date: req.start_date,
        end_date: req.end_date,
        t_shares: req.t_shares,
        status: None,
    };

    let mut miners = state.miners.write().await;
    miners.push(miner);

    let miners_clone = miners.clone();
    drop(miners);

    save_miners_to_file(&miners_clone).await;

    StatusCode::CREATED
}

async fn handle_end_miner(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MinerIdRequest>,
) -> impl IntoResponse {
    let mut miners = state.miners.write().await;

    let Some(miner) = miners
        .iter_mut()
        .find(|m| m.id == Some(req.id))
    else {
        return StatusCode::BAD_REQUEST;
    };

    miner.status = Some("completed".to_string());

    let miners_clone = miners.clone();
    drop(miners);

    save_miners_to_file(&miners_clone).await;

    StatusCode::OK
}

async fn handle_delete_miner(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MinerIdRequest>,
) -> impl IntoResponse {
    let mut miners = state.miners.write().await;
    let before = miners.len();

    miners.retain(|m| m.id != Some(req.id));

    if miners.len() == before {
        return StatusCode::BAD_REQUEST;
    }

    let miners_clone = miners.clone();
    drop(miners);

    save_miners_to_file(&miners_clone).await;

    StatusCode::OK
}

// =============================================
// MAIN & ROUTING
// =============================================

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tokio::fs::create_dir_all(DATA_DIR)
        .await
        .expect("Failed to create data dir");

    let initial_config = match tokio::fs::read_to_string(config_file_path()).await {
        Ok(content) => serde_json::from_str(&content).unwrap_or(Config {
            live_data_frequency: 15,
            liquid_hex: 0.0,
            historical_start_day: 1260,
        }),
        Err(_) => Config {
            live_data_frequency: 15,
            liquid_hex: 0.0,
            historical_start_day: 1260,
        },
    };

    let initial_config = sanitize_config(initial_config);

    let raw_miners = match tokio::fs::read_to_string(miners_file_path()).await {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    let (initial_miners, next_miner_id) = normalize_miners(raw_miners);

    let (config_tx, _) = broadcast::channel(16);

    let state = Arc::new(AppState {
        live_data: RwLock::new(LiveData::default()),
        hex_json: RwLock::new(Arc::new(Vec::new())),
        miners: RwLock::new(initial_miners),
        config: RwLock::new(initial_config),
        config_tx,
        active_rpc_idx: RwLock::new(0),
        hex_json_version: AtomicU64::new(1),
        next_miner_id: AtomicU64::new(next_miner_id),
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .pool_max_idle_per_host(16)
        .user_agent("hexfetch-rs/0.3")
        .build()
        .unwrap();

    tokio::spawn(live_data_updater(state.clone(), client.clone()));
    tokio::spawn(hex_json_updater(state.clone(), client.clone()));
    tokio::spawn(rpc_health_checker(state.clone(), client.clone()));

    let app = Router::new()
        .route("/api/live-data", get(handle_live_data))
        .route("/api/miners", get(handle_miners))
        .route("/api/add-miner", post(handle_add_miner))
        .route("/api/end-miner", post(handle_end_miner))
        .route("/api/delete-miner", post(handle_delete_miner))
        .route("/api/hexjson", get(handle_hex_json))
        .route("/api/config", get(handle_get_config).post(handle_post_config))
        .fallback(get(|uri: axum::http::Uri| async move {
            let path = uri.path().trim_start_matches('/');
            let path = if path.is_empty() { "index.html" } else { path };

            if path.contains("..") {
                return Err(StatusCode::NOT_FOUND);
            }

            match Assets::get(path) {
                Some(content) => {
                    let mime = get_mime_type(path);

                    Ok::<_, StatusCode>(
                        axum::response::Response::builder()
                            .header("Content-Type", mime)
                            .body(axum::body::Body::from(content.data.into_owned()))
                            .unwrap(),
                    )
                }
                None => Err(StatusCode::NOT_FOUND),
            }
        }))
        .layer(CompressionLayer::new())
        .with_state(state);

    let addr: SocketAddr = std::env::var("HEXFETCH_BIND")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 5555)));

    info!("⬢ HEX Stats server starting on {} ⬢", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            info!("Shutdown signal received");
        })
        .await
        .unwrap();
}
