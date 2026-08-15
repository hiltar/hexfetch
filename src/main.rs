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
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use tower_http::compression::CompressionLayer;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

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
const HEXDAILYSTATS_URL: &str = "https://hexdailystats.com/fulldatapulsechain";
const GLOBALS_SELECTOR: &str = "0xc3124525";
const DAILY_DATA_SELECTOR: &str = "0x90de6871";
const HEARTS_PER_HEX: f64 = 1e8;
const TSHARE_UNIT: f64 = 10000.0;
const BACKFILL_DELAY_MS: u64 = 60;
const BACKFILL_SAVE_INTERVAL: usize = 1000;
const BACKFILL_CONCURRENCY: usize = 4;
const DAILY_INITIAL_DELAY_SECS: u64 = 120;

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
    Ok(opt.filter(|v| v.is_finite()).unwrap_or(0.0))
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
    #[serde(default)]
    pub id: u64,

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
struct IdRequest {
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
    to: Option<u64>,
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
    next_miner_id: RwLock<u64>,
    hex_json_version: AtomicU64,
}

// =============================================
// HELPERS
// =============================================

fn default_config() -> Config {
    Config {
        live_data_frequency: 15,
        liquid_hex: 0.0,
        historical_start_day: 1260,
    }
}

fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

fn sanitize_config(config: Config) -> Config {
    Config {
        live_data_frequency: config.live_data_frequency.clamp(1, 1440),
        liquid_hex: if config.liquid_hex.is_finite() && config.liquid_hex >= 0.0 {
            config.liquid_hex
        } else {
            0.0
        },
        historical_start_day: config.historical_start_day.max(1),
    }
}

fn data_path(file_name: &str) -> PathBuf {
    PathBuf::from(DATA_DIR).join(file_name)
}

async fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp_path = path.with_extension("tmp");

    tokio::fs::write(&tmp_path, data)
        .await
        .map_err(|e| format!("failed to write temp file {:?}: {}", tmp_path, e))?;

    tokio::fs::rename(&tmp_path, path)
        .await
        .map_err(|e| format!("failed to rename temp file {:?}: {}", tmp_path, e))?;

    Ok(())
}

fn parse_date_naive(s: &str) -> Option<NaiveDate> {
    let date = NaiveDate::parse_from_str(s.trim(), "%d-%m-%Y").ok()?;

    let y = date.year();
    if !(2000..=2100).contains(&y) {
        return None;
    }

    Some(date)
}

fn normalize_hexjson(entries: Vec<HexJsonEntry>) -> Vec<HexJsonEntry> {
    let mut by_day: HashMap<u64, HexJsonEntry> = HashMap::new();

    for mut entry in entries {
        if entry.current_day == 0 {
            continue;
        }

        entry.tshare_rate_hex = finite_or_zero(entry.tshare_rate_hex);
        entry.daily_payout_hex = finite_or_zero(entry.daily_payout_hex);
        entry.payout_per_tshare_hex = finite_or_zero(entry.payout_per_tshare_hex);
        entry.price_pulse_x = finite_or_zero(entry.price_pulse_x);

        by_day.insert(entry.current_day, entry);
    }

    let mut result: Vec<HexJsonEntry> = by_day.into_values().collect();
    result.sort_by_key(|e| e.current_day);
    result
}

fn normalize_miners(mut miners: Vec<Miner>) -> (Vec<Miner>, u64) {
    let mut seen = HashSet::new();
    let mut next_id = 1u64;

    for miner in miners.iter_mut() {
        if miner.id == 0 || seen.contains(&miner.id) {
            while seen.contains(&next_id) {
                next_id += 1;
            }

            miner.id = next_id;
            seen.insert(next_id);
            next_id += 1;
        } else {
            seen.insert(miner.id);
            if miner.id >= next_id {
                next_id = miner.id + 1;
            }
        }
    }

    (miners, next_id)
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

fn get_duration_until_next_0am_utc() -> Duration {
    let now = Utc::now();

    let today_0am = now
        .date_naive()
        .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());

    let mut next_0am = today_0am.and_utc();

    if next_0am <= now {
        next_0am = next_0am.checked_add_days(Days::new(1)).unwrap();
    }

    (next_0am - now)
        .to_std()
        .unwrap_or(Duration::from_secs(60))
}

// =============================================
// RPC LOGIC
// =============================================

async fn call_rpc(
    client: &Client,
    state: &Arc<AppState>,
    method: &str,
    params: Value,
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

                match resp.json::<Value>().await {
                    Ok(json) => {
                        if let Some(err) = json.get("error") {
                            if !err.is_null() {
                                last_err = format!(
                                    "RPC error on {}: {}",
                                    url,
                                    err["message"].as_str().unwrap_or("unknown")
                                );
                                continue;
                            }
                        }

                        if idx != start_idx {
                            info!(
                                "Successfully connected to fallback RPC: {} (index {})",
                                url, idx
                            );
                            *state.active_rpc_idx.write().await = idx;
                        }

                        let result = json.get("result").cloned().unwrap_or(Value::Null);

                        return Ok(match result {
                            Value::String(s) => s,
                            Value::Null => String::new(),
                            other => other.to_string(),
                        });
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
        finite_or_zero(share_rate_raw.to_f64() / 10.0)
    } else {
        0.0
    };

    let penalties = finite_or_zero(penalty_raw.to_f64() / HEARTS_PER_HEX);
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

    Ok((finite_or_zero(day_payout.to_f64()), finite_or_zero(day_shares.to_f64())))
}

// =============================================
// CALCULATION HELPERS
// =============================================

fn calc_payout_per_tshare(payout_hearts: f64, shares: f64) -> f64 {
    if payout_hearts <= 0.0 || shares <= 0.0 || !payout_hearts.is_finite() || !shares.is_finite() {
        return 0.0;
    }

    finite_or_zero((payout_hearts / shares) * TSHARE_UNIT)
}

// =============================================
// EXTERNAL DATA SOURCES
// =============================================

fn parse_dex_price(resp: &Value) -> Option<f64> {
    let pairs = resp.get("pairs")?.as_array()?;

    let mut best: Option<(f64, f64)> = None;

    for pair in pairs {
        let price = pair
            .get("priceUsd")
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .filter(|p| p.is_finite() && *p > 0.0);

        if let Some(price) = price {
            let liquidity = pair
                .pointer("/liquidity/usd")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            best = Some(match best {
                None => (price, liquidity),
                Some((_, best_liquidity)) if liquidity > best_liquidity => (price, liquidity),
                Some(existing) => existing,
            });
        }
    }

    best.map(|(price, _)| price)
}

/// Current price from DEXScreener.
async fn fetch_price_dexscreener(client: &Client) -> Result<f64, String> {
    let resp: Value = client
        .get(DEXSCREENER_URL)
        .send()
        .await
        .map_err(|e| format!("DEXScreener request failed: {}", e))?
        .json()
        .await
        .map_err(|e| format!("DEXScreener JSON parse failed: {}", e))?;

    parse_dex_price(&resp)
        .ok_or_else(|| "DEXScreener returned zero or invalid price".to_string())
}

/// ONE-TIME fetch of historical T-Share rates (and prices) from HEXDailyStats.
async fn fetch_hexdailystats_backfill(
    client: &Client,
) -> (HashMap<u64, f64>, HashMap<u64, f64>) {
    info!("Attempting one-time historical fetch from HEXDailyStats...");

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
        if entry.tshare_rate_hex.is_finite() && entry.tshare_rate_hex > 0.0 {
            tshare_map.insert(entry.current_day, entry.tshare_rate_hex);
        }

        if entry.price_pulse_x.is_finite() && entry.price_pulse_x > 0.0 {
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
// HEXJSON PERSISTENCE
// =============================================

async fn save_hex_json_to_file(data: &[HexJsonEntry]) {
    let path = data_path("hexjson.json");

    match serde_json::to_vec(data) {
        Ok(bytes) => {
            if let Err(e) = atomic_write(&path, &bytes).await {
                error!("Failed to save hexjson to {:?}: {}", path, e);
            }
        }
        Err(e) => error!("Failed to serialize hexjson: {}", e),
    }
}

async fn load_hex_json_from_file() -> Vec<HexJsonEntry> {
    let path = data_path("hexjson.json");

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => match serde_json::from_str::<Vec<HexJsonEntry>>(&content) {
            Ok(data) => {
                let normalized = normalize_hexjson(data);
                info!("Loaded {} hexjson entries from file", normalized.len());
                normalized
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

// =============================================
// BACKFILL: BUILD HEXJSON
// =============================================

async fn backfill_hex_json(
    client: &Client,
    state: &Arc<AppState>,
    existing_data: &[HexJsonEntry],
) -> Vec<HexJsonEntry> {
    info!("Starting HEXJSON backfill...");

    // 1. On-chain globals → current tshare rate + day count
    let (current_tshare_rate, day_count, _penalties) = match read_globals(client, state).await {
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

    // 2. HEXDailyStats one-time fetch (per-day prices + T-Share rates)
    let (hds_tshares, hds_prices) = fetch_hexdailystats_backfill(client).await;

    // 3. DEXScreener current price fallback
    let current_price = match fetch_price_dexscreener(client).await {
        Ok(p) => {
            info!("Using DEXScreener current price as fallback: ${:.8}", p);
            p
        }
        Err(e) => {
            warn!("DEXScreener fallback failed: {}. Using 0.0.", e);
            0.0
        }
    };

    // 4. Determine missing days (gap-aware)
    let known_days: HashSet<u64> = existing_data.iter().map(|e| e.current_day).collect();

    let missing_days: Vec<u64> = (1..day_count)
        .filter(|day| !known_days.contains(day))
        .collect();

    if missing_days.is_empty() {
        info!("No missing HEXJSON days detected.");
        return normalize_hexjson(existing_data.to_vec());
    }

    let mut merged: HashMap<u64, HexJsonEntry> = existing_data
        .iter()
        .map(|e| (e.current_day, e.clone()))
        .collect();

    let total_missing = missing_days.len();

    info!(
        "Backfilling {} missing day(s) up to chain day {}...",
        total_missing,
        day_count - 1
    );

    let mut fetched = 0usize;

    // 5. Fetch in bounded-concurrency chunks
    for chunk in missing_days.chunks(BACKFILL_SAVE_INTERVAL) {
        let results: Vec<(u64, Result<(f64, f64), String>)> =
            stream::iter(chunk.iter().copied())
                .map(|day| async move {
                    let result = read_daily_data(client, state, day).await;
                    tokio::time::sleep(Duration::from_millis(BACKFILL_DELAY_MS)).await;
                    (day, result)
                })
                .buffer_unordered(BACKFILL_CONCURRENCY)
                .collect()
                .await;

        let mut chunk_errors = 0usize;

        for (day, result) in results {
            fetched += 1;

            match result {
                Ok((payout_hearts, shares)) => {
                    let daily_payout_hex = finite_or_zero(payout_hearts / HEARTS_PER_HEX);
                    let payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);

                    let price = hds_prices
                        .get(&day)
                        .copied()
                        .filter(|v| v.is_finite() && *v > 0.0)
                        .unwrap_or(current_price);

                    let tshare = hds_tshares
                        .get(&day)
                        .copied()
                        .filter(|v| v.is_finite() && *v > 0.0)
                        .unwrap_or(current_tshare_rate);

                    merged.insert(
                        day,
                        HexJsonEntry {
                            current_day: day,
                            tshare_rate_hex: finite_or_zero(tshare),
                            daily_payout_hex,
                            payout_per_tshare_hex: payout_per_tshare,
                            price_pulse_x: finite_or_zero(price),
                        },
                    );
                }
                Err(e) => {
                    chunk_errors += 1;

                    if chunk_errors <= 3 || chunk_errors % 20 == 0 {
                        warn!("Backfill: error reading day {}: {}", day, e);
                    }
                }
            }

            if fetched % 100 == 0 || fetched == total_missing {
                info!(
                    "Backfill progress: {}/{} days ({:.1}%)",
                    fetched,
                    total_missing,
                    (fetched as f64 / total_missing as f64) * 100.0
                );
            }
        }

        if chunk_errors == chunk.len() && chunk_errors > 20 {
            error!("Too many consecutive backfill errors in chunk. Aborting backfill.");
            break;
        }

        // Intermediate save, but not excessively often.
        if fetched % BACKFILL_SAVE_INTERVAL == 0 || fetched == total_missing {
            let mut partial: Vec<HexJsonEntry> = merged.values().cloned().collect();
            partial.sort_by_key(|e| e.current_day);
            save_hex_json_to_file(&partial).await;
            info!("Backfill: intermediate save ({} total entries)", partial.len());
        }
    }

    let mut result: Vec<HexJsonEntry> = merged.into_values().collect();
    result.sort_by_key(|e| e.current_day);

    info!("Backfill complete. Total HEXJSON entries: {}", result.len());
    result
}

// =============================================
// DAILY RECORDING (GOING FORWARD)
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

    let daily_payout_hex = finite_or_zero(payout_hearts / HEARTS_PER_HEX);
    let payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);

    let price = match fetch_price_dexscreener(client).await {
        Ok(p) => p,
        Err(e) => {
            warn!(
                "Daily recording: DEXScreener price fetch failed: {}. Falling back to last live price.",
                e
            );
            state.live_data.read().await.price_pulsechain
        }
    };

    Ok(HexJsonEntry {
        current_day: target_day,
        tshare_rate_hex: finite_or_zero(tshare_rate),
        daily_payout_hex,
        payout_per_tshare_hex: payout_per_tshare,
        price_pulse_x: finite_or_zero(price),
    })
}

// =============================================
// LIVE DATA FETCHING
// =============================================

async fn fetch_live_data(
    client: &Client,
    state: &Arc<AppState>,
) -> Result<LiveData, String> {
    // DEXScreener → current price
    let dex_resp: Value = client
        .get(DEXSCREENER_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let price = parse_dex_price(&dex_resp)
        .ok_or_else(|| "DEXScreener returned zero or invalid price".to_string())?;

    // RPC → gas price (beat)
    let gas_price_hex = call_rpc(client, state, "eth_gasPrice", serde_json::json!([])).await?;
    let gas_price_wei = U256::from_hex(&gas_price_hex);
    let beat = finite_or_zero(gas_price_wei.to_f64() / 1e9);

    // RPC → globals (tshare rate, penalties, day count)
    let (tshare_rate, daily_data_count, penalties) = read_globals(client, state).await?;

    // RPC → dailyData for latest day (payout per tshare)
    let mut payout_per_tshare = 0.0;

    if daily_data_count > 0 {
        let day_to_query = daily_data_count - 1;

        if let Ok((payout_hearts, shares)) = read_daily_data(client, state, day_to_query).await {
            payout_per_tshare = calc_payout_per_tshare(payout_hearts, shares);
        }
    }

    Ok(LiveData {
        price_pulsechain: finite_or_zero(price),
        tshare_price_pulsechain: finite_or_zero(tshare_rate * price),
        tshare_rate_hex_pulsechain: finite_or_zero(tshare_rate),
        penalties_hex_pulsechain: finite_or_zero(penalties),
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
    let start = std::time::Instant::now();
    let max_elapsed = Duration::from_secs(2 * 60);

    loop {
        match fetch_live_data(client, state).await {
            Ok(data) => return Ok(data),
            Err(e) => {
                if start.elapsed() > max_elapsed {
                    return Err(format!("Live data fetch failed after retries: {}", e));
                }

                warn!("Live data fetch failed: {}. Retrying in {:?}...", e, delay);
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
            let config = state.config.read().await;
            sanitize_config((*config).clone()).live_data_frequency
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
                    Err(_) => {
                        warn!("Config receiver closed unexpectedly");
                    }
                }
            }
        }
    }
}

/// HEXJSON updater:
/// - backfills gaps on startup
/// - records daily after 00:00 UTC with delay + validation
async fn hex_json_updater(state: Arc<AppState>, client: Client) {
    // Phase 1: Initial load / backfill
    let file_data = load_hex_json_from_file().await;

    let initial_data = backfill_hex_json(&client, &state, &file_data).await;

    if initial_data.len() != file_data.len() {
        save_hex_json_to_file(&initial_data).await;
    }

    let initial_arc = Arc::new(initial_data);

    {
        let mut hex_json = state.hex_json.write().await;
        info!("HEXJSON loaded into memory: {} entries", hex_json.len());
        *hex_json = initial_arc;
        state.hex_json_version.fetch_add(1, Ordering::Relaxed);
    }

    // Phase 2: Daily recording loop at 0 AM UTC
    loop {
        let sleep_duration = get_duration_until_next_0am_utc();

        info!(
            "HEXJSON updater sleeping for {:?} until next 0 AM UTC recording...",
            sleep_duration
        );

        tokio::time::sleep(sleep_duration).await;

        // Give the chain / indexer a little time after day rollover.
        tokio::time::sleep(Duration::from_secs(DAILY_INITIAL_DELAY_SECS)).await;

        info!("Running daily HEXJSON recording...");

        let mut delay = Duration::from_secs(30);
        let max_retries = 30;
        let mut recorded = false;

        for attempt in 1..=max_retries {
            match record_daily_entry(&client, &state).await {
                Ok(entry) => {
                    let looks_valid = entry.current_day > 0
                        && (entry.daily_payout_hex > 0.0
                            || entry.payout_per_tshare_hex > 0.0
                            || entry.tshare_rate_hex > 0.0);

                    if !looks_valid {
                        warn!(
                            "Daily recording attempt {}/{} returned weak data. Retrying in {:?}...",
                            attempt, max_retries, delay
                        );

                        tokio::time::sleep(delay).await;
                        delay = std::cmp::min(delay * 2, Duration::from_secs(120));
                        continue;
                    }

                    let maybe_new_arc = {
                        let hex_json = state.hex_json.read().await;

                        if hex_json.iter().any(|e| e.current_day == entry.current_day) {
                            info!("Day {} already recorded. Skipping.", entry.current_day);
                            None
                        } else {
                            let mut new_data = (**hex_json).clone();

                            info!(
                                "Recorded day {}: payout={:.2} HEX, payout/tshare={:.4}, tshareRate={:.1}, price=${:.8}",
                                entry.current_day,
                                entry.daily_payout_hex,
                                entry.payout_per_tshare_hex,
                                entry.tshare_rate_hex,
                                entry.price_pulse_x
                            );

                            new_data.push(entry);
                            new_data.sort_by_key(|e| e.current_day);

                            Some(Arc::new(new_data))
                        }
                    };

                    if let Some(new_arc) = maybe_new_arc {
                        {
                            let mut hex_json = state.hex_json.write().await;
                            *hex_json = new_arc.clone();
                            state.hex_json_version.fetch_add(1, Ordering::Relaxed);
                        }

                        save_hex_json_to_file(&new_arc).await;
                    }

                    recorded = true;
                    break;
                }
                Err(e) => {
                    warn!(
                        "Daily recording attempt {}/{} failed: {}. Retrying in {:?}...",
                        attempt, max_retries, e, delay
                    );

                    tokio::time::sleep(delay).await;
                    delay = std::cmp::min(delay * 2, Duration::from_secs(120));
                }
            }
        }

        if !recorded {
            error!(
                "Daily HEXJSON recording failed after {} attempts. Will retry next cycle.",
                max_retries
            );
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
        Ok(resp) => match resp.json::<Value>().await {
            Ok(json) => json.get("error").map_or(true, |e| e.is_null()) && json.get("result").is_some(),
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
        "params": [{ "to": HEX_CONTRACT, "data": GLOBALS_SELECTOR }, "latest"],
        "id": 2
    });

    match client.post(url).json(&call_req).send().await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(json) => json.get("error").map_or(true, |e| e.is_null()) && json.get("result").is_some(),
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
                info!("Primary RPC endpoint is back online! Switching back.");
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

async fn handle_get_config(State(state): State<Arc<AppState>>) -> Json<Config> {
    Json(state.config.read().await.clone())
}

async fn handle_hex_json(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HexJsonQuery>,
    headers: HeaderMap,
) -> Response {
    let data = state.hex_json.read().await.clone();
    let version = state.hex_json_version.load(Ordering::Relaxed);

    let (last_day, last_price) = data
        .last()
        .map(|e| (e.current_day, e.price_pulse_x))
        .unwrap_or((0, 0.0));

    let etag = format!(
        "\"{}-{}-{}-{}-{:?}-{:?}-{:?}\"",
        version,
        data.len(),
        last_day,
        last_price,
        query.from,
        query.to,
        query.limit
    );

    let mut response_headers = HeaderMap::new();

    response_headers.insert(
        header::ETAG,
        etag
            .parse()
            .unwrap_or_else(|_| header::HeaderValue::from_static("\"\"")),
    );

    response_headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("public, max-age=60, stale-while-revalidate=3600"),
    );

    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if if_none_match == etag {
            return (response_headers, StatusCode::NOT_MODIFIED).into_response();
        }
    }

    let limit = query.limit.unwrap_or(usize::MAX).min(100_000);

    let filtered: Vec<HexJsonEntry> = data
        .iter()
        .filter(|e| query.from.map_or(true, |from| e.current_day >= from))
        .filter(|e| query.to.map_or(true, |to| e.current_day <= to))
        .take(limit)
        .cloned()
        .collect();

    (response_headers, Json(filtered)).into_response()
}

async fn handle_post_config(
    State(state): State<Arc<AppState>>,
    Json(new_config): Json<Config>,
) -> impl IntoResponse {
    let new_config = sanitize_config(new_config);

    {
        let mut config = state.config.write().await;
        *config = new_config;
    }

    let _ = state.config_tx.send(());

    let cfg = state.config.read().await.clone();

    if let Ok(bytes) = serde_json::to_vec_pretty(&cfg) {
        let _ = atomic_write(&data_path("config.json"), &bytes).await;
    }

    StatusCode::OK
}

async fn handle_add_miner(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddMinerRequest>,
) -> impl IntoResponse {
    let start = parse_date_naive(&req.start_date);
    let end = parse_date_naive(&req.end_date);

    let valid = match (start, end) {
        (Some(s), Some(e)) => {
            e >= s
                && req.t_shares.is_finite()
                && req.t_shares > 0.0
                && req.t_shares <= 1e18
        }
        _ => false,
    };

    if !valid {
        return StatusCode::BAD_REQUEST;
    }

    let id = {
        let mut next_miner_id = state.next_miner_id.write().await;
        let id = *next_miner_id;
        *next_miner_id = next_miner_id.saturating_add(1);
        id
    };

    let miner = Miner {
        id,
        start_date: req.start_date,
        end_date: req.end_date,
        t_shares: req.t_shares,
        status: None,
    };

    let miners_clone = {
        let mut miners = state.miners.write().await;
        miners.push(miner);
        miners.clone()
    };

    if let Ok(bytes) = serde_json::to_vec_pretty(&miners_clone) {
        let _ = atomic_write(&data_path("miners.json"), &bytes).await;
    }

    StatusCode::CREATED
}

async fn handle_end_miner(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IdRequest>,
) -> impl IntoResponse {
    let miners_clone = {
        let mut miners = state.miners.write().await;

        if let Some(miner) = miners.iter_mut().find(|m| m.id == req.id) {
            miner.status = Some("completed".to_string());
            miners.clone()
        } else {
            return StatusCode::BAD_REQUEST;
        }
    };

    if let Ok(bytes) = serde_json::to_vec_pretty(&miners_clone) {
        let _ = atomic_write(&data_path("miners.json"), &bytes).await;
    }

    StatusCode::OK
}

async fn handle_delete_miner(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IdRequest>,
) -> impl IntoResponse {
    let miners_clone = {
        let mut miners = state.miners.write().await;

        let before = miners.len();
        miners.retain(|m| m.id != req.id);

        if miners.len() == before {
            return StatusCode::BAD_REQUEST;
        }

        miners.clone()
    };

    if let Ok(bytes) = serde_json::to_vec_pretty(&miners_clone) {
        let _ = atomic_write(&data_path("miners.json"), &bytes).await;
    }

    StatusCode::OK
}

// =============================================
// MAIN & ROUTING
// =============================================

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tokio::fs::create_dir_all(DATA_DIR)
        .await
        .expect("Failed to create data dir");

    let initial_config =
        match tokio::fs::read_to_string(data_path("config.json")).await {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| default_config()),
            Err(_) => default_config(),
        };

    let initial_config = sanitize_config(initial_config);

    let initial_miners =
        match tokio::fs::read_to_string(data_path("miners.json")).await {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Vec::new(),
        };

    let (initial_miners, next_miner_id) = normalize_miners(initial_miners);

    if !initial_miners.is_empty() {
        if let Ok(bytes) = serde_json::to_vec_pretty(&initial_miners) {
            let _ = atomic_write(&data_path("miners.json"), &bytes).await;
        }
    }

    let (config_tx, _) = broadcast::channel(16);

    let state = Arc::new(AppState {
        live_data: RwLock::new(LiveData::default()),
        hex_json: RwLock::new(Arc::new(Vec::new())),
        miners: RwLock::new(initial_miners),
        config: RwLock::new(initial_config),
        config_tx,
        active_rpc_idx: RwLock::new(0),
        next_miner_id: RwLock::new(next_miner_id),
        hex_json_version: AtomicU64::new(1),
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .pool_max_idle_per_host(16)
        .user_agent("hexfetch/0.3")
        .build()
        .expect("Failed to build HTTP client");

    // Spawn background tasks
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
                            .header(header::CONTENT_TYPE, mime)
                            .header(
                                header::CACHE_CONTROL,
                                "public, max-age=3600",
                            )
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
