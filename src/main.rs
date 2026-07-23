use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{Days, NaiveTime, Utc};
use reqwest::Client;
use rust_embed::Embed;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, RwLock};

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
const DEXSCREENER_URL: &str =
    "https://api.dexscreener.com/latest/dex/tokens/0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";

/// CoinGecko – HEX on PulseChain (per-day historical prices)
const COINGECKO_URL: &str =
    "https://api.coingecko.com/api/v3/coins/hex-pulsechain/market_chart?vs_currency=usd&days=365&interval=daily";

/// HEXDailyStats – used ONCE during initial backfill for historical T-Share rates.
/// After the first successful backfill the system never contacts this endpoint again.
const HEXDAILYSTATS_URL: &str = "https://hexdailystats.com/fulldatapulsechain";

/// globals() selector: keccak256("globals()")[0..4]
const GLOBALS_SELECTOR: &str = "0xc3124525";
/// dailyData(uint256) selector: keccak256("dailyData(uint256)")[0..4]
const DAILY_DATA_SELECTOR: &str = "0x90de6871";

/// 1 HEX = 10^8 hearts
const HEARTS_PER_HEX: f64 = 1e8;
/// T-Share precision factor used in payout calculation
const TSHARE_UNIT: f64 = 10000.0;

/// HEX launch: 2019-12-02 00:00:00 UTC (day 0)
const HEX_LAUNCH_TS: i64 = 1_575_244_800;

/// Delay between RPC calls during backfill to avoid rate-limiting
const BACKFILL_DELAY_MS: u64 = 60;
/// Save progress to disk every N days during backfill
const BACKFILL_SAVE_INTERVAL: usize = 200;

#[derive(Embed)]
#[folder = "static/"]
struct Assets;

// =============================================
// NATIVE LOGGING MACROS
// =============================================
macro_rules! info {
    ($($arg:tt)*) => { println!("[INFO] {}", format!($($arg)*)) }
}
macro_rules! error {
    ($($arg:tt)*) => { eprintln!("[ERROR] {}", format!($($arg)*)) }
}
macro_rules! warn {
    ($($arg:tt)*) => { eprintln!("[WARN] {}", format!($($arg)*)) }
}

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
    #[serde(rename = "startDate")]
    pub start_date: String,
    #[serde(rename = "endDate")]
    pub end_date: String,
    #[serde(rename = "tShares")]
    pub t_shares: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
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
struct IndexRequest {
    index: usize,
}

// =============================================
// APPLICATION STATE
// =============================================
struct AppState {
    live_data: RwLock<LiveData>,
    hex_json: RwLock<Vec<HexJsonEntry>>,
    miners: RwLock<Vec<Miner>>,
    config: RwLock<Config>,
    config_tx: broadcast::Sender<()>,
    active_rpc_idx: RwLock<usize>,
}

// =============================================
// 1. CUSTOM DATE PARSER
// =============================================
fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let mut parts = s.split('-');
    let d: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let y: i32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if !(1..=31).contains(&d) || !(1..=12).contains(&m) || !(2000..=2100).contains(&y) {
        return None;
    }
    Some((y, m, d))
}

// =============================================
// 2. CUSTOM MIME GUESSER
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
// 3. CUSTOM U256
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
        for (i, &limb) in self.0.iter().enumerate() {
            if limb != 0 {
                result += (limb as f64) * (2.0_f64).powi(64 * i as i32);
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
// 4. UTC TIME CALCULATION
// =============================================
fn get_duration_until_next_3am_utc() -> std::time::Duration {
    let now = Utc::now();
    let today_3am = now
        .date_naive()
        .and_time(NaiveTime::from_hms_opt(3, 0, 0).unwrap());
    let mut next_3am = today_3am.and_utc();
    if next_3am <= now {
        next_3am = next_3am.checked_add_days(Days::new(1)).unwrap();
    }
    (next_3am - now)
        .to_std()
        .unwrap_or(std::time::Duration::from_secs(60))
}

// =============================================
// HELPERS & RPC LOGIC
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
            Ok(resp) => match resp.json::<serde_json::Value>().await {
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
            },
            Err(e) => {
                last_err = format!("Network error on {}: {}", url, e);
                continue;
            }
        }
    }
    Err(format!("All RPC endpoints failed. Last error: {}", last_err))
}

// =============================================
// 5. ON-CHAIN DATA READING
// =============================================

/// Reads globals() → (tshare_rate_hex, daily_data_count, penalties_hex)
async fn read_globals(
    client: &Client,
    state: &Arc<AppState>,
) -> Result<(f64, u64, f64), String> {
    let params = serde_json::json!([{"to": HEX_CONTRACT, "data": GLOBALS_SELECTOR}, "latest"]);
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
    let params = serde_json::json!([{"to": HEX_CONTRACT, "data": call_data}, "latest"]);
    let hex_result = call_rpc(client, state, "eth_call", params).await?;
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

// =============================================
// 6. EXTERNAL DATA SOURCES
// =============================================

/// Current price from DEXScreener (single value, used for live data & fallback).
async fn fetch_price_dexscreener(client: &Client) -> Result<f64, String> {
    let resp: serde_json::Value = client
        .get(DEXSCREENER_URL)
        .send()
        .await
        .map_err(|e| format!("DEXScreener request failed: {}", e))?
        .json()
        .await
        .map_err(|e| format!("DEXScreener JSON parse failed: {}", e))?;

    let price = resp["pairs"][0]["priceUsd"]
        .as_f64()
        .or_else(|| resp["pairs"][0]["priceUsd"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0);

    if price <= 0.0 {
        return Err("DEXScreener returned zero or invalid price".to_string());
    }
    Ok(price)
}

/// Historical per-day prices from CoinGecko (hex-pulsechain, last 365 days).
/// Returns HashMap<hex_day, price_usd>. Empty map on any failure.
async fn fetch_price_history_coingecko(client: &Client) -> HashMap<u64, f64> {
    let resp: serde_json::Value = match client.get(COINGECKO_URL).send().await {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                warn!("CoinGecko JSON parse failed: {}", e);
                return HashMap::new();
            }
        },
        Err(e) => {
            warn!("CoinGecko request failed: {}", e);
            return HashMap::new();
        }
    };

    // Check for CoinGecko API-level errors (rate limit, time range, etc.)
    if let Some(err) = resp.get("error").or_else(|| resp.get("status")) {
        let msg = err["error_message"]
            .as_str()
            .or_else(|| err["message"].as_str())
            .unwrap_or("unknown error");
        warn!("CoinGecko API error: {}", msg);
        return HashMap::new();
    }

    let prices = match resp["prices"].as_array() {
        Some(p) => p,
        None => {
            warn!("CoinGecko response missing 'prices' array");
            return HashMap::new();
        }
    };

    let mut map: HashMap<u64, f64> = HashMap::with_capacity(prices.len());
    for entry in prices {
        if let (Some(ts_ms), Some(price)) = (entry[0].as_f64(), entry[1].as_f64()) {
            let ts_secs = (ts_ms / 1000.0) as i64;
            let day = (ts_secs - HEX_LAUNCH_TS) / 86_400;
            if day >= 0 {
                map.insert(day as u64, price);
            }
        }
    }

    if map.is_empty() {
        warn!("CoinGecko returned no usable price points");
    } else {
        info!(
            "CoinGecko: loaded {} daily price points (days {}–{})",
            map.len(),
            map.keys().min().unwrap_or(&0),
            map.keys().max().unwrap_or(&0),
        );
    }
    map
}

/// ONE-TIME fetch of historical T-Share rates (and prices) from HEXDailyStats.
/// Returns (tshare_map, price_map). Both empty on any failure.
/// After the initial backfill this endpoint is never contacted again.
async fn fetch_hexdailystats_backfill(
    client: &Client,
) -> (HashMap<u64, f64>, HashMap<u64, f64>) {
    info!("Attempting one-time historical fetch from HEXDailyStats...");

    let resp = match client.get(HEXDAILYSTATS_URL).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!(
                "HEXDailyStats unreachable: {}. Will use CoinGecko prices + current T-Share rate.",
                e
            );
            return (HashMap::new(), HashMap::new());
        }
    };

    if !resp.status().is_success() {
        warn!(
            "HEXDailyStats returned HTTP {}. Will use CoinGecko prices + current T-Share rate.",
            resp.status()
        );
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
        if entry.tshare_rate_hex > 0.0 {
            tshare_map.insert(entry.current_day, entry.tshare_rate_hex);
        }
        if entry.price_pulse_x > 0.0 {
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
// 7. HEXJSON PERSISTENCE
// =============================================

fn hexjson_file_path() -> String {
    format!("{}/hexjson.json", DATA_DIR)
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

// =============================================
// 8. BACKFILL: BUILD HEXJSON FROM MULTIPLE SOURCES
// =============================================
//
// Data-source priority per field:
//   dailyPayoutHEX      → RPC dailyData(day)        (always, immutable)
//   payoutPerTshareHEX  → RPC dailyData(day)        (always, immutable)
//   pricePulseX         → CoinGecko hex-pulsechain  (per-day)
//                         → HEXDailyStats            (per-day, fallback)
//                         → DEXScreener current      (flat, last resort)
//   tshareRateHEX       → HEXDailyStats             (per-day, one-time)
//                         → globals() current        (flat, fallback)
//
async fn backfill_hex_json(
    client: &Client,
    state: &Arc<AppState>,
    existing_data: &[HexJsonEntry],
) -> Vec<HexJsonEntry> {
    info!("Starting HEXJSON backfill...");

    // ── 1. On-chain globals ──────────────────────────────────────────
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

    // ── 2. CoinGecko historical prices (primary price source) ────────
    let cg_prices = fetch_price_history_coingecko(client).await;

    // ── 3. HEXDailyStats one-time fetch (T-Share rates + backup prices)
    let (hds_tshares, hds_prices) = fetch_hexdailystats_backfill(client).await;

    // ── 4. DEXScreener current price (last-resort fallback) ──────────
    let fallback_price = if cg_prices.is_empty() && hds_prices.is_empty() {
        match fetch_price_dexscreener(client).await {
            Ok(p) => {
                info!("Using DEXScreener current price as flat fallback: ${:.8}", p);
                p
            }
            Err(e) => {
                warn!("DEXScreener fallback also failed: {}. Using 0.0.", e);
                0.0
            }
        }
    } else {
        0.0
    };

    // ── 5. Determine which days to fetch ─────────────────────────────
    let existing_max_day = existing_data.iter().map(|e| e.current_day).max().unwrap_or(0);
    let start_day = if existing_max_day > 0 {
        existing_max_day + 1
    } else {
        1
    };

    if start_day >= day_count {
        info!(
            "HEXJSON already up to date (max day {} >= count {}).",
            existing_max_day, day_count
        );
        return existing_data.to_vec();
    }

    let total_to_fetch = day_count - start_day;
    info!(
        "Backfilling days {} to {} ({} entries)...",
        start_day,
        day_count - 1,
        total_to_fetch
    );

    // ── 6. Iterate through each day ──────────────────────────────────
    let mut new_entries: Vec<HexJsonEntry> = Vec::with_capacity(total_to_fetch as usize);
    let mut consecutive_errors = 0u32;
    let max_consecutive_errors = 50u32;

    for day in start_day..day_count {
        match read_daily_data(client, state, day).await {
            Ok((payout_hearts, shares)) => {
                consecutive_errors = 0;

                let daily_payout_hex = payout_hearts / HEARTS_PER_HEX;
                let payout_per_tshare = if shares > 0.0 {
                    (payout_hearts / shares) * TSHARE_UNIT
                } else {
                    0.0
                };

                // Skip empty pre-staking days
                if daily_payout_hex == 0.0 && shares == 0.0 && day < 10 {
                    continue;
                }

                // Price: CoinGecko → HEXDailyStats → DEXScreener flat
                let price = cg_prices
                    .get(&day)
                    .copied()
                    .or_else(|| hds_prices.get(&day).copied())
                    .unwrap_or(fallback_price);

                // T-Share rate: HEXDailyStats → current globals()
                let tshare = hds_tshares
                    .get(&day)
                    .copied()
                    .unwrap_or(current_tshare_rate);

                new_entries.push(HexJsonEntry {
                    current_day: day,
                    tshare_rate_hex: tshare,
                    daily_payout_hex,
                    payout_per_tshare_hex: payout_per_tshare,
                    price_pulse_x: price,
                });
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
                if consecutive_errors <= 3 || consecutive_errors.is_multiple_of(20) {
                    warn!("Backfill: error reading day {}: {}", day, e);
                }
            }
        }

        // Progress logging
        let fetched = (day - start_day + 1) as usize;
        if fetched.is_multiple_of(100) || fetched == total_to_fetch as usize {
            info!(
                "Backfill progress: {}/{} days ({:.1}%)",
                fetched,
                total_to_fetch,
                (fetched as f64 / total_to_fetch as f64) * 100.0
            );
        }

        // Periodic save
        if fetched.is_multiple_of(BACKFILL_SAVE_INTERVAL) && !new_entries.is_empty() {
            let mut partial = existing_data.to_vec();
            partial.extend(new_entries.clone());
            partial.sort_by_key(|e| e.current_day);
            save_hex_json_to_file(&partial).await;
            info!("Backfill: intermediate save ({} total entries)", partial.len());
        }

        // Rate-limit delay
        tokio::time::sleep(Duration::from_millis(BACKFILL_DELAY_MS)).await;
    }

    // ── 7. Merge ─────────────────────────────────────────────────────
    let mut result = existing_data.to_vec();
    result.extend(new_entries);
    result.sort_by_key(|e| e.current_day);
    result.dedup_by_key(|e| e.current_day);

    info!("Backfill complete. Total HEXJSON entries: {}", result.len());
    result
}

// =============================================
// 9. DAILY RECORDING (GOING FORWARD)
// =============================================

/// Records one day's data using live RPC + DEXScreener values.
/// Called daily at 3 AM UTC. No external HEXJSON API needed.
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
    let payout_per_tshare = if shares > 0.0 {
        (payout_hearts / shares) * TSHARE_UNIT
    } else {
        0.0
    };

    let price = fetch_price_dexscreener(client).await.unwrap_or(0.0);

    Ok(HexJsonEntry {
        current_day: target_day,
        tshare_rate_hex: tshare_rate,
        daily_payout_hex,
        payout_per_tshare_hex: payout_per_tshare,
        price_pulse_x: price,
    })
}

// =============================================
// 10. LIVE DATA FETCHING
// =============================================
async fn fetch_live_data(
    client: &Client,
    state: &Arc<AppState>,
) -> Result<LiveData, String> {
    // DEXScreener → current price
    let dex_resp: serde_json::Value = client
        .get(DEXSCREENER_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let price = dex_resp["pairs"][0]["priceUsd"]
        .as_f64()
        .or_else(|| dex_resp["pairs"][0]["priceUsd"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0);

    // RPC → gas price (beat)
    let gas_price_hex = call_rpc(client, state, "eth_gasPrice", serde_json::json!([])).await?;
    let gas_price_wei = U256::from_hex(&gas_price_hex);
    let beat = gas_price_wei.to_f64() / 1e9;

    // RPC → globals (tshare rate, penalties, day count)
    let (tshare_rate, daily_data_count, penalties) = read_globals(client, state).await?;

    // RPC → dailyData for latest day (payout per tshare)
    let mut payout_per_tshare = 0.0;
    if daily_data_count > 0 {
        let day_to_query = daily_data_count - 1;
        if let Ok((payout_hearts, shares)) = read_daily_data(client, state, day_to_query).await {
            if shares > 0.0 {
                payout_per_tshare = (payout_hearts / shares) * TSHARE_UNIT;
            }
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

// =============================================
// LIVE DATA WITH RETRY
// =============================================
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
        let freq = state.config.read().await.live_data_frequency;
        let mut interval = tokio::time::interval(Duration::from_secs(freq * 60));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match fetch_live_data_with_retry(&client, &state).await {
                        Ok(data) => *state.live_data.write().await = data,
                        Err(e) => error!("Background live data fetch failed: {}", e),
                    }
                }
                _ = rx.recv() => {
                    info!("Config changed, restarting updater loop...");
                    break;
                }
            }
        }
    }
}

/// HEXJSON updater: backfills on startup, then records daily at 3 AM UTC.
/// Fully self-contained after initial backfill — no continuous external API dependency.
async fn hex_json_updater(state: Arc<AppState>, client: Client) {
    // ── Phase 1: Initial load or backfill ────────────────────────────
    let file_data = load_hex_json_from_file().await;

    let initial_data = if file_data.is_empty() {
        info!("No persisted HEXJSON data. Building from RPC + external sources (this may take several minutes)...");
        let backfilled = backfill_hex_json(&client, &state, &[]).await;
        save_hex_json_to_file(&backfilled).await;
        backfilled
    } else {
        // Check if we need to catch up (program was offline for days)
        let max_day_in_file = file_data.iter().map(|e| e.current_day).max().unwrap_or(0);
        match read_globals(&client, &state).await {
            Ok((_, day_count, _)) => {
                if day_count > max_day_in_file + 1 {
                    info!(
                        "HEXJSON behind (file max={}, chain count={}). Catching up...",
                        max_day_in_file, day_count
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

    // Store in memory
    {
        let mut hex_json = state.hex_json.write().await;
        *hex_json = initial_data;
        info!("HEXJSON loaded into memory: {} entries", hex_json.len());
    }

    // ── Phase 2: Daily recording loop at 3 AM UTC ────────────────────
    loop {
        let sleep_duration = get_duration_until_next_3am_utc();
        info!(
            "HEXJSON updater sleeping for {:?} until next 3 AM UTC recording...",
            sleep_duration
        );
        tokio::time::sleep(sleep_duration).await;

        info!("Running daily HEXJSON recording...");

        let mut delay = Duration::from_secs(5);
        let max_retries = 10;
        let mut recorded = false;

        for attempt in 1..=max_retries {
            match record_daily_entry(&client, &state).await {
                Ok(entry) => {
                    let mut hex_json = state.hex_json.write().await;

                    if hex_json.iter().any(|e| e.current_day == entry.current_day) {
                        info!("Day {} already recorded. Skipping.", entry.current_day);
                    } else {
                        info!(
                            "Recorded day {}: payout={:.2} HEX, payout/tshare={:.4}, tshareRate={:.1}, price=${:.8}",
                            entry.current_day,
                            entry.daily_payout_hex,
                            entry.payout_per_tshare_hex,
                            entry.tshare_rate_hex,
                            entry.price_pulse_x
                        );
                        hex_json.push(entry);
                        hex_json.sort_by_key(|e| e.current_day);
                    }

                    let data_clone = hex_json.clone();
                    drop(hex_json);
                    save_hex_json_to_file(&data_clone).await;
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

async fn handle_hex_json(State(state): State<Arc<AppState>>) -> Json<Vec<HexJsonEntry>> {
    Json(state.hex_json.read().await.clone())
}

async fn handle_get_config(State(state): State<Arc<AppState>>) -> Json<Config> {
    Json(state.config.read().await.clone())
}

async fn handle_post_config(
    State(state): State<Arc<AppState>>,
    Json(new_config): Json<Config>,
) -> impl IntoResponse {
    let mut config = state.config.write().await;
    *config = new_config;
    let cfg_clone = config.clone();
    drop(config);

    let _ = state.config_tx.send(());

    let path = format!("{}/config.json", DATA_DIR);
    if let Ok(json) = serde_json::to_string_pretty(&cfg_clone) {
        let _ = tokio::fs::write(path, json).await;
    }
    StatusCode::OK
}

async fn handle_add_miner(
    State(state): State<Arc<AppState>>,
    Json(miner): Json<Miner>,
) -> impl IntoResponse {
    let start = parse_date(&miner.start_date);
    let end = parse_date(&miner.end_date);
    if start.is_none() || end.is_none() || end.unwrap() < start.unwrap() || miner.t_shares <= 0.0 {
        return StatusCode::BAD_REQUEST;
    }

    let mut miners = state.miners.write().await;
    miners.push(miner);
    let miners_clone = miners.clone();
    drop(miners);

    let path = format!("{}/miners.json", DATA_DIR);
    if let Ok(json) = serde_json::to_string_pretty(&miners_clone) {
        let _ = tokio::fs::write(path, json).await;
    }
    StatusCode::CREATED
}

async fn handle_end_miner(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IndexRequest>,
) -> impl IntoResponse {
    let mut miners = state.miners.write().await;
    if req.index < miners.len() {
        miners[req.index].status = Some("completed".to_string());
        let miners_clone = miners.clone();
        drop(miners);

        let path = format!("{}/miners.json", DATA_DIR);
        if let Ok(json) = serde_json::to_string_pretty(&miners_clone) {
            let _ = tokio::fs::write(path, json).await;
        }
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    }
}

async fn handle_delete_miner(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IndexRequest>,
) -> impl IntoResponse {
    let mut miners = state.miners.write().await;
    if req.index < miners.len() {
        miners.remove(req.index);
        let miners_clone = miners.clone();
        drop(miners);

        let path = format!("{}/miners.json", DATA_DIR);
        if let Ok(json) = serde_json::to_string_pretty(&miners_clone) {
            let _ = tokio::fs::write(path, json).await;
        }
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    }
}

// =============================================
// MAIN & ROUTING
// =============================================
#[tokio::main]
async fn main() {
    tokio::fs::create_dir_all(DATA_DIR)
        .await
        .expect("Failed to create data dir");

    let initial_config =
        match tokio::fs::read_to_string(format!("{}/config.json", DATA_DIR)).await {
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

    let initial_miners =
        match tokio::fs::read_to_string(format!("{}/miners.json", DATA_DIR)).await {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Vec::new(),
        };

    let (config_tx, _) = broadcast::channel(16);

    let state = Arc::new(AppState {
        live_data: RwLock::new(LiveData::default()),
        hex_json: RwLock::new(Vec::new()),
        miners: RwLock::new(initial_miners),
        config: RwLock::new(initial_config),
        config_tx,
        active_rpc_idx: RwLock::new(0),
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

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
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 5555));
    info!("⬢ HEX Stats server starting on {} ⬢", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

// ==========================================
// TEST UNITS
// ==========================================
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use tokio::sync::broadcast;

    fn create_test_state() -> Arc<AppState> {
        let (config_tx, _) = broadcast::channel(16);
        Arc::new(AppState {
            live_data: RwLock::new(LiveData::default()),
            hex_json: RwLock::new(Vec::new()),
            miners: RwLock::new(Vec::new()),
            config: RwLock::new(Config {
                live_data_frequency: 15,
                liquid_hex: 0.0,
                historical_start_day: 1260,
            }),
            config_tx,
            active_rpc_idx: RwLock::new(0),
        })
    }

    #[test]
    fn test_parse_date_valid() {
        assert_eq!(parse_date("15-08-2023"), Some((2023, 8, 15)));
        assert_eq!(parse_date("01-01-2000"), Some((2000, 1, 1)));
    }

    #[test]
    fn test_parse_date_invalid() {
        assert_eq!(parse_date("32-01-2020"), None);
        assert_eq!(parse_date("15-13-2020"), None);
        assert_eq!(parse_date("15-08-1999"), None);
        assert_eq!(parse_date("invalid-date"), None);
        assert_eq!(parse_date("15-08-2023-extra"), None);
    }

    #[test]
    fn test_get_mime_type() {
        assert_eq!(get_mime_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(get_mime_type("app.js"), "application/javascript");
        assert_eq!(get_mime_type("style.css"), "text/css");
        assert_eq!(get_mime_type("data.json"), "application/json");
        assert_eq!(get_mime_type("unknown.xyz"), "application/octet-stream");
        assert_eq!(get_mime_type("no_extension"), "application/octet-stream");
    }

    #[test]
    fn test_u256_from_hex_and_math() {
        let val = U256::from_hex("0x400");
        assert_eq!(val.to_f64(), 1024.0);

        let res = val - 24;
        assert_eq!(res.to_f64(), 1000.0);

        let large_val = U256::from_hex("0xDE0B6B3A7640000");
        assert!(large_val.to_f64() > 1e17);
    }

    #[test]
    fn test_daily_data_call_encoding() {
        let day: u64 = 1500;
        let call_data = format!("{}{:064x}", DAILY_DATA_SELECTOR, day);
        assert_eq!(call_data.len(), 2 + 8 + 64);
        assert!(call_data.starts_with("0x90de6871"));
        assert!(call_data.ends_with("5dc"));
    }

    #[test]
    fn test_payout_calculation() {
        let payout_hearts = 1_000_000.0 * HEARTS_PER_HEX;
        let shares = 500_000.0;
        let payout_per_tshare = (payout_hearts / shares) * TSHARE_UNIT;
        assert!(payout_per_tshare > 0.0);

        let daily_payout_hex = payout_hearts / HEARTS_PER_HEX;
        assert_eq!(daily_payout_hex, 1_000_000.0);
    }

    #[test]
    fn test_hex_day_to_timestamp() {
        // Day 0 = 2019-12-02
        assert_eq!(HEX_LAUNCH_TS, 1_575_244_800);
        // Day 1260 ≈ 2023-05-15 (around PulseChain launch)
        let day_1260_ts = HEX_LAUNCH_TS + 1260 * 86_400;
        assert!(day_1260_ts > 1_684_000_000); // After May 2023
    }

    #[test]
    fn test_hexjson_entry_serialization() {
        let entry = HexJsonEntry {
            current_day: 1500,
            tshare_rate_hex: 12345.6,
            daily_payout_hex: 789.01,
            payout_per_tshare_hex: 0.005,
            price_pulse_x: 0.00012,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"currentDay\":1500"));
        assert!(json.contains("\"tshareRateHEX\":"));
        assert!(json.contains("\"dailyPayoutHEX\":"));
        assert!(json.contains("\"payoutPerTshareHEX\":"));
        assert!(json.contains("\"pricePulseX\":"));

        let parsed: HexJsonEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.current_day, 1500);
    }

    #[tokio::test]
    async fn test_handle_add_miner_success() {
        let state = create_test_state();
        let miner = Miner {
            start_date: "01-01-2023".to_string(),
            end_date: "01-01-2024".to_string(),
            t_shares: 100.0,
            status: None,
        };
        let response = handle_add_miner(State(state.clone()), Json(miner)).await;
        let status = response.into_response().status();
        assert_eq!(status, StatusCode::CREATED);
        let miners = state.miners.read().await;
        assert_eq!(miners.len(), 1);
        assert_eq!(miners[0].t_shares, 100.0);
    }

    #[tokio::test]
    async fn test_handle_add_miner_invalid_date() {
        let state = create_test_state();
        let miner = Miner {
            start_date: "32-01-2023".to_string(),
            end_date: "01-01-2024".to_string(),
            t_shares: 100.0,
            status: None,
        };
        let response = handle_add_miner(State(state.clone()), Json(miner)).await;
        assert_eq!(response.into_response().status(), StatusCode::BAD_REQUEST);
        assert_eq!(state.miners.read().await.len(), 0);
    }

    #[tokio::test]
    async fn test_handle_add_miner_end_before_start() {
        let state = create_test_state();
        let miner = Miner {
            start_date: "01-01-2024".to_string(),
            end_date: "01-01-2023".to_string(),
            t_shares: 100.0,
            status: None,
        };
        let response = handle_add_miner(State(state.clone()), Json(miner)).await;
        assert_eq!(response.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_handle_delete_miner() {
        let state = create_test_state();
        {
            let mut miners = state.miners.write().await;
            miners.push(Miner {
                start_date: "01-01-2023".into(),
                end_date: "01-01-2024".into(),
                t_shares: 10.0,
                status: None,
            });
        }
        let req = IndexRequest { index: 0 };
        let response = handle_delete_miner(State(state.clone()), Json(req)).await;
        assert_eq!(response.into_response().status(), StatusCode::OK);
        assert_eq!(state.miners.read().await.len(), 0);
    }

    #[tokio::test]
    async fn test_handle_delete_miner_out_of_bounds() {
        let state = create_test_state();
        let req = IndexRequest { index: 99 };
        let response = handle_delete_miner(State(state.clone()), Json(req)).await;
        assert_eq!(response.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_handle_post_config() {
        let state = create_test_state();
        let new_config = Config {
            live_data_frequency: 5,
            liquid_hex: 1000.0,
            historical_start_day: 1000,
        };
        let response = handle_post_config(State(state.clone()), Json(new_config)).await;
        assert_eq!(response.into_response().status(), StatusCode::OK);
        let config = state.config.read().await;
        assert_eq!(config.live_data_frequency, 5);
        assert_eq!(config.liquid_hex, 1000.0);
    }
}
