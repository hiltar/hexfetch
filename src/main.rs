use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::NaiveDate;
use reqwest::Client;
use ruint::aliases::U256;
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info};

// =============================================
// CONFIGURATION & CONSTANTS
// =============================================
const DATA_DIR: &str = "/opt/hexfetch";
const RPC_URL: &str = "https://rpc.pulsechain.com";
const HEX_CONTRACT: &str = "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";
const DEXSCREENER_URL: &str = "https://api.dexscreener.com/latest/dex/tokens/0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";

#[derive(Embed)]
#[folder = "static/"]
struct Assets;

// =============================================
// DATA STRUCTURES
// =============================================
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HexJsonEntry {
    #[serde(rename = "currentDay")]
    pub current_day: u64,
    #[serde(rename = "tshareRateHEX")]
    pub tshare_rate_hex: f64,
    #[serde(rename = "dailyPayoutHEX")]
    pub daily_payout_hex: f64,
    #[serde(rename = "payoutPerTshareHEX")]
    pub payout_per_tshare_hex: f64,
    #[serde(rename = "pricePulseX")]
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
}

// =============================================
// HELPERS & RPC LOGIC
// =============================================
fn parse_u256(hex_str: &str) -> U256 {
    let s = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    U256::from_str_radix(s, 16).unwrap_or(U256::ZERO)
}

fn u256_to_f64(u: U256) -> f64 {
    let limbs = u.as_limbs();
    let mut result: f64 = 0.0;
    for (i, &limb) in limbs.iter().enumerate() {
        result += (limb as f64) * (2.0_f64).powi(64 * i as i32);
    }
    result
}

async fn call_rpc(client: &Client, method: &str, params: serde_json::Value) -> Result<String, String> {
    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });

    let resp = client
        .post(RPC_URL)
        .json(&req_body)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())?;

    if let Some(err) = resp.get("error") {
        return Err(format!("RPC error: {}", err["message"].as_str().unwrap_or("unknown")));
    }

    Ok(resp["result"].as_str().unwrap_or("").to_string())
}

async fn fetch_live_data(client: &Client) -> Result<LiveData, String> {
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

    let gas_price_hex = call_rpc(client, "eth_gasPrice", serde_json::json!([])).await?;
    let gas_price_wei = parse_u256(&gas_price_hex);
    let beat = u256_to_f64(gas_price_wei) / 1e9;

    let globals_data = serde_json::json!([{"to": HEX_CONTRACT, "data": "0xc3124525"}, "latest"]);
    let globals_hex = call_rpc(client, "eth_call", globals_data).await?;
    let g_str = globals_hex.strip_prefix("0x").unwrap_or(&globals_hex);

    let mut tshare_rate = 0.0;
    let mut penalties = 0.0;
    let mut daily_data_count = U256::ZERO;

    if g_str.len() >= 320 {
        let share_rate = parse_u256(&g_str[128..192]);
        let penalty_total = parse_u256(&g_str[192..256]);
        daily_data_count = parse_u256(&g_str[256..320]);

        if share_rate > U256::ZERO {
            tshare_rate = u256_to_f64(share_rate) / 10.0;
        }
        penalties = u256_to_f64(penalty_total) / 1e8;
    }

    let mut payout_per_tshare = 0.0;
    if daily_data_count > U256::ZERO {
        let day_to_query = daily_data_count - U256::from(1);
        let day_padded = format!("0x90de6871{:064x}", day_to_query);
        let daily_data = serde_json::json!([{"to": HEX_CONTRACT, "data": day_padded}, "latest"]);
        
        if let Ok(daily_hex) = call_rpc(client, "eth_call", daily_data).await {
            let d_str = daily_hex.strip_prefix("0x").unwrap_or(&daily_hex);
            if d_str.len() >= 192 {
                let day_payout = parse_u256(&d_str[0..64]);
                let day_shares = parse_u256(&d_str[64..128]);
                
                if day_shares > U256::ZERO {
                    payout_per_tshare = (u256_to_f64(day_payout) / u256_to_f64(day_shares)) * 10000.0;
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
// BACKGROUND TASKS
// =============================================
async fn live_data_updater(state: Arc<AppState>, client: Client) {
    let mut rx = state.config_tx.subscribe();

    loop {
        let freq = state.config.read().await.live_data_frequency;
        let mut interval = tokio::time::interval(Duration::from_secs(freq * 60));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match fetch_live_data(&client).await {
                        Ok(data) => *state.live_data.write().await = data,
                        Err(e) => error!("Live data fetch failed: {}", e),
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
    let start = NaiveDate::parse_from_str(&miner.start_date, "%d-%m-%Y").ok();
    let end = NaiveDate::parse_from_str(&miner.end_date, "%d-%m-%Y").ok();

    if start.is_none() || end.is_none() || end < start || miner.t_shares <= 0.0 {
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
    tracing_subscriber::fmt::init();
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
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
        
    tokio::spawn(live_data_updater(state.clone(), client));

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
                    let mime = mime_guess::from_path(path).first_or_octet_stream();
                    Ok::<_, StatusCode>(axum::response::Response::builder()
                        .header("Content-Type", mime.as_ref())
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
