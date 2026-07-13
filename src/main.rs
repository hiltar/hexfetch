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
use tokio::sync::{broadcast, RwLock};
use std::{net::SocketAddr, sync::Arc, time::Duration};
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
#[serde(rename_all = "camelCase")]
pub struct HexJsonEntry {
    pub current_day: u64,
    pub tshare_rate_hex: f64,
    pub daily_payout_hex: f64,
    pub payout_per_tshare_hex: f64,
    pub price_pulse_x: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LiveData {
    pub price_pulsechain: f64,
    pub tshare_price_pulsechain: f64,
    pub tshare_rate_hex_pulsechain: f64,
    pub penalties_hex_pulsechain: f64,
    pub payout_per_tshare_pulsechain: f64,
    pub beat: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Miner {
    pub start_date: String,
    pub end_date: String,
    pub t_shares: f64,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub live_data_frequency: u64,
    pub liquid_hex: f64,
    pub historical_start_day: u64,
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
    u.to_string().parse::<f64>().unwrap_or(0.0)
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
    // 1. DexScreener Price
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

    // 2. Gas Price (Beat)
    let gas_price_hex = call_rpc(client, "eth_gasPrice", serde_json::json!([])).await?;
    let gas_price_wei = parse_u256(&gas_price_hex);
    let beat = u256_to_f64(gas_price_wei) / 1e9;

    // 3. Globals (0xc3124525)
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

    // 4. Daily Data Payout
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
                    break; // Break inner loop to recreate the interval with new frequency
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
    // TODO: Persist to disk async
    StatusCode::CREATED
}

// =============================================
// MAIN & ROUTING
// =============================================
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    tokio::fs::create_dir_all(DATA_DIR).await.expect("Failed to create data dir");

    let (config_tx, _) = broadcast::channel(16);

    let state = Arc::new(AppState {
        live_data: RwLock::new(LiveData::default()),
        hex_json: RwLock::new(Vec::new()),
        miners: RwLock::new(Vec::new()),
        config: RwLock::new(Config {
            live_data_frequency: 15,
            liquid_hex: 0.0,
            historical_start_day: 1260,
        }),
        config_tx,
    });

    // Start background tasks
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
        
    tokio::spawn(live_data_updater(state.clone(), client));

    // Build Router
    let app = Router::new()
        .route("/api/live-data", get(handle_live_data))
        .route("/api/add-miner", post(handle_add_miner))
        .fallback(get(|uri: axum::http::Uri| async move {
            // Serve embedded static files
            let path = uri.path().trim_start_matches('/');
            let path = if path.is_empty() { "index.html" } else { path };
            
            match Assets::get(path) {
                Some(content) => {
                    let mime = mime_guess::from_path(path).first_or_octet_stream();
                    Ok::<_, StatusCode>(axum::response::Response::builder()
                        .header("Content-Type", mime.as_ref())
                        .body(content.data.into())
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
