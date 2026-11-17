use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{sse::Event as SseEvent, sse::KeepAlive, IntoResponse, Response, Sse},
    routing::{get, post},
    Json, Router,
};
use chrono::{Datelike, Days, NaiveDate, NaiveTime, Utc};
use futures_util::stream::{self, Stream, StreamExt};
use reqwest::Client;
use rust_embed::RustEmbed;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;
use tower_http::compression::CompressionLayer;
use tracing::{error, info, warn};

// ============================================================
// CONSTANTS
// ============================================================
const RPC_ENDPOINTS: &[&str] = &[
    "https://rpc.pulsechain.com",
    "https://rpc-pulsechain.g4mm4.io",
    "https://rpc.libertyswap.finance",
    "https://pulsechain-rpc.publicnode.com",
    "https://rpc.pulsechainstats.com",
    "https://rpc.hairylabs.io",
    "https://rpc.pulsechainrpc.com",
];
const HEX_CONTRACT: &str = "0x2b591e99afe9f32eaa6214f7b7629768c40eeb39";
const USDC_HEX_PAIR: &str = "0xc475332e92561cd58f278e4e2ed76c17d5b50f05";
const MULTICALL3: &str = "0xca11bde05977b3631167028862be2a173976ca11";
const SEL_GET_RESERVES: &str = "0902f1ac";
const SEL_TOKEN0: &str = "0dfe1681";
const SEL_GLOBALS: &str = "c3124525";
const SEL_DAILY_DATA: &str = "90de6871";
const SEL_STAKE_COUNT: &str = "33060d90";
const SEL_STAKE_LISTS: &str = "2607443b";
const SEL_BALANCE_OF: &str = "70a08231";
const SEL_AGGREGATE3: &str = "82ad56cb";

const DEXSCREENER_URL: &str = "https://api.dexscreener.com/latest/dex/tokens/0x2b591e99afe9f32eaa6214f7b7629768c40eeb39";
const GECKOTERMINAL_URL: &str ="https://api.geckoterminal.com/api/v2/networks/pulsechain/tokens/0x2b591e99afe9f32eaa6214f7b7629768c40eeb39";

const HEARTS_PER_HEX: f64 = 1e8;
const USDC_DECIMALS: u32 = 6;
const HEX_DECIMALS: u32 = 8;
const TSHARE_UNIT: f64 = 10_000.0;
const SHARES_PER_TSHARE: f64 = 1e12;
const SHARE_RATE_DIVISOR: f64 = 10.0;
const HEX_DAY_ZERO_UNIX: i64 = 1_575_331_200;
const SECS_PER_DAY: u64 = 86_400;
const HISTORICAL_QUERY_OFFSET_SECS: u64 = 3_700;
const HISTORICAL_TIMESTAMP_TOLERANCE_SECS: u64 = 30;
const DEFAULT_SECONDS_PER_BLOCK: f64 = 2.0;
const BLOCK_TIME_SAMPLE_BLOCKS: u64 = 100_000;
const MAX_CALLS_PER_REQ: usize = 100;
const BACKFILL_CONCURRENCY: usize = 6;
const BACKFILL_PUBLISH_INTERVAL: usize = 200;
const RPC_MIN_GAP_MS: u64 = 25;
const RPC_TIMEOUT_SECS: u64 = 8;
const RPC_COOLDOWN_MAX_SECS: u64 = 300;
const GLOBALS_TTL_SECS: u64 = 10;
const PRICE_SAMPLE_SECS: u64 = 15;
const RAW_WINDOW_SECS: u64 = 24 * 3600;
const COARSE_SECS: u64 = 60;
const MAX_RETENTION_SECS: u64 = 7 * 24 * 3600;
const MAX_POINTS: usize = 20_000;
const MAX_CANDLES: usize = 5_000;
const LIVE_PERSIST_SECS: u64 = 300;
const WALLET_REFRESH_SECS: u64 = 3_600;
const DAILY_RETRY_SECS: u64 = 900;
const MAX_STAKE_COUNT: u64 = 500;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

// ============================================================
// SMALL HELPERS
// ============================================================
fn unix_now() -> u64 { Utc::now().timestamp().max(0) as u64 }
fn jitter_ms(max: u64) -> u64 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64).unwrap_or(0);
    if max == 0 { 0 } else { n % max }
}
fn hex_today() -> u64 {
    ((unix_now() as i64 - HEX_DAY_ZERO_UNIX) / SECS_PER_DAY as i64).max(0) as u64
}
fn strip0x(s: &str) -> &str {
    let t = s.trim();
    t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t)
}
fn word(s: &str, i: usize) -> Result<&str, String> {
    let s = strip0x(s);
    if !s.is_ascii() { return Err("non-ASCII hex payload".into()); }
    s.get(i * 64..(i + 1) * 64)
        .ok_or_else(|| format!("hex payload too short: want word {i}, have {} chars", s.len()))
}
fn word_u256(s: &str, i: usize) -> Result<U256, String> {
    U256::from_hex(word(s, i)?).map_err(|e| e.to_string())
}
fn word_u64(s: &str, i: usize) -> Result<u64, String> {
    let v = word_u256(s, i)?;
    if v.0[1] != 0 || v.0[2] != 0 || v.0[3] != 0 { return Err("word overflows u64".into()); }
    Ok(v.0[0])
}
fn word_f64(s: &str, i: usize) -> Result<f64, String> { Ok(word_u256(s, i)?.to_f64()) }
fn pad32(a: &str) -> String { format!("{:0>64}", strip0x(a).to_lowercase()) }
fn calldata(sel: &str, args: &[String]) -> String {
    let mut s = format!("0x{sel}");
    for a in args { s.push_str(a); }
    s
}
fn call(to: &str, data: String) -> Call { Call { to: to.to_string(), data } }
fn block_param(b: Option<u64>) -> serde_json::Value {
    match b { Some(n) => serde_json::json!(format!("0x{n:x}")), None => serde_json::json!("latest") }
}
fn normalize_address(s: &str) -> Option<String> {
    let t = s.trim();
    if t.len() != 42 || !t.starts_with("0x") && !t.starts_with("0X") { return None; }
    if !t[2..].chars().all(|c| c.is_ascii_hexdigit()) { return None; }
    Some(t.to_lowercase())
}
fn parse_addresses(s: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    s.split(',').filter_map(normalize_address).filter(|a| seen.insert(a.clone())).collect()
}

// ============================================================
// U256
// ============================================================
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct U256([u64; 4]);
#[derive(Debug, thiserror::Error)]
pub enum HexParseError {
    #[error("empty hex string")] Empty,
    #[error("invalid hex character: {0:?} in {1:?}")] InvalidChar(char, String),
    #[error("hex string too long ({0} chars, max {1})")] TooLong(usize, usize),
}
impl U256 {
    const ZERO: Self = Self([0; 4]);
    fn from_hex(s: &str) -> Result<Self, HexParseError> {
        let s = strip0x(s);
        if s.is_empty() { return Err(HexParseError::Empty); }
        if s.len() > 64 { return Err(HexParseError::TooLong(s.len(), 64)); }
        let mut limbs = [0u64; 4];
        for (i, c) in s.chars().rev().enumerate() {
            let v = match c {
                '0'..='9' => c as u64 - '0' as u64,
                'a'..='f' => c as u64 - 'a' as u64 + 10,
                'A'..='F' => c as u64 - 'A' as u64 + 10,
                _ => return Err(HexParseError::InvalidChar(c, s.to_string())),
            };
            limbs[i / 16] |= v << ((i % 16) * 4);
        }
        Ok(Self(limbs))
    }
    fn to_f64(self) -> f64 {
        self.0.iter().enumerate().map(|(i, l)| (*l as f64) * 2f64.powi(64 * i as i32)).sum()
    }
}
fn parse_hex_u64(s: &str) -> Result<u64, HexParseError> {
    let st = strip0x(s);
    if st.is_empty() { return Err(HexParseError::Empty); }
    if st.len() > 16 { return Err(HexParseError::TooLong(st.len(), 16)); }
    u64::from_str_radix(st, 16)
        .map_err(|_| HexParseError::InvalidChar(st.chars().next().unwrap_or('\0'), s.into()))
}

// ============================================================
// DATA TYPES
// ============================================================
fn f64_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.unwrap_or(0.0))
}
fn u64_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    Ok(Option::<u64>::deserialize(d)?.unwrap_or(0))
}

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
impl HexJsonEntry {
    fn has_payout(&self) -> bool { self.daily_payout_hex > 0.0 && self.payout_per_tshare_hex > 0.0 }
    fn has_market(&self) -> bool { self.price_pulse_x > 0.0 && self.tshare_rate_hex > 0.0 }
    fn complete(&self) -> bool { self.has_payout() && self.has_market() }
}

#[derive(Clone, Copy, Debug)]
struct LivePricePoint { ts: u64, price: f64 }
#[derive(Serialize)]
struct LiveChartCandle { t: u64, o: f64, h: f64, l: f64, c: f64, n: u32 }
#[derive(Serialize)]
struct LiveChartResponse { range_minutes: u64, interval_minutes: u64, now: u64, candles: Vec<LiveChartCandle> }
struct CachedLiveChart { key: (u64, u64, u64, u64), response: Arc<LiveChartResponse> }

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LiveData {
    #[serde(rename = "price_Pulsechain")] pub price_pulsechain: f64,
    #[serde(rename = "tsharePrice_Pulsechain")] pub tshare_price_pulsechain: f64,
    #[serde(rename = "tshareRateHEX_Pulsechain")] pub tshare_rate_hex_pulsechain: f64,
    #[serde(rename = "penaltiesHEX_Pulsechain")] pub penalties_hex_pulsechain: f64,
    #[serde(rename = "payoutPerTshare_Pulsechain")] pub payout_per_tshare_pulsechain: f64,
    #[serde(rename = "beat")] pub beat: f64,
    #[serde(rename = "liquidHEX", default)] pub liquid_hex: f64,
    #[serde(rename = "updatedAt", default)] pub updated_at: u64,   // 5.3
    #[serde(rename = "priceSource", default)] pub price_source: String,
    #[serde(rename = "currentDay", default)] pub current_day: u64,
    #[serde(rename = "stale", default)] pub stale: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Miner {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub id: Option<u64>,
    #[serde(default)] pub address: String,
    #[serde(rename = "stakeId", default, skip_serializing_if = "Option::is_none")] pub stake_id: Option<u64>,
    #[serde(rename = "startDate")] pub start_date: String,
    #[serde(rename = "endDate")] pub end_date: String,
    #[serde(rename = "tShares")] pub t_shares: f64,
    #[serde(rename = "stakedHEX", default)] pub staked_hex: f64,
    #[serde(rename = "autoStake", default)] pub auto_stake: bool,
    #[serde(rename = "endedOnChain", default)] pub ended_on_chain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub status: Option<String>,
}
impl Miner {
    fn is_completed(&self) -> bool { self.status.as_deref() == Some("completed") }
    fn key(&self) -> Option<(String, u64)> {
        self.stake_id.map(|s| (self.address.to_lowercase(), s))
    }
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
#[serde(default)]
pub struct Config {
    #[serde(rename = "liveDataFrequency")] pub live_data_frequency: u64,
    #[serde(rename = "liquidHEX")] pub liquid_hex: f64,
    #[serde(rename = "historicalStartDay")] pub historical_start_day: u64,
    #[serde(rename = "walletAddresses")] pub wallet_addresses: Arc<str>,
    #[serde(rename = "liveChartRangeMinutes", default = "def_range")] pub live_chart_range_minutes: u64,
    #[serde(rename = "liveChartCandleMinutes", default = "def_candle")] pub live_chart_candle_minutes: u64,
}
fn def_range() -> u64 { 10_080 }
fn def_candle() -> u64 { 15 }
impl Default for Config {
    fn default() -> Self {
        Config {
            live_data_frequency: 1,
            liquid_hex: 0.0,
            historical_start_day: 1256,
            wallet_addresses: Arc::from(""),
            live_chart_range_minutes: def_range(),
            live_chart_candle_minutes: def_candle(),
        }
    }
}
fn sanitize_config(mut c: Config) -> Config {
    c.live_data_frequency = c.live_data_frequency.clamp(1, 24 * 60);
    c.live_chart_range_minutes = c.live_chart_range_minutes.clamp(5, 7 * 24 * 60);
    let min_candle = c.live_chart_range_minutes.div_ceil(MAX_CANDLES as u64).max(1);
    c.live_chart_candle_minutes = c.live_chart_candle_minutes.clamp(min_candle, c.live_chart_range_minutes);
    if !c.liquid_hex.is_finite() || c.liquid_hex < 0.0 { c.liquid_hex = 0.0; }
    c.historical_start_day = c.historical_start_day.max(1);
    let norm = parse_addresses(&c.wallet_addresses).join(",");
    c.wallet_addresses = Arc::from(norm.as_str());
    c
}

#[derive(Clone, Default, Serialize)]
struct BackfillStatus { running: bool, phase: String, total: usize, done: usize }
#[derive(Clone, Default, Serialize)]
struct Status {
    version: String, uptime_secs: u64, active_rpc: String, multicall: bool,
    rpc: Vec<RpcStatus>, sample_secs: u64, live_points: usize,
    live_first_ts: u64, live_last_ts: u64, last_live_fetch: u64, last_live_error: String,
    hexjson_days: usize, hexjson_first_day: u64, hexjson_last_day: u64, hex_day_today: u64,
    last_daily_record: u64, backfill: BackfillStatus,
    miners_active: usize, miners_completed: usize, wallet_addresses: usize,
    last_wallet_refresh: u64, last_wallet_error: String,
}
#[derive(Clone, Serialize)]
struct RpcStatus { url: String, failures: u32, cooldown_secs: u64, latency_ms: u64, active: bool }

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum AppEvent {
    Live { data: LiveData },
    Chart { version: u64 },
    Miners { count: usize },
    Config { config: Config },
    HexJson { version: u64, len: usize },
    Status { status: Status },
}

// ============================================================
// PERSISTENCE
// ============================================================
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind { Config, Miners, HexJson, LiveChart }
struct WriteJob { kind: Kind, version: u64, path: PathBuf, bytes: Vec<u8> }
#[derive(Clone)]
struct Persister { tx: mpsc::UnboundedSender<WriteJob> }
impl Persister {
    fn submit(&self, kind: Kind, version: u64, path: PathBuf, bytes: Vec<u8>) {
        let _ = self.tx.send(WriteJob { kind, version, path, bytes });
    }
}
fn data_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Ok(p) = std::env::var("HEXTRACK_DATA_DIR") { if !p.trim().is_empty() { return PathBuf::from(p); } }
        std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    })
}
fn hexjson_path() -> PathBuf { data_dir().join("hexjson.json") }
fn config_path() -> PathBuf { data_dir().join("config.json") }
fn miners_path() -> PathBuf { data_dir().join("miners.json") }
fn livechart_path() -> PathBuf { data_dir().join("livechart.json") }

async fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = tokio::fs::File::create(&tmp).await?;
    tokio::io::AsyncWriteExt::write_all(&mut f, bytes).await?;
    f.sync_all().await?;
    tokio::fs::rename(&tmp, path).await
}
async fn persistence_worker(mut rx: mpsc::UnboundedReceiver<WriteJob>) {
    let mut written: HashMap<Kind, u64> = HashMap::new();
    while let Some(first) = rx.recv().await {
        let mut latest: HashMap<Kind, WriteJob> = HashMap::new();
        latest.insert(first.kind, first);
        while let Ok(j) = rx.try_recv() { latest.insert(j.kind, j); } // coalesce
        for (kind, job) in latest {
            if written.get(&kind).copied().unwrap_or(0) >= job.version { continue; }
            match atomic_write(&job.path, &job.bytes).await {
                Ok(_) => { written.insert(kind, job.version); }
                Err(e) => error!("Failed to persist {}: {e}", job.path.display()),
            }
        }
    }
    info!("Persistence worker flushed and stopped");
}
fn persist_config(p: &Persister, v: u64, c: &Config) {
    if let Ok(b) = serde_json::to_vec_pretty(c) { p.submit(Kind::Config, v, config_path(), b); }
}
fn persist_miners(p: &Persister, v: u64, m: &[Miner]) {
    if let Ok(b) = serde_json::to_vec_pretty(m) { p.submit(Kind::Miners, v, miners_path(), b); }
}
fn persist_hexjson(p: &Persister, v: u64, d: &[HexJsonEntry]) {
    if let Ok(b) = serde_json::to_vec(d) { p.submit(Kind::HexJson, v, hexjson_path(), b); }
}

// ============================================================
// RPC POOL
// ============================================================
struct RpcEndpoint { url: &'static str, failures: u32, cooldown_until: u64, latency_ms: u64 }
struct RpcPool { endpoints: RwLock<Vec<RpcEndpoint>>, active: AtomicUsize, gate: Mutex<Instant> }
impl RpcPool {
    fn new() -> Self {
        Self {
            endpoints: RwLock::new(RPC_ENDPOINTS.iter().map(|u| RpcEndpoint {
                url: u, failures: 0, cooldown_until: 0, latency_ms: 0 }).collect()),
            active: AtomicUsize::new(0),
            gate: Mutex::new(Instant::now()),
        }
    }
    async fn order(&self) -> Vec<usize> {
        let now = unix_now();
        let eps = self.endpoints.read().await;
        let act = self.active.load(Ordering::Relaxed);
        let mut idx: Vec<usize> = (0..eps.len()).collect();
        idx.sort_by_key(|i| {
            let e = &eps[*i];
            (if e.cooldown_until > now { 1 } else { 0 }, e.failures, if *i == act { 0 } else { 1 }, *i)
        });
        idx
    }
    async fn throttle(&self) {
        let mut next = self.gate.lock().await;
        let now = Instant::now();
        let slot = (*next).max(now);
        *next = slot + Duration::from_millis(RPC_MIN_GAP_MS);
        drop(next);
        if slot > now { tokio::time::sleep(slot - now).await; }
    }
    async fn ok(&self, idx: usize, ms: u64) {
        let mut eps = self.endpoints.write().await;
        eps[idx].failures = 0; eps[idx].cooldown_until = 0; eps[idx].latency_ms = ms;
        self.active.store(idx, Ordering::Relaxed);
    }
    async fn bad(&self, idx: usize) {
        let mut eps = self.endpoints.write().await;
        let e = &mut eps[idx];
        e.failures = e.failures.saturating_add(1);
        let back = (5u64 << e.failures.min(6)).min(RPC_COOLDOWN_MAX_SECS);
        e.cooldown_until = unix_now() + back;
    }
    async fn snapshot(&self) -> Vec<RpcStatus> {
        let now = unix_now();
        let act = self.active.load(Ordering::Relaxed);
        self.endpoints.read().await.iter().enumerate().map(|(i, e)| RpcStatus {
            url: e.url.to_string(), failures: e.failures,
            cooldown_secs: e.cooldown_until.saturating_sub(now),
            latency_ms: e.latency_ms, active: i == act,
        }).collect()
    }
}

// ============================================================
// APP STATE
// ============================================================
struct AppState {
    client: Client,
    rpc: RpcPool,
    config: RwLock<Config>,
    live_data: RwLock<LiveData>,
    hex_json: RwLock<Arc<[HexJsonEntry]>>,
    miners: RwLock<Vec<Miner>>,
    status: RwLock<Status>,
    events: broadcast::Sender<AppEvent>,
    config_tx: broadcast::Sender<()>,
    backfill: Arc<Notify>,
    hex_json_version: AtomicU64,
    config_version: AtomicU64,
    miners_version: AtomicU64,
    next_miner_id: AtomicU64,
    live_chart: RwLock<VecDeque<LivePricePoint>>,
    live_chart_cache: RwLock<Option<CachedLiveChart>>,
    live_chart_version: AtomicU64,
    globals_cache: Mutex<Option<(Instant, Globals)>>,
    multicall: AtomicBool,
    hex_is_token0: AtomicBool,
    persister: Persister,
    shutdown: CancellationToken,
    started: Instant,
}
impl AppState {
    fn emit(&self, e: AppEvent) { let _ = self.events.send(e); }
}

// ============================================================
// JSON-RPC TRANSPORT
// ============================================================
async fn rpc_send(state: &AppState, body: &serde_json::Value, batch: bool) -> Result<serde_json::Value, String> {
    let order = state.rpc.order().await;
    let mut last = String::from("no endpoints");
    for idx in order {
        let url = state.rpc.endpoints.read().await[idx].url;
        state.rpc.throttle().await;
        let t0 = Instant::now();
        let res = state.client.post(url).timeout(Duration::from_secs(RPC_TIMEOUT_SECS)).json(body).send().await;
        match res {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(v) => {
                        let bad = if batch { !v.is_array() } else { v.get("error").is_some() };
                        if bad {
                            last = format!("{url}: {}", v.get("error").map(|e| e.to_string())
                                .unwrap_or_else(|| "batch not supported / malformed".into()));
                            state.rpc.bad(idx).await;
                            continue;
                        }
                        state.rpc.ok(idx, t0.elapsed().as_millis() as u64).await;
                        return Ok(v);
                    }
                    Err(e) => { last = format!("{url}: decode: {e}"); state.rpc.bad(idx).await; }
                }
            }
            Ok(resp) => { last = format!("{url}: HTTP {}", resp.status()); state.rpc.bad(idx).await; }
            Err(e) => { last = format!("{url}: {e}"); state.rpc.bad(idx).await; }
        }
    }
    Err(format!("all RPC endpoints failed: {last}"))
}
async fn rpc_call(state: &AppState, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    let v = rpc_send(state, &body, false).await?;
    Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
}
async fn rpc_batch(state: &AppState, reqs: Vec<serde_json::Value>) -> Result<Vec<serde_json::Value>, String> {
    let v = rpc_send(state, &serde_json::Value::Array(reqs), true).await?;
    Ok(v.as_array().cloned().unwrap_or_default())
}
async fn rpc_block_number(state: &AppState) -> Result<u64, String> {
    let v = rpc_call(state, "eth_blockNumber", serde_json::json!([])).await?;
    parse_hex_u64(v.as_str().ok_or("blockNumber not a string")?).map_err(|e| e.to_string())
}

// ============================================================
// MULTICALL3
// ============================================================
#[derive(Clone)]
struct Call { to: String, data: String }

fn encode_aggregate3(calls: &[Call]) -> String {
    let w = |x: u64| format!("{x:064x}");
    let mut elems = Vec::with_capacity(calls.len());
    for c in calls {
        let bytes = strip0x(&c.data);
        let padded = bytes.len().div_ceil(64) * 64;
        let mut e = String::new();
        e.push_str(&pad32(&c.to));
        e.push_str(&w(1));
        e.push_str(&w(0x60));
        e.push_str(&w((bytes.len() / 2) as u64));
        e.push_str(bytes);
        for _ in bytes.len()..padded { e.push('0'); }
        elems.push(e);
    }
    let mut body = w(calls.len() as u64);
    let mut off = (calls.len() * 32) as u64;
    let mut tails = String::new();
    for e in &elems { body.push_str(&w(off)); off += (e.len() / 2) as u64; tails.push_str(e); }
    body.push_str(&tails);
    format!("0x{SEL_AGGREGATE3}{}{body}", w(0x20))
}
fn decode_aggregate3(s: &str, expect: usize) -> Result<Vec<Option<String>>, String> {
    let raw = strip0x(s);
    if !raw.is_ascii() { return Err("non-ASCII multicall payload".into()); }
    let outer = word_u64(raw, 0)? as usize;
    if outer % 32 != 0 { return Err("bad multicall outer offset".into()); }
    let base = outer / 32;
    let n = word_u64(raw, base)? as usize;
    if n != expect { return Err(format!("multicall length {n} != {expect}")); }
    let offs = base + 1;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = word_u64(raw, offs + i)? as usize;
        if o % 32 != 0 { return Err("bad multicall element offset".into()); }
        let e = offs + o / 32;
        let success = word_u64(raw, e)? != 0;
        let d = word_u64(raw, e + 1)? as usize;
        if d % 32 != 0 { return Err("bad multicall data offset".into()); }
        let lw = e + d / 32;
        let len = word_u64(raw, lw)? as usize;
        let (a, b) = ((lw + 1) * 64, (lw + 1) * 64 + len * 2);
        let data = raw.get(a..b).ok_or("multicall payload truncated")?;
        out.push(if success && len > 0 { Some(format!("0x{data}")) } else { None });
    }
    Ok(out)
}
async fn multicall3(state: &AppState, block: Option<u64>, calls: &[Call]) -> Result<Vec<Option<String>>, String> {
    let params = serde_json::json!([{"to": MULTICALL3, "data": encode_aggregate3(calls)}, block_param(block)]);
    let v = rpc_call(state, "eth_call", params).await?;
    decode_aggregate3(v.as_str().ok_or("multicall result not a string")?, calls.len())
}
async fn batch_calls(state: &AppState, block: u64, calls: &[Call]) -> Result<Vec<Option<String>>, String> {
    let blk = block_param(Some(block));
    let reqs: Vec<_> = calls.iter().enumerate().map(|(i, c)| serde_json::json!({
        "jsonrpc":"2.0","id":i,"method":"eth_call",
        "params":[{"to":c.to,"data":c.data}, blk]
    })).collect();
    let resp = rpc_batch(state, reqs).await?;
    let mut out = vec![None; calls.len()];
    for item in resp {
        let id = item.get("id").and_then(|v| v.as_u64()).unwrap_or(u64::MAX) as usize;
        if id >= out.len() || item.get("error").is_some() { continue; }
        if let Some(s) = item.get("result").and_then(|v| v.as_str()) {
            if strip0x(s).len() >= 64 { out[id] = Some(s.to_string()); }
        }
    }
    Ok(out)
}
async fn eth_calls(state: &AppState, block: Option<u64>, calls: &[Call]) -> Result<Vec<Option<String>>, String> {
    if calls.is_empty() { return Ok(vec![]); }
    let pinned = match block {
        Some(b) => Some(b),
        None => if state.multicall.load(Ordering::Relaxed) { None } else { Some(rpc_block_number(state).await?) },
    };
    let mut out = Vec::with_capacity(calls.len());
    for chunk in calls.chunks(MAX_CALLS_PER_REQ) {
        let r = if state.multicall.load(Ordering::Relaxed) {
            match multicall3(state, pinned, chunk).await {
                Ok(v) => v,
                Err(e) => {
                    warn!("Multicall3 failed ({e}); disabling and using pinned-block batches");
                    state.multicall.store(false, Ordering::Relaxed);
                    let b = match pinned { Some(b) => b, None => rpc_block_number(state).await? };
                    batch_calls(state, b, chunk).await?
                }
            }
        } else {
            batch_calls(state, pinned.unwrap_or(0), chunk).await?
        };
        out.extend(r);
    }
    Ok(out)
}

// ============================================================
// CHAIN DECODERS
// ============================================================
#[derive(Clone, Copy, Debug, Default)]
struct Globals { tshare_rate: f64, day_count: u64, penalties: f64 }
fn decode_globals(s: &str) -> Result<Globals, String> {
    Ok(Globals {
        tshare_rate: word_f64(s, 2)? / SHARE_RATE_DIVISOR,
        penalties: word_f64(s, 3)? / HEARTS_PER_HEX,
        day_count: word_u64(s, 4)?,
    })
}
fn decode_daily(s: &str) -> Result<(f64, f64), String> { Ok((word_f64(s, 0)?, word_f64(s, 1)?)) }
fn decode_price(s: &str, hex_is_token0: bool) -> Result<f64, String> {
    let (r0, r1) = (word_f64(s, 0)?, word_f64(s, 1)?);
    let (usdc, hex) = if hex_is_token0 { (r1, r0) } else { (r0, r1) };
    if usdc <= 0.0 || hex <= 0.0 { return Err("zero reserves".into()); }
    let scale = 10f64.powi(HEX_DECIMALS as i32 - USDC_DECIMALS as i32);
    let p = (usdc / hex) * scale;
    if !p.is_finite() || p <= 0.0 { return Err("invalid price".into()); }
    Ok(p)
}
fn calc_payout_per_tshare(payout_hearts: f64, shares: f64) -> f64 {
    if shares <= 0.0 || !shares.is_finite() || !payout_hearts.is_finite() { return 0.0; }
    (payout_hearts / shares) * TSHARE_UNIT
}
fn last_finalized_day(g: &Globals) -> u64 {
    hex_today().saturating_sub(1).min(g.day_count.saturating_sub(1))
}
fn globals_call() -> Call { call(HEX_CONTRACT, calldata(SEL_GLOBALS, &[])) }
fn daily_call(day: u64) -> Call { call(HEX_CONTRACT, calldata(SEL_DAILY_DATA, &[format!("{day:064x}")])) }
fn reserves_call() -> Call { call(USDC_HEX_PAIR, calldata(SEL_GET_RESERVES, &[])) }
fn balance_call(a: &str) -> Call { call(HEX_CONTRACT, calldata(SEL_BALANCE_OF, &[pad32(a)])) }

async fn read_globals(state: &AppState, block: Option<u64>) -> Result<Globals, String> {
    let r = eth_calls(state, block, &[globals_call()]).await?;
    decode_globals(r[0].as_deref().ok_or("globals() empty result")?)
}
async fn globals_cached(state: &AppState) -> Result<Globals, String> {
    let mut guard = state.globals_cache.lock().await;
    if let Some((at, g)) = *guard {
        if at.elapsed() < Duration::from_secs(GLOBALS_TTL_SECS) { return Ok(g); }
    }
    let g = read_globals(state, None).await?;
    *guard = Some((Instant::now(), g));
    Ok(g)
}
async fn read_daily(state: &AppState, day: u64, block: Option<u64>) -> Result<(f64, f64), String> {
    let r = eth_calls(state, block, &[daily_call(day)]).await?;
    decode_daily(r[0].as_deref().ok_or("dailyData() empty result")?)
}

// ============================================================
// PRICE
// ============================================================
async fn fetch_price_rpc(state: &AppState) -> Result<f64, String> {
    let r = eth_calls(state, None, &[reserves_call()]).await?;
    decode_price(r[0].as_deref().ok_or("getReserves() empty")?, state.hex_is_token0.load(Ordering::Relaxed))
}
async fn fetch_price_dexscreener(state: &AppState) -> Result<f64, String> {
    let v: serde_json::Value = state.client.get(DEXSCREENER_URL).send().await
        .map_err(|e| format!("dexscreener request: {e}"))?.json().await
        .map_err(|e| format!("dexscreener decode: {e}"))?;
    let mut pairs: Vec<&serde_json::Value> = v.get("pairs").and_then(|p| p.as_array())
        .map(|a| a.iter().filter(|p| p.get("chainId").and_then(|c| c.as_str()) == Some("pulsechain")).collect())
        .unwrap_or_default();
    pairs.sort_by(|a, b| {
        let l = |p: &serde_json::Value| p.pointer("/liquidity/usd").and_then(|x| x.as_f64()).unwrap_or(0.0);
        l(b).partial_cmp(&l(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let p = pairs.first().and_then(|p| p.get("priceUsd"))
        .and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0.0);
    if p > 0.0 && p.is_finite() { Ok(p) } else { Err("dexscreener: no valid price".into()) }
}
async fn fetch_price_gecko(state: &AppState) -> Result<f64, String> {
    let v: serde_json::Value = state.client.get(GECKOTERMINAL_URL).header("Accept", "application/json")
        .send().await.map_err(|e| format!("gecko request: {e}"))?.json().await
        .map_err(|e| format!("gecko decode: {e}"))?;
    let p = v.pointer("/data/attributes/price_usd").and_then(|x| x.as_str())
        .and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    if p > 0.0 && p.is_finite() { Ok(p) } else { Err("gecko: no valid price".into()) }
}
async fn fetch_price(state: &AppState) -> Result<(f64, &'static str), String> {
    if let Ok(p) = fetch_price_rpc(state).await { return Ok((p, "rpc")); }
    if let Ok(p) = fetch_price_dexscreener(state).await { return Ok((p, "dexscreener")); }
    fetch_price_gecko(state).await.map(|p| (p, "geckoterminal"))
}

// ============================================================
// LIVE DATA
// ============================================================
async fn fetch_live_data(state: &AppState) -> Result<LiveData, String> {
    let expected_day = hex_today().saturating_sub(1);
    let calls = [reserves_call(), globals_call(), daily_call(expected_day)];
    let (res, gas) = tokio::join!(
        eth_calls(state, None, &calls),
        rpc_call(state, "eth_gasPrice", serde_json::json!([]))
    );
    let res = res?;
    let g = decode_globals(res[1].as_deref().ok_or("globals() missing from live batch")?)?;
    let day = last_finalized_day(&g);

    let (price, source) = match res[0].as_deref().and_then(|r| decode_price(r, state.hex_is_token0.load(Ordering::Relaxed)).ok()) {
        Some(p) => (p, "rpc"),
        None => fetch_price(state).await?,
    };
    let daily = if day == expected_day {
        res[2].clone()
    } else {
        eth_calls(state, None, &[daily_call(day)]).await?.into_iter().next().flatten()
    };
    let payout = daily.as_deref().and_then(|d| decode_daily(d).ok())
        .map(|(p, s)| calc_payout_per_tshare(p, s)).unwrap_or(0.0);
    let beat = gas.ok().and_then(|v| v.as_str().and_then(|s| U256::from_hex(s).ok()))
        .map(|u| u.to_f64() / 1e9).unwrap_or(0.0);

    Ok(LiveData {
        price_pulsechain: price,
        tshare_price_pulsechain: g.tshare_rate * price,
        tshare_rate_hex_pulsechain: g.tshare_rate,
        penalties_hex_pulsechain: g.penalties,
        payout_per_tshare_pulsechain: payout,
        beat,
        liquid_hex: state.config.read().await.liquid_hex,
        updated_at: unix_now(),
        price_source: source.to_string(),
        current_day: day,
        stale: false,
    })
}
async fn fetch_live_data_retry(state: &AppState) -> Result<LiveData, String> {
    let start = Instant::now();
    let mut delay = Duration::from_secs(1);
    let mut last = String::new();
    for attempt in 0..5 {
        match fetch_live_data(state).await {
            Ok(d) => return Ok(d),
            Err(e) => {
                last = e;
                if attempt == 4 || start.elapsed() > Duration::from_secs(60) { break; }
                tokio::time::sleep(delay + Duration::from_millis(jitter_ms(400))).await;
                delay = (delay * 2).min(Duration::from_secs(15));
            }
        }
    }
    Err(last)
}
async fn publish_live(state: &Arc<AppState>, data: LiveData) {
    record_live_price_point(state, data.price_pulsechain).await;
    *state.live_data.write().await = data.clone();
    {
        let mut s = state.status.write().await;
        s.last_live_fetch = data.updated_at;
        s.last_live_error.clear();
    }
    state.emit(AppEvent::Live { data });
}

// ============================================================
// LIVE CHART SERIES
// ============================================================
async fn record_live_price_point(state: &Arc<AppState>, price: f64) {
    if !price.is_finite() || price <= 0.0 { return; }
    let now = unix_now();
    {
        let mut pts = state.live_chart.write().await;
        if pts.back().is_some_and(|p| now <= p.ts) { return; }
        pts.push_back(LivePricePoint { ts: now, price });
        while pts.front().is_some_and(|p| p.ts < now.saturating_sub(MAX_RETENTION_SECS)) { pts.pop_front(); }
        while pts.len() > MAX_POINTS { pts.pop_front(); }
    }
    state.live_chart_version.fetch_add(1, Ordering::Relaxed);
    state.emit(AppEvent::Chart { version: state.live_chart_version.load(Ordering::Relaxed) });
}
fn compact_series(pts: &mut VecDeque<LivePricePoint>, now: u64) {
    while pts.front().is_some_and(|p| p.ts < now.saturating_sub(MAX_RETENTION_SECS)) { pts.pop_front(); }
    let cutoff = now.saturating_sub(RAW_WINDOW_SECS);
    let idx = pts.binary_search_by(|p| p.ts.cmp(&cutoff)).unwrap_or_else(|i| i);
    if idx > 1 {
        let mut kept = Vec::new();
        let mut last_bucket = u64::MAX;
        for i in (0..idx).rev() {
            let p = pts[i];
            let b = p.ts / COARSE_SECS;
            if b != last_bucket { kept.push(p); last_bucket = b; }
        }
        pts.drain(..idx);
        for p in kept.into_iter().rev() { pts.push_front(p); }
    }
    while pts.len() > MAX_POINTS { pts.pop_front(); }
}
async fn persist_live_chart(state: &Arc<AppState>) {
    let pts = state.live_chart.read().await;
    let v: Vec<(u64, f64)> = pts.iter().map(|p| (p.ts, p.price)).collect();
    drop(pts);
    if let Ok(bytes) = serde_json::to_vec(&v) {
        state.persister.submit(Kind::LiveChart, unix_now(), livechart_path(), bytes);
    }
}
async fn load_live_chart(state: &Arc<AppState>) {
    let Ok(txt) = tokio::fs::read_to_string(livechart_path()).await else { return };
    let Ok(v) = serde_json::from_str::<Vec<(u64, f64)>>(&txt) else {
        warn!("livechart.json unreadable, starting empty"); return;
    };
    let mut pts = state.live_chart.write().await;
    let mut last = 0u64;
    for (ts, price) in v {
        if ts > last && price > 0.0 && price.is_finite() { pts.push_back(LivePricePoint { ts, price }); last = ts; }
    }
    compact_series(&mut pts, unix_now());
    info!("Restored {} live price points", pts.len());
    state.live_chart_version.fetch_add(1, Ordering::Relaxed);
}
async fn build_live_chart(state: &AppState, range_min: u64, interval_min: u64) -> LiveChartResponse {
    let now = unix_now();
    let interval = interval_min.max(1) * 60;
    let cutoff = now.saturating_sub(range_min * 60);
    let pts = state.live_chart.read().await;
    let start = pts.binary_search_by(|p| p.ts.cmp(&cutoff)).unwrap_or_else(|i| i);
    let mut candles: Vec<LiveChartCandle> = Vec::new();
    if start < pts.len() {
        let last_bucket = now - now % interval;
        let first_bucket = cutoff - cutoff % interval;
        let span = ((last_bucket - first_bucket) / interval + 1) as usize;
        let mut bucket = if span > MAX_CANDLES { last_bucket - ((MAX_CANDLES - 1) as u64 * interval) } else { first_bucket };
        let mut cursor = start;
        let mut last_close: Option<f64> = None;
        while bucket <= last_bucket && candles.len() < MAX_CANDLES {
            let end = bucket + interval;
            while cursor < pts.len() && pts[cursor].ts < bucket { cursor += 1; }
            let mut j = cursor;
            let (mut o, mut h, mut l, mut c, mut n) = (None, f64::MIN, f64::MAX, None, 0u32);
            while j < pts.len() && pts[j].ts < end {
                let p = pts[j].price;
                if o.is_none() { o = Some(p); }
                h = h.max(p); l = l.min(p); c = Some(p); n += 1; j += 1;
            }
            if n > 0 {
                last_close = c;
                candles.push(LiveChartCandle { t: bucket, o: o.unwrap(), h, l, c: c.unwrap(), n });
                cursor = j;
            } else if let Some(pc) = last_close {
                candles.push(LiveChartCandle { t: bucket, o: pc, h: pc, l: pc, c: pc, n: 0 });
            }
            bucket = end;
        }
    }
    LiveChartResponse { range_minutes: range_min, interval_minutes: interval_min.max(1), now, candles }
}

// ============================================================
// BLOCK FINDER
// ============================================================
#[derive(Clone, Copy)]
struct BlockFinder { latest_block: u64, latest_ts: u64, genesis_ts: u64, spb: f64 }
impl BlockFinder {
    async fn new(state: &AppState) -> Result<Self, String> {
        let latest_block = rpc_block_number(state).await?;
        let latest_ts = block_ts(state, latest_block).await?;
        let sample = latest_block.saturating_sub(BLOCK_TIME_SAMPLE_BLOCKS);
        let sample_ts = if sample > 0 { block_ts(state, sample).await.unwrap_or(0) } else { 0 };
        let mut spb = DEFAULT_SECONDS_PER_BLOCK;
        if sample_ts > 0 && latest_ts > sample_ts && latest_block > sample {
            spb = (latest_ts - sample_ts) as f64 / (latest_block - sample) as f64;
        }
        if !spb.is_finite() || spb <= 0.0 { spb = DEFAULT_SECONDS_PER_BLOCK; }
        let genesis_ts = block_ts(state, 0).await.unwrap_or(0);
        Ok(Self { latest_block, latest_ts, genesis_ts, spb })
    }
    fn target_ts(day: u64) -> u64 {
        (HEX_DAY_ZERO_UNIX as u64) + (day + 1) * SECS_PER_DAY + HISTORICAL_QUERY_OFFSET_SECS
    }
    async fn find(&self, state: &AppState, target: u64, hint: (u64, u64)) -> Result<u64, String> {
        if self.genesis_ts > 0 && target <= self.genesis_ts { return Ok(0); }
        if target >= self.latest_ts { return Ok(self.latest_block); }
        let (hb, ht) = if hint.0 > 0 && hint.1 > 0 { hint } else { (self.latest_block, self.latest_ts) };
        let est = if target >= ht {
            hb.saturating_add(((target - ht) as f64 / self.spb) as u64)
        } else {
            hb.saturating_sub(((ht - target) as f64 / self.spb) as u64)
        }.clamp(1, self.latest_block);
        let mut span = ((3600.0 / self.spb) as u64).max(16);
        let mut lo = est.saturating_sub(span).max(1);
        let mut hi = (est + span).min(self.latest_block);
        for _ in 0..4 {
            if hi <= lo { break; }
            let probes: Vec<u64> = (0..9).map(|i| lo + (hi - lo) * i / 8).collect();
            let ts = block_ts_batch(state, &probes).await?;
            let hit = probes.iter().zip(&ts).position(|(_, t)| t.map(|t| t >= target).unwrap_or(false));
            match hit {
                Some(i) => {
                    let t = ts[i].unwrap_or(0);
                    if t <= target + HISTORICAL_TIMESTAMP_TOLERANCE_SECS { return Ok(probes[i]); }
                    hi = probes[i];
                    lo = if i > 0 { probes[i - 1] } else { lo };
                    if hi.saturating_sub(lo) <= 2 { break; }
                }
                None => { lo = hi; span *= 2; hi = (hi + span).min(self.latest_block); if hi == lo { break; } }
            }
        }
        let probes: Vec<u64> = (lo..=hi.min(lo + 8)).collect();
        let ts = block_ts_batch(state, &probes).await?;
        for (b, t) in probes.iter().zip(&ts) { if t.map(|t| t >= target).unwrap_or(false) { return Ok(*b); } }
        Ok(hi.min(self.latest_block))
    }
}
async fn block_ts(state: &AppState, n: u64) -> Result<u64, String> {
    let v = rpc_call(state, "eth_getBlockByNumber", serde_json::json!([format!("0x{n:x}"), false])).await?;
    if v.is_null() { return Err(format!("block {n} not found")); }
    let s = v.get("timestamp").and_then(|x| x.as_str()).ok_or("block has no timestamp")?;
    parse_hex_u64(s).map_err(|e| e.to_string())
}
async fn block_ts_batch(state: &AppState, blocks: &[u64]) -> Result<Vec<Option<u64>>, String> {
    let reqs: Vec<_> = blocks.iter().enumerate().map(|(i, b)| serde_json::json!({
        "jsonrpc":"2.0","id":i,"method":"eth_getBlockByNumber","params":[format!("0x{b:x}"), false]
    })).collect();
    let resp = rpc_batch(state, reqs).await?;
    let mut out = vec![None; blocks.len()];
    for item in resp {
        let id = item.get("id").and_then(|v| v.as_u64()).unwrap_or(u64::MAX) as usize;
        if id >= out.len() { continue; }
        if let Some(s) = item.pointer("/result/timestamp").and_then(|v| v.as_str()) {
            out[id] = parse_hex_u64(s).ok();
        }
    }
    Ok(out)
}

// ============================================================
// HEXJSON
// ============================================================
fn repair_historical_values(entries: &mut [HexJsonEntry]) {
    entries.sort_by_key(|e| e.current_day);
    let (mut lt, mut lp) = (0.0, 0.0);
    for e in entries.iter_mut() {
        if e.tshare_rate_hex > 0.0 && e.tshare_rate_hex.is_finite() { lt = e.tshare_rate_hex; } else { e.tshare_rate_hex = lt; }
        if e.price_pulse_x > 0.0 && e.price_pulse_x.is_finite() { lp = e.price_pulse_x; } else { e.price_pulse_x = lp; }
    }
    let (mut nt, mut np) = (0.0, 0.0);
    for e in entries.iter_mut().rev() {
        if e.tshare_rate_hex > 0.0 { nt = e.tshare_rate_hex; } else if nt > 0.0 { e.tshare_rate_hex = nt; }
        if e.price_pulse_x > 0.0 { np = e.price_pulse_x; } else if np > 0.0 { e.price_pulse_x = np; }
    }
}
async fn publish_hexjson(state: &Arc<AppState>, mut data: Vec<HexJsonEntry>) {
    repair_historical_values(&mut data);
    let arc: Arc<[HexJsonEntry]> = Arc::from(data);
    let v = state.hex_json_version.fetch_add(1, Ordering::Relaxed) + 1;
    let len = arc.len();
    persist_hexjson(&state.persister, v, &arc);
    *state.hex_json.write().await = arc;
    state.emit(AppEvent::HexJson { version: v, len });
}
async fn store_daily_entries(state: &Arc<AppState>, new_entries: Vec<HexJsonEntry>) {
    if new_entries.is_empty() { return; }
    let snapshot = {
        let mut guard = state.hex_json.write().await;
        let mut v: Vec<HexJsonEntry> = guard.to_vec();
        let mut idx: HashMap<u64, usize> = v.iter().enumerate().map(|(i, e)| (e.current_day, i)).collect();
        for e in new_entries {
            match idx.get(&e.current_day) {
                Some(i) => {
                    let cur = &mut v[*i];
                    if e.has_payout() { cur.daily_payout_hex = e.daily_payout_hex; cur.payout_per_tshare_hex = e.payout_per_tshare_hex; }
                    if e.tshare_rate_hex > 0.0 { cur.tshare_rate_hex = e.tshare_rate_hex; }
                    if e.price_pulse_x > 0.0 { cur.price_pulse_x = e.price_pulse_x; }
                }
                None => { idx.insert(e.current_day, v.len()); v.push(e); }
            }
        }
        v.sort_by_key(|e| e.current_day);
        let arc: Arc<[HexJsonEntry]> = Arc::from(v);
        *guard = Arc::clone(&arc);
        arc
    };
    let ver = state.hex_json_version.fetch_add(1, Ordering::Relaxed) + 1;
    persist_hexjson(&state.persister, ver, &snapshot);
    state.emit(AppEvent::HexJson { version: ver, len: snapshot.len() });
}

async fn catch_up_hexjson(state: &Arc<AppState>) -> bool {
    let g = match globals_cached(state).await {
        Ok(g) => g,
        Err(e) => { warn!("catch-up: globals() failed: {e}"); return false; }
    };
    let last_final = last_finalized_day(&g);
    let start = state.config.read().await.historical_start_day;
    if last_final < start { return true; }

    let existing: Vec<HexJsonEntry> = state.hex_json.read().await.to_vec();
    let mut by_day: HashMap<u64, HexJsonEntry> = existing.into_iter().map(|e| (e.current_day, e)).collect();
    let all: Vec<u64> = (start..=last_final).collect();
    let need_payout: Vec<u64> = all.iter().copied()
        .filter(|d| by_day.get(d).map(|e| !e.has_payout()).unwrap_or(true)).collect();
    let need_market: Vec<u64> = all.iter().copied()
        .filter(|d| by_day.get(d).map(|e| !e.has_market()).unwrap_or(true)).collect();
    if need_payout.is_empty() && need_market.is_empty() { return true; }

    let total = need_payout.len() + need_market.len();
    {
        let mut s = state.status.write().await;
        s.backfill = BackfillStatus { running: true, phase: "payouts".into(), total, done: 0 };
    }
    state.emit(AppEvent::Status { status: status_snapshot(state).await });
    info!("Catch-up: {} payout days, {} market days (start {start}, last finalized {last_final})",
        need_payout.len(), need_market.len());

    let mut done = 0usize;
    let mut errors = 0u32;
    for chunk in need_payout.chunks(MAX_CALLS_PER_REQ) {
        if state.shutdown.is_cancelled() { break; }
        let calls: Vec<Call> = chunk.iter().map(|d| daily_call(*d)).collect();
        match eth_calls(state, None, &calls).await {
            Ok(res) => {
                for (day, r) in chunk.iter().zip(res) {
                    let Some(hexs) = r else { continue };
                    let Ok((payout, shares)) = decode_daily(&hexs) else { continue };
                    if payout <= 0.0 || shares <= 0.0 { continue; }
                    let e = by_day.entry(*day).or_insert(HexJsonEntry { current_day: *day, ..Default::default() });
                    e.daily_payout_hex = payout / HEARTS_PER_HEX;
                    e.payout_per_tshare_hex = calc_payout_per_tshare(payout, shares);
                }
                errors = 0;
            }
            Err(e) => { errors += 1; warn!("payout chunk failed: {e}"); if errors >= 5 { break; } }
        }
        done += chunk.len();
        update_backfill(state, "payouts", total, done).await;
        if done % BACKFILL_PUBLISH_INTERVAL < MAX_CALLS_PER_REQ {
            publish_hexjson(state, by_day.values().cloned().collect()).await;
        }
    }
    publish_hexjson(state, by_day.values().cloned().collect()).await;

    if !need_market.is_empty() && !state.shutdown.is_cancelled() {
        match BlockFinder::new(state).await {
            Err(e) => warn!("catch-up: BlockFinder init failed: {e}"),
            Ok(finder) => {
                let hint_b = Arc::new(AtomicU64::new(finder.latest_block));
                let hint_t = Arc::new(AtomicU64::new(finder.latest_ts));
                let live_price = state.live_data.read().await.price_pulsechain;
                let mut results = stream::iter(need_market.clone())
                    .map(|day| {
                        let (hb, ht) = (hint_b.clone(), hint_t.clone());
                        async move {
                            let mut target = BlockFinder::target_ts(day);
                            for attempt in 0..2 {
                                let hint = (hb.load(Ordering::Relaxed), ht.load(Ordering::Relaxed));
                                let block = match finder.find(state, target, hint).await {
                                    Ok(b) => b, Err(e) => { if attempt == 1 { return (day, Err(e)); } target += 3600; continue; }
                                };
                                let r = match eth_calls(state, Some(block), &[globals_call(), reserves_call()]).await {
                                    Ok(r) => r, Err(e) => { if attempt == 1 { return (day, Err(e)); } target += 3600; continue; }
                                };
                                let gg = r[0].as_deref().and_then(|s| decode_globals(s).ok());
                                if gg.map(|x| x.day_count > day).unwrap_or(false) || attempt == 1 {
                                    let rate = gg.map(|x| x.tshare_rate).unwrap_or(0.0);
                                    let price = r[1].as_deref()
                                        .and_then(|s| decode_price(s, state.hex_is_token0.load(Ordering::Relaxed)).ok())
                                        .unwrap_or(0.0);
                                    hb.fetch_max(block, Ordering::Relaxed);
                                    if let Ok(ts) = block_ts(state, block).await { ht.fetch_max(ts, Ordering::Relaxed); }
                                    return (day, Ok((rate, price)));
                                }
                                target += 3600;
                            }
                            (day, Err("unsettled".to_string()))
                        }
                    })
                    .buffer_unordered(BACKFILL_CONCURRENCY);
                let mut consec = 0u32;
                while let Some((day, outcome)) = results.next().await {
                    done += 1;
                    match outcome {
                        Ok((rate, mut price)) => {
                            consec = 0;
                            if price <= 0.0 && day == last_final && live_price > 0.0 { price = live_price; }
                            let e = by_day.entry(day).or_insert(HexJsonEntry { current_day: day, ..Default::default() });
                            if rate > 0.0 { e.tshare_rate_hex = rate; }
                            if price > 0.0 { e.price_pulse_x = price; }
                        }
                        Err(e) => {
                            consec += 1;
                            if consec <= 3 || consec % 20 == 0 { warn!("market day {day}: {e}"); }
                            if consec >= 50 { error!("catch-up aborted: 50 consecutive errors"); break; }
                        }
                    }
                    update_backfill(state, "market", total, done).await;
                    if done % BACKFILL_PUBLISH_INTERVAL == 0 {
                        publish_hexjson(state, by_day.values().cloned().collect()).await;
                    }
                }
            }
        }
    }
    publish_hexjson(state, by_day.values().cloned().collect()).await;
    {
        let mut s = state.status.write().await;
        s.backfill = BackfillStatus::default();
        s.last_daily_record = unix_now();
    }
    state.emit(AppEvent::Status { status: status_snapshot(state).await });
    info!("Catch-up complete: {} entries", by_day.len());
    errors == 0
}
async fn update_backfill(state: &Arc<AppState>, phase: &str, total: usize, done: usize) {
    { let mut s = state.status.write().await; s.backfill = BackfillStatus { running: true, phase: phase.into(), total, done }; }
    if done % 100 == 0 { info!("catch-up [{phase}] {done}/{total}"); }
}

// ============================================================
// WALLET
// ============================================================
async fn fetch_wallet_balances(state: &AppState, addrs: &[String]) -> Result<f64, String> {
    if addrs.is_empty() { return Ok(0.0); }
    let calls: Vec<Call> = addrs.iter().map(|a| balance_call(a)).collect();
    let res = eth_calls(state, None, &calls).await?;
    Ok(res.iter().flatten().filter_map(|s| word_f64(s, 0).ok()).sum::<f64>() / HEARTS_PER_HEX)
}
async fn fetch_wallet_miners(state: &AppState, addrs: &[String]) -> (Vec<Miner>, HashSet<String>) {
    let mut ok_addrs = HashSet::new();
    let mut out = Vec::new();
    if addrs.is_empty() { return (out, ok_addrs); }
    let count_calls: Vec<Call> = addrs.iter()
        .map(|a| call(HEX_CONTRACT, calldata(SEL_STAKE_COUNT, &[pad32(a)]))).collect();
    let counts = match eth_calls(state, None, &count_calls).await {
        Ok(r) => r, Err(e) => { warn!("stakeCount batch failed: {e}"); return (out, ok_addrs); }
    };
    for (addr, c) in addrs.iter().zip(counts) {
        let Some(cs) = c else { continue };
        let count = match word_u64(&cs, 0) { Ok(v) => v, Err(_) => continue };
        if count == 0 { ok_addrs.insert(addr.clone()); continue; }
        if count > MAX_STAKE_COUNT { warn!("{addr}: unrealistic stakeCount {count}, skipped"); continue; }
        let mut got_all = true;
        for chunk in (0..count).collect::<Vec<u64>>().chunks(MAX_CALLS_PER_REQ) {
            let calls: Vec<Call> = chunk.iter().map(|i| call(HEX_CONTRACT,
                calldata(SEL_STAKE_LISTS, &[pad32(addr), format!("{i:064x}")]))).collect();
            let res = match eth_calls(state, None, &calls).await {
                Ok(r) => r, Err(e) => { warn!("{addr}: stake chunk failed: {e}"); got_all = false; break; }
            };
            for r in res.into_iter().flatten() {
                let Ok(stake_id) = word_u64(&r, 0) else { continue };
                let Ok(shares_raw) = word_f64(&r, 2) else { continue };
                if stake_id == 0 && shares_raw == 0.0 { continue; }
                let (Ok(locked), Ok(days)) = (word_u64(&r, 3), word_u64(&r, 4)) else { continue };
                let unlocked = word_u64(&r, 5).unwrap_or(0);
                let hearts = word_f64(&r, 1).unwrap_or(0.0);
                let auto = word_u64(&r, 6).map(|v| v != 0).unwrap_or(false);
                let fmt = |d: u64| chrono::DateTime::from_timestamp(
                    HEX_DAY_ZERO_UNIX + (d as i64 * SECS_PER_DAY as i64), 0)
                    .unwrap_or(chrono::DateTime::UNIX_EPOCH).format("%d-%m-%Y").to_string();
                out.push(Miner {
                    id: None, address: addr.clone(), stake_id: Some(stake_id),
                    start_date: fmt(locked), end_date: fmt(locked + days),
                    t_shares: shares_raw / SHARES_PER_TSHARE,
                    staked_hex: hearts / HEARTS_PER_HEX,
                    auto_stake: auto, ended_on_chain: unlocked != 0,
                    status: if unlocked != 0 { Some("completed".into()) } else { None },
                });
            }
        }
        if got_all { ok_addrs.insert(addr.clone()); }
    }
    (out, ok_addrs)
}
fn merge_fetched_miners(
    current: &[Miner], fetched: Vec<Miner>, tracked: &HashSet<String>,
    ok_addrs: &HashSet<String>, next_id: &AtomicU64,
) -> Vec<Miner> {
    let mut by_key: HashMap<(String, u64), &Miner> = HashMap::new();
    let mut legacy: Vec<&Miner> = Vec::new();
    for m in current {
        match m.key() { Some(k) => { by_key.insert(k, m); }, None => legacy.push(m) }
    }
    let mut out: Vec<Miner> = Vec::new();
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    for mut f in fetched {
        let k = f.key().expect("fetched miners always have stakeId");
        let prior = by_key.get(&k).copied().or_else(|| legacy.iter().copied().find(|s| {
            s.address.eq_ignore_ascii_case(&f.address) && s.start_date == f.start_date
                && s.end_date == f.end_date && (s.t_shares - f.t_shares).abs() < 0.01
        }));
        f.id = prior.and_then(|p| p.id).or_else(|| Some(next_id.fetch_add(1, Ordering::Relaxed)));
        if f.ended_on_chain { f.status = Some("completed".into()); }
        else if prior.map(|p| p.is_completed()).unwrap_or(false) { f.status = Some("completed".into()); }
        seen.insert(k);
        out.push(f);
    }
    for m in current {
        let addr = m.address.to_lowercase();
        if addr.is_empty() { out.push(m.clone()); continue; }
        if !tracked.contains(&addr) { continue; }
        if let Some(k) = m.key() {
            if seen.contains(&k) { continue; }
            if ok_addrs.contains(&addr) {
                let mut c = m.clone();
                c.status = Some("completed".into());
                c.ended_on_chain = true;
                out.push(c);
            } else {
                out.push(m.clone());
            }
        } else if !out.iter().any(|o| o.id == m.id) {
            out.push(m.clone());
        }
    }
    out
}
fn miners_equivalent(a: &[Miner], b: &[Miner]) -> bool {
    let norm = |v: &[Miner]| {
        let mut t: Vec<(u64, String, Option<u64>, String, String, f64, Option<String>)> = v.iter()
            .map(|m| (m.id.unwrap_or(0), m.address.clone(), m.stake_id, m.start_date.clone(),
                      m.end_date.clone(), m.t_shares, m.status.clone())).collect();
        t.sort_by(|x, y| x.0.cmp(&y.0));
        t
    };
    let (na, nb) = (norm(a), norm(b));
    na.len() == nb.len() && na.iter().zip(&nb).all(|(x, y)| {
        x.0 == y.0 && x.1 == y.1 && x.2 == y.2 && x.3 == y.3 && x.4 == y.4
            && (x.5 - y.5).abs() < 0.001 && x.6 == y.6
    })
}
fn normalize_miners(mut miners: Vec<Miner>) -> (Vec<Miner>, u64) {
    let mut next = miners.iter().filter_map(|m| m.id).max().unwrap_or(0) + 1;
    for m in miners.iter_mut() {
        if m.id.is_none() { m.id = Some(next); next += 1; }
        m.address = m.address.to_lowercase();
    }
    (miners, next)
}
async fn refresh_wallet_data(state: &Arc<AppState>, addrs: &[String]) {
    let (bal, miners) = tokio::join!(fetch_wallet_balances(state, addrs), fetch_wallet_miners(state, addrs));
    match bal {
        Ok(b) => {
            let (cfg, ver) = {
                let mut c = state.config.write().await;
                c.liquid_hex = b;
                (c.clone(), state.config_version.fetch_add(1, Ordering::Relaxed) + 1)
            };
            state.live_data.write().await.liquid_hex = b;
            persist_config(&state.persister, ver, &cfg);
            state.emit(AppEvent::Config { config: cfg });
        }
        Err(e) => { warn!("balance fetch: {e}"); state.status.write().await.last_wallet_error = e; }
    }
    let (fetched, ok_addrs) = miners;
    let tracked: HashSet<String> = addrs.iter().cloned().collect();
    let changed = {
        let mut guard = state.miners.write().await;
        let merged = merge_fetched_miners(&guard, fetched, &tracked, &ok_addrs, &state.next_miner_id);
        if miners_equivalent(&guard, &merged) { false } else { *guard = merged; true }
    };
    if changed {
        let snapshot = state.miners.read().await.clone();
        let ver = state.miners_version.fetch_add(1, Ordering::Relaxed) + 1;
        persist_miners(&state.persister, ver, &snapshot);
        state.emit(AppEvent::Miners { count: snapshot.len() });
        info!("Wallet miners updated ({} records)", snapshot.len());
    }
    let mut s = state.status.write().await;
    s.last_wallet_refresh = unix_now();
    s.last_wallet_error.clear();
}

// ============================================================
// BACKGROUND TASKS
// ============================================================
async fn live_data_updater(state: Arc<AppState>) {
    let mut rx = state.config_tx.subscribe();
    match fetch_live_data_retry(&state).await {
        Ok(d) => publish_live(&state, d).await,
        Err(e) => { error!("initial live fetch: {e}"); state.status.write().await.last_live_error = e; }
    }
    loop {
        let freq = state.config.read().await.live_data_frequency.max(1);
        let wait = Duration::from_secs(freq * 60) + Duration::from_millis(jitter_ms(1500));
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = tokio::time::sleep(wait) => {}
            r = rx.recv() => { if r.is_err() { continue; } }
        }
        if state.shutdown.is_cancelled() { break; }
        match fetch_live_data_retry(&state).await {
            Ok(d) => publish_live(&state, d).await,
            Err(e) => { warn!("live fetch: {e}"); state.status.write().await.last_live_error = e; }
        }
    }
}
async fn price_sampler(state: Arc<AppState>) {
    loop {
        let wait = Duration::from_secs(PRICE_SAMPLE_SECS) + Duration::from_millis(jitter_ms(1000));
        tokio::select! { _ = state.shutdown.cancelled() => break, _ = tokio::time::sleep(wait) => {} }
        if state.shutdown.is_cancelled() { break; }
        if let Ok(p) = fetch_price_rpc(&state).await { record_live_price_point(&state, p).await; }
    }
}
async fn hex_json_updater(state: Arc<AppState>) {
    if let Ok(txt) = tokio::fs::read_to_string(hexjson_path()).await {
        match serde_json::from_str::<Vec<HexJsonEntry>>(&txt) {
            Ok(v) => { info!("Loaded {} hexjson entries", v.len()); publish_hexjson(&state, v).await; }
            Err(e) => warn!("hexjson.json corrupt ({e}); rebuilding"),
        }
    }
    load_live_chart(&state).await;
    catch_up_hexjson(&state).await;
    let mut retry_at: Option<tokio::time::Instant> = None;
    loop {
        let now = Utc::now();
        let mut next_1am = now.date_naive().and_time(NaiveTime::from_hms_opt(1, 0, 0).unwrap()).and_utc();
        if next_1am <= now { next_1am = next_1am.checked_add_days(Days::new(1)).unwrap(); }
        let secs = (next_1am - now).to_std().unwrap_or(Duration::from_secs(3600)).as_secs() + 120;
        let mut wake = tokio::time::Instant::now() + Duration::from_secs(secs);
        if let Some(t) = retry_at { wake = wake.min(t); }
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = tokio::time::sleep_until(wake) => {}
            _ = state.backfill.notified() => { info!("Config change triggered hexjson catch-up"); }
        }
        if state.shutdown.is_cancelled() { break; }
        retry_at = None;
        if !catch_up_hexjson(&state).await {
            retry_at = Some(tokio::time::Instant::now() + Duration::from_secs(DAILY_RETRY_SECS));
        }
    }
}
async fn wallet_updater(state: Arc<AppState>) {
    let mut rx = state.config_tx.subscribe();
    let mut current: Vec<String> = Vec::new();
    loop {
        let addrs = parse_addresses(&state.config.read().await.wallet_addresses);
        if addrs != current {
            current = addrs.clone();
            if !addrs.is_empty() { refresh_wallet_data(&state, &addrs).await; }
        }
        if addrs.is_empty() {
            tokio::select! { _ = state.shutdown.cancelled() => break, _ = rx.recv() => {} }
            continue;
        }
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(WALLET_REFRESH_SECS)) => {
                refresh_wallet_data(&state, &current).await;
            }
            _ = rx.recv() => {}
        }
    }
}
async fn chart_janitor(state: Arc<AppState>) {
    let mut last_persist = Instant::now();
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(60)) => {} }
        { let mut pts = state.live_chart.write().await; compact_series(&mut pts, unix_now()); }
        if last_persist.elapsed() >= Duration::from_secs(LIVE_PERSIST_SECS) {
            persist_live_chart(&state).await;
            last_persist = Instant::now();
        }
    }
    persist_live_chart(&state).await;
}
async fn status_broadcaster(state: Arc<AppState>) {
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(30)) => {} }
        state.emit(AppEvent::Status { status: status_snapshot(&state).await });
    }
}
async fn status_snapshot(state: &AppState) -> Status {
    let mut s = state.status.read().await.clone();
    let cfg = state.config.read().await.clone();
    let live = state.live_data.read().await.clone();
    let hj = state.hex_json.read().await.clone();
    let miners = state.miners.read().await.clone();
    let pts = state.live_chart.read().await;
    s.version = env!("CARGO_PKG_VERSION").into();
    s.uptime_secs = state.started.elapsed().as_secs();
    s.active_rpc = state.rpc.endpoints.read().await[state.rpc.active.load(Ordering::Relaxed)].url.into();
    s.rpc = state.rpc.snapshot().await;
    s.multicall = state.multicall.load(Ordering::Relaxed);
    s.sample_secs = PRICE_SAMPLE_SECS;
    s.live_points = pts.len();
    s.live_first_ts = pts.front().map(|p| p.ts).unwrap_or(0);
    s.live_last_ts = pts.back().map(|p| p.ts).unwrap_or(0);
    s.hexjson_days = hj.len();
    s.hexjson_first_day = hj.first().map(|e| e.current_day).unwrap_or(0);
    s.hexjson_last_day = hj.last().map(|e| e.current_day).unwrap_or(0);
    s.hex_day_today = hex_today();
    s.miners_active = miners.iter().filter(|m| !m.is_completed()).count();
    s.miners_completed = miners.iter().filter(|m| m.is_completed()).count();
    s.wallet_addresses = parse_addresses(&cfg.wallet_addresses).len();
    if s.last_live_fetch == 0 { s.last_live_fetch = live.updated_at; }
    s
}

// ============================================================
// HANDLERS
// ============================================================
async fn handle_live_data(State(state): State<Arc<AppState>>) -> Json<LiveData> {
    let mut d = state.live_data.read().await.clone();
    let freq = state.config.read().await.live_data_frequency.max(1);
    d.stale = d.updated_at == 0 || unix_now().saturating_sub(d.updated_at) > freq * 120;
    Json(d)
}
#[derive(Deserialize)]
struct LiveChartQuery { range: Option<u64>, interval: Option<u64> }
async fn handle_live_chart(State(state): State<Arc<AppState>>, Query(q): Query<LiveChartQuery>) -> Json<Arc<LiveChartResponse>> {
    let cfg = state.config.read().await.clone();
    let range = q.range.unwrap_or(cfg.live_chart_range_minutes).clamp(5, 7 * 24 * 60);
    let min_i = range.div_ceil(MAX_CANDLES as u64).max(1);
    let interval = q.interval.unwrap_or(cfg.live_chart_candle_minutes).clamp(min_i, range);
    let version = state.live_chart_version.load(Ordering::Relaxed);
    let bucket = unix_now() / (interval * 60);
    let key = (version, range, interval, bucket);
    if let Some(c) = state.live_chart_cache.read().await.as_ref() {
        if c.key == key { return Json(Arc::clone(&c.response)); }
    }
    let resp = Arc::new(build_live_chart(&state, range, interval).await);
    *state.live_chart_cache.write().await = Some(CachedLiveChart { key, response: Arc::clone(&resp) });
    Json(resp)
}
async fn handle_miners(State(state): State<Arc<AppState>>) -> Json<Vec<Miner>> { Json(state.miners.read().await.clone()) }
async fn handle_get_config(State(state): State<Arc<AppState>>) -> Json<Config> { Json(state.config.read().await.clone()) }
async fn handle_status(State(state): State<Arc<AppState>>) -> Json<Status> { Json(status_snapshot(&state).await) }
async fn handle_refresh(State(state): State<Arc<AppState>>) -> Response {
    match fetch_live_data_retry(&state).await {
        Ok(d) => { publish_live(&state, d.clone()).await; Json(d).into_response() }
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}
async fn handle_post_config(State(state): State<Arc<AppState>>, Json(new): Json<Config>) -> impl IntoResponse {
    let new = sanitize_config(new);
    let (hist_changed, ver) = {
        let mut c = state.config.write().await;
        let hist_changed = new.historical_start_day < c.historical_start_day;
        *c = new.clone();
        (hist_changed, state.config_version.fetch_add(1, Ordering::Relaxed) + 1)
    };
    state.live_data.write().await.liquid_hex = new.liquid_hex;
    persist_config(&state.persister, ver, &new);
    state.emit(AppEvent::Config { config: new });
    let _ = state.config_tx.send(());
    if hist_changed { state.backfill.notify_one(); }
    StatusCode::OK
}
#[derive(Deserialize)]
struct HexJsonQuery { from: Option<u64>, limit: Option<usize> }
async fn handle_hex_json(State(state): State<Arc<AppState>>, Query(q): Query<HexJsonQuery>, headers: HeaderMap) -> Response {
    let data = Arc::clone(&*state.hex_json.read().await);
    let version = state.hex_json_version.load(Ordering::Relaxed);
    let full = q.from.is_none() && q.limit.is_none();
    let etag = format!("\"hj-{}-{}-{}-{}\"", q.from.unwrap_or(0), q.limit.unwrap_or(0), data.len(), version);
    if let Some(inm) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        let matches = inm.split(',').any(|t| {
            let t = t.trim().trim_start_matches("W/");
            t == etag || t == "*"
        });
        if matches {
            return Response::builder().status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, &etag).header(header::CACHE_CONTROL, "no-cache")
                .body(axum::body::Body::empty()).unwrap_or_else(|_| StatusCode::NOT_MODIFIED.into_response());
        }
    }
    let mut resp = if full { Json(data).into_response() } else {
        let from = q.from.unwrap_or(0);
        let it = data.iter().filter(|e| e.current_day >= from);
        let v: Vec<HexJsonEntry> = match q.limit { Some(n) => it.take(n).cloned().collect(), None => it.cloned().collect() };
        Json(v).into_response()
    };
    if let Ok(hv) = header::HeaderValue::from_str(&etag) { resp.headers_mut().insert(header::ETAG, hv); }
    resp.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
    resp
}
fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let d = NaiveDate::parse_from_str(s.trim(), "%d-%m-%Y").ok()?;
    if !(2000..=2100).contains(&d.year()) { return None; }
    Some((d.year(), d.month(), d.day()))
}
async fn handle_add_miner(State(state): State<Arc<AppState>>, Json(req): Json<AddMinerRequest>) -> impl IntoResponse {
    let (Some(s), Some(e)) = (parse_date(&req.start_date), parse_date(&req.end_date)) else { return StatusCode::BAD_REQUEST };
    if e < s || !req.t_shares.is_finite() || req.t_shares <= 0.0 || req.t_shares > 1e12 { return StatusCode::BAD_REQUEST }
    let miner = Miner {
        id: Some(state.next_miner_id.fetch_add(1, Ordering::Relaxed)),
        address: String::new(), stake_id: None,
        start_date: req.start_date.trim().into(), end_date: req.end_date.trim().into(),
        t_shares: req.t_shares, staked_hex: 0.0, auto_stake: false, ended_on_chain: false, status: None,
    };
    let snapshot = {
        let mut m = state.miners.write().await;
        if m.iter().any(|x| x.address.is_empty() && x.start_date == miner.start_date
            && x.end_date == miner.end_date && (x.t_shares - miner.t_shares).abs() < 0.001) {
            return StatusCode::CONFLICT;
        }
        if m.len() >= 1000 { return StatusCode::BAD_REQUEST }
        m.push(miner);
        m.clone()
    };
    let ver = state.miners_version.fetch_add(1, Ordering::Relaxed) + 1;
    persist_miners(&state.persister, ver, &snapshot);
    state.emit(AppEvent::Miners { count: snapshot.len() });
    StatusCode::CREATED
}
async fn handle_end_miner(State(state): State<Arc<AppState>>, Json(req): Json<MinerIdRequest>) -> impl IntoResponse {
    let snapshot = {
        let mut m = state.miners.write().await;
        let Some(x) = m.iter_mut().find(|x| x.id == Some(req.id)) else { return StatusCode::NOT_FOUND };
        x.status = Some("completed".into());
        m.clone()
    };
    let ver = state.miners_version.fetch_add(1, Ordering::Relaxed) + 1;
    persist_miners(&state.persister, ver, &snapshot);
    state.emit(AppEvent::Miners { count: snapshot.len() });
    StatusCode::OK
}
async fn handle_delete_miner(State(state): State<Arc<AppState>>, Json(req): Json<MinerIdRequest>) -> impl IntoResponse {
    let snapshot = {
        let mut m = state.miners.write().await;
        let before = m.len();
        m.retain(|x| x.id != Some(req.id));
        if m.len() == before { return StatusCode::NOT_FOUND }
        m.clone()
    };
    let ver = state.miners_version.fetch_add(1, Ordering::Relaxed) + 1;
    persist_miners(&state.persister, ver, &snapshot);
    state.emit(AppEvent::Miners { count: snapshot.len() });
    StatusCode::OK
}
async fn handle_events(State(state): State<Arc<AppState>>)
    -> Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    let live = state.live_data.read().await.clone();
    let status = status_snapshot(&state).await;
    let hello = stream::once(async move {
        Ok(SseEvent::default().event("live").json_data(AppEvent::Live { data: live })
            .unwrap_or_default())
    });
    let status_ev = stream::once(async move {
        Ok(SseEvent::default().event("status").json_data(AppEvent::Status { status })
            .unwrap_or_default())
    });
    let rx = state.events.subscribe();
    let updates = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|r| async move {
        match r {
            Ok(ev) => Some(Ok(SseEvent::default().json_data(ev).unwrap_or_default())),
            Err(_) => None,
        }
    });
    Sse::new(hello.chain(status_ev).chain(updates))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(20)).text("ping"))
}
fn get_mime_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" | "webmanifest" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "png" => "image/png", "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml", "ico" => "image/x-icon",
        "woff2" => "font/woff2", "woff" => "font/woff", "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}
async fn handle_static(uri: axum::http::Uri) -> Response {
    let p = uri.path().trim_start_matches('/');
    let p = if p.is_empty() { "index.html" } else { p };
    if p.split('/').any(|s| s == ".." || s == ".") { return StatusCode::NOT_FOUND.into_response(); }
    let Some(c) = Assets::get(p) else { return StatusCode::NOT_FOUND.into_response() };
    let cache = if p == "index.html" { "no-cache" } else { "public, max-age=300" };
    let body = match c.data {
        std::borrow::Cow::Borrowed(b) => axum::body::Body::from(b),
        std::borrow::Cow::Owned(v) => axum::body::Body::from(v),
    };
    Response::builder().status(StatusCode::OK)
        .header(header::CONTENT_TYPE, get_mime_type(p))
        .header(header::CACHE_CONTROL, cache)
        .header("X-Content-Type-Options", "nosniff")
        .body(body).unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ============================================================
// MAIN
// ============================================================
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))).init();
    tokio::fs::create_dir_all(data_dir()).await.expect("cannot create data dir");

    let initial_config = sanitize_config(
        tokio::fs::read_to_string(config_path()).await.ok()
            .and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default());
    let raw_miners: Vec<Miner> = tokio::fs::read_to_string(miners_path()).await.ok()
        .and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default();
    let (initial_miners, next_id) = normalize_miners(raw_miners);

    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(5))
        .pool_max_idle_per_host(16)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(30))
        .user_agent("hextrack/1.1")
        .build().unwrap();

    let (ptx, prx) = mpsc::unbounded_channel();
    let (etx, _) = broadcast::channel(256);
    let (ctx, _) = broadcast::channel(16);
    let shutdown = CancellationToken::new();

    let state = Arc::new(AppState {
        client, rpc: RpcPool::new(),
        config: RwLock::new(initial_config),
        live_data: RwLock::new(LiveData::default()),
        hex_json: RwLock::new(Arc::<[HexJsonEntry]>::from(Vec::new())),
        miners: RwLock::new(initial_miners),
        status: RwLock::new(Status::default()),
        events: etx, config_tx: ctx, backfill: Arc::new(Notify::new()),
        hex_json_version: AtomicU64::new(1),
        config_version: AtomicU64::new(1),
        miners_version: AtomicU64::new(1),
        next_miner_id: AtomicU64::new(next_id),
        live_chart: RwLock::new(VecDeque::new()),
        live_chart_cache: RwLock::new(None),
        live_chart_version: AtomicU64::new(0),
        globals_cache: Mutex::new(None),
        multicall: AtomicBool::new(false),
        hex_is_token0: AtomicBool::new(false),
        persister: Persister { tx: ptx },
        shutdown: shutdown.clone(),
        started: Instant::now(),
    });

    {
        let code = rpc_call(&state, "eth_getCode", serde_json::json!([MULTICALL3, "latest"])).await;
        let has = code.ok().and_then(|v| v.as_str().map(|s| strip0x(s).len() > 2)).unwrap_or(false);
        state.multicall.store(has, Ordering::Relaxed);
        info!("Multicall3: {}", if has { "available (single-call snapshots)" } else { "absent (pinned-block batches)" });
        if let Ok(Some(t0)) = eth_calls(&state, None, &[call(USDC_HEX_PAIR, calldata(SEL_TOKEN0, &[]))])
            .await.map(|mut v| v.pop().flatten())
        {
            if let Ok(w) = word(&t0, 0) {
                let is_hex = strip0x(w).to_lowercase().ends_with(strip0x(HEX_CONTRACT));
                state.hex_is_token0.store(is_hex, Ordering::Relaxed);
                info!("Pair token0 is {}", if is_hex { "HEX" } else { "USDC" });
            }
        }
    }

    let writer = tokio::spawn(persistence_worker(prx));
    let tasks = vec![
        tokio::spawn(live_data_updater(state.clone())),
        tokio::spawn(price_sampler(state.clone())),
        tokio::spawn(hex_json_updater(state.clone())),
        tokio::spawn(wallet_updater(state.clone())),
        tokio::spawn(chart_janitor(state.clone())),
        tokio::spawn(status_broadcaster(state.clone())),
    ];

    let app = Router::new()
        .route("/api/live-data", get(handle_live_data))
        .route("/api/live-chart", get(handle_live_chart))
        .route("/api/hexjson", get(handle_hex_json))
        .route("/api/miners", get(handle_miners))
        .route("/api/config", get(handle_get_config).post(handle_post_config))
        .route("/api/add-miner", post(handle_add_miner))
        .route("/api/end-miner", post(handle_end_miner))
        .route("/api/delete-miner", post(handle_delete_miner))
        .route("/api/refresh", post(handle_refresh))
        .route("/api/status", get(handle_status))
        .route("/api/events", get(handle_events))
        .fallback(get(handle_static))
        .layer(CompressionLayer::new())
        .with_state(state.clone());

    let addr: SocketAddr = std::env::var("HEXTRACK_BIND")
        .ok().and_then(|s| s.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 5555)));
    info!("⬢ HEXTRACK {} on {addr} (data: {}) ⬢", env!("CARGO_PKG_VERSION"), data_dir().display());
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).with_graceful_shutdown({
        let sd = shutdown.clone();
        async move { let _ = tokio::signal::ctrl_c().await; info!("Shutdown requested"); sd.cancel(); }
    }).await.unwrap();

    for t in tasks { let _ = tokio::time::timeout(Duration::from_secs(5), t).await; }
    persist_live_chart(&state).await;
    drop(state);
    let _ = tokio::time::timeout(Duration::from_secs(10), writer).await;
    info!("Shutdown complete");
}
