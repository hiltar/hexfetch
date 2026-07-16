use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{Utc, NaiveTime, Days};
use reqwest::Client;
use rust_embed::Embed;
use serde::{Deserialize, Deserializer, Serialize};
use std::{net::SocketAddr, sync::Arc, time::{Duration, Instant}};
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
const DEXSCREENER_URL: &str = "https://api.dexscreener.com/latest/dex/tokens/0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";
const HEX_JSON_URL: &str = "https://hexdailystats.com/fulldatapulsechain";

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
    if parts.next().is_some() { return None; }
    if d < 1 || d > 31 || m < 1 || m > 12 || y < 2000 || y > 2100 { return None; }
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
        if s.is_empty() { return Self::ZERO; }
        
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

    fn to_f64(&self) -> f64 {
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
        for i in 1..4 {
            let (res, overflow) = limbs[i].overflowing_sub(borrow);
            limbs[i] = res;
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
    
    // Get today's date at exactly 03:00:00 UTC
    let today_3am = now.date_naive().and_time(NaiveTime::from_hms_opt(3, 0, 0).unwrap());
    let mut next_3am = today_3am.and_utc();
    
    // If 3 AM UTC has already passed today, schedule for tomorrow
    if next_3am <= now {
        next_3am = next_3am.checked_add_days(Days::new(1)).unwrap();
    }
    
    // Convert chrono::Duration to std::time::Duration for tokio::time::sleep
    (next_3am - now).to_std().unwrap_or(std::time::Duration::from_secs(60))
}

// =============================================
// HELPERS & RPC LOGIC
// =============================================
async fn call_rpc(client: &Client, state: &Arc<AppState>, method: &str, params: serde_json::Value) -> Result<String, String> {
    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });

    let start_idx = *state.active_rpc_idx.read().await;
    let mut last_err = String::new();

    // Iterate through endpoints starting from the currently active one
    for i in 0..RPC_ENDPOINTS.len() {
        let idx = (start_idx + i) % RPC_ENDPOINTS.len();
        let url = RPC_ENDPOINTS[idx];

        match client.post(url).json(&req_body).send().await {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    if let Some(err) = json.get("error") {
                        last_err = format!("RPC error on {}: {}", url, err["message"].as_str().unwrap_or("unknown"));
                        continue;
                    }
                    
                    // If we successfully connected to an endpoint other than the start_idx, update the active index
                    if idx != start_idx {
                        info!("Successfully connected to fallback RPC: {} (index {})", url, idx);
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

async fn fetch_live_data(client: &Client, state: &Arc<AppState>) -> Result<LiveData, String> {
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

    let gas_price_hex = call_rpc(client, state, "eth_gasPrice", serde_json::json!([])).await?;
    let gas_price_wei = U256::from_hex(&gas_price_hex);
    let beat = gas_price_wei.to_f64() / 1e9;

    let globals_data = serde_json::json!([{"to": HEX_CONTRACT, "data": "0xc3124525"}, "latest"]);
    let globals_hex = call_rpc(client, state, "eth_call", globals_data).await?;
    let g_str = globals_hex.strip_prefix("0x").unwrap_or(&globals_hex);

    let mut tshare_rate = 0.0;
    let mut penalties = 0.0;
    let mut daily_data_count = U256::ZERO;

    if g_str.len() >= 320 {
        let share_rate = U256::from_hex(&g_str[128..192]);
        let penalty_total = U256::from_hex(&g_str[192..256]);
        daily_data_count = U256::from_hex(&g_str[256..320]);

        if share_rate > U256::ZERO {
            tshare_rate = share_rate.to_f64() / 10.0;
        }
        penalties = penalty_total.to_f64() / 1e8;
    }

    let mut payout_per_tshare = 0.0;
    if daily_data_count > U256::ZERO {
        let day_to_query = daily_data_count - 1;
        let day_padded = format!("0x90de6871{:x}", day_to_query);
        let daily_data = serde_json::json!([{"to": HEX_CONTRACT, "data": day_padded}, "latest"]);
        
        if let Ok(daily_hex) = call_rpc(client, state, "eth_call", daily_data).await {
            let d_str = daily_hex.strip_prefix("0x").unwrap_or(&daily_hex);
            if d_str.len() >= 192 {
                let day_payout = U256::from_hex(&d_str[0..64]);
                let day_shares = U256::from_hex(&d_str[64..128]);
                
                if day_shares > U256::ZERO {
                    payout_per_tshare = (day_payout.to_f64() / day_shares.to_f64()) * 10000.0;
                }
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
async fn fetch_live_data_with_retry(client: &Client, state: &Arc<AppState>) -> Result<LiveData, String> {
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
// HEXJSON FETCHING & MERGING
// =============================================
async fn fetch_hex_json(client: &Client) -> Result<Vec<HexJsonEntry>, String> {
    let resp = client
        .get(HEX_JSON_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?;
        
    if !resp.status().is_success() {
        return Err(format!("Status {}", resp.status()));
    }
    
    let text = resp.text().await.map_err(|e| e.to_string())?;
    
    serde_json::from_str::<Vec<HexJsonEntry>>(&text).map_err(|e| {
        let snippet: String = text.chars().take(300).collect();
        format!("JSON decode error: {}. Snippet: {}", e, snippet)
    })
}

async fn fetch_hex_json_with_retry(client: &Client) -> Result<Vec<HexJsonEntry>, String> {
    let mut delay = Duration::from_secs(1);
    let max_delay = Duration::from_secs(60);
    let start = Instant::now();
    let max_elapsed = Duration::from_secs(5 * 60);

    loop {
        match fetch_hex_json(client).await {
            Ok(data) => return Ok(data),
            Err(e) => {
                if start.elapsed() > max_elapsed {
                    return Err(format!("Max elapsed time reached: {}", e));
                }
                error!("HEXJSON fetch error: {}. Retrying in {:?}...", e, delay);
                tokio::time::sleep(delay).await;
                delay = std::cmp::min(delay * 2, max_delay);
            }
        }
    }
}

async fn update_local_hex_json(state: Arc<AppState>, client: &Client) {
    match fetch_hex_json_with_retry(client).await {
        Ok(remote) => {
            let mut local = state.hex_json.write().await;
            let local_max = local.iter().map(|e| e.current_day).max().unwrap_or(0);
            
            let mut new_entries: Vec<HexJsonEntry> = remote
                .into_iter()
                .filter(|e| e.current_day > local_max)
                .collect();
                
            if !new_entries.is_empty() || local.is_empty() {
                new_entries.extend(local.clone());
                new_entries.sort_by_key(|e| e.current_day);
                *local = new_entries;
                info!("HEXJSON updated. Total entries: {}", local.len());
            }
        }
        Err(e) => {
            error!("HEXJSON fetch failed after retries: {}", e);
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

async fn hex_json_updater(state: Arc<AppState>, client: Client) {
    loop {
        let sleep_duration = get_duration_until_next_3am_utc();        
        tokio::time::sleep(sleep_duration).await;
        info!("Running daily HEXJSON update at 3:00 AM UTC...");
        update_local_hex_json(state.clone(), &client).await;
    }
}

async fn test_rpc(client: &Client, url: &str) -> bool {
    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_chainId",
        "params": [],
        "id": 1
    });

    match client.post(url).json(&req_body).send().await {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => json.get("error").is_none(),
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

async fn rpc_health_checker(state: Arc<AppState>, client: Client) {
    loop {
        // Wait 24 hours before checking
        tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        
        let current_idx = *state.active_rpc_idx.read().await;
        if current_idx != 0 {
            info!("24h RPC health check: Testing primary RPC endpoint ({})...", RPC_ENDPOINTS[0]);
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
    tokio::fs::create_dir_all(DATA_DIR).await.expect("Failed to create data dir");

    let initial_config = match tokio::fs::read_to_string(format!("{}/config.json", DATA_DIR)).await {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| Config {
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

    let initial_miners = match tokio::fs::read_to_string(format!("{}/miners.json", DATA_DIR)).await {
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
        active_rpc_idx: RwLock::new(0), // Initialize to first endpoint
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
        
    let client_for_hex = client.clone();
    let state_for_hex = state.clone();
    tokio::spawn(async move {
        info!("Fetching initial HEXJSON data...");
        update_local_hex_json(state_for_hex, &client_for_hex).await;
    });

    tokio::spawn(live_data_updater(state.clone(), client.clone()));
    tokio::spawn(hex_json_updater(state.clone(), client.clone()));
    tokio::spawn(rpc_health_checker(state.clone(), client));

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
                    Ok::<_, StatusCode>(axum::response::Response::builder()
                        .header("Content-Type", mime)
                        .body(axum::body::Body::from(content.data.into_owned()))
                        .unwrap())
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

    // ==========================================
    // TEST HELPER: Create a mock AppState
    // ==========================================
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

    // ==========================================
    // 1. UNIT TESTS: UTILS & PARSING
    // ==========================================

    #[test]
    fn test_parse_date_valid() {
        // Test standard valid dates
        assert_eq!(parse_date("15-08-2023"), Some((2023, 8, 15)));
        assert_eq!(parse_date("01-01-2000"), Some((2000, 1, 1)));
    }

    #[test]
    fn test_parse_date_invalid() {
        // Test bounds and malformed strings
        assert_eq!(parse_date("32-01-2020"), None); // Day > 31
        assert_eq!(parse_date("15-13-2020"), None); // Month > 12
        assert_eq!(parse_date("15-08-1999"), None); // Year < 2000
        assert_eq!(parse_date("invalid-date"), None);
        assert_eq!(parse_date("15-08-2023-extra"), None); // Too many parts
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
        // 0x400 is 1024 in decimal
        let val = U256::from_hex("0x400");
        assert_eq!(val.to_f64(), 1024.0);

        // Test subtraction (Crucial for your daily_data_count - 1 logic)
        let res = val - 24;
        assert_eq!(res.to_f64(), 1000.0);

        // Test large hex (simulating T-Share rate or wei)
        let large_val = U256::from_hex("0xDE0B6B3A7640000"); // 1 ETH in wei (1e18)
        assert!(large_val.to_f64() > 1e17);
    }

    // ==========================================
    // 2. INTEGRATION TESTS: API HANDLERS
    // ==========================================
    
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
        
        // The handler returns `impl IntoResponse`, so we convert it to check the HTTP status
        let status = response.into_response().status();
        assert_eq!(status, StatusCode::CREATED);
        
        // Verify the in-memory state was updated
        let miners = state.miners.read().await;
        assert_eq!(miners.len(), 1);
        assert_eq!(miners[0].t_shares, 100.0);
    }

    #[tokio::test]
    async fn test_handle_add_miner_invalid_date() {
        let state = create_test_state();
        let miner = Miner {
            start_date: "32-01-2023".to_string(), // Invalid day
            end_date: "01-01-2024".to_string(),
            t_shares: 100.0,
            status: None,
        };
        
        let response = handle_add_miner(State(state.clone()), Json(miner)).await;
        assert_eq!(response.into_response().status(), StatusCode::BAD_REQUEST);
        
        // Verify state was NOT updated because validation failed
        assert_eq!(state.miners.read().await.len(), 0);
    }

    #[tokio::test]
    async fn test_handle_add_miner_end_before_start() {
        let state = create_test_state();
        let miner = Miner {
            start_date: "01-01-2024".to_string(),
            end_date: "01-01-2023".to_string(), // End before start
            t_shares: 100.0,
            status: None,
        };
        
        let response = handle_add_miner(State(state.clone()), Json(miner)).await;
        assert_eq!(response.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_handle_delete_miner() {
        let state = create_test_state();
        
        // Pre-populate state with a miner
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
        
        // Verify it was removed
        assert_eq!(state.miners.read().await.len(), 0);
    }

    #[tokio::test]
    async fn test_handle_delete_miner_out_of_bounds() {
        let state = create_test_state();
        let req = IndexRequest { index: 99 }; // No miners exist, so 99 is invalid
        
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
        
        // Verify state update
        let config = state.config.read().await;
        assert_eq!(config.live_data_frequency, 5);
        assert_eq!(config.liquid_hex, 1000.0);
    }
}
