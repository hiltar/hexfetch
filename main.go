package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"embed"
	"fmt"
	"io/fs"
	"log"
	"math/big"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/cenkalti/backoff/v5"
)

//go:embed static
var staticFiles embed.FS

// =============================================
// CONFIGURATION
// =============================================
const (
	dataDir                  = "/opt/hexfetch"
	dateLayout               = "02-01-2006"
	defaultLiveDataFrequency = 15

	// Direct RPC & DexScreener Endpoints
	RpcURL         = "https://rpc.pulsechain.com"
	HexContract    = "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39"
	DexScreenerURL = "https://api.dexscreener.com/latest/dex/tokens/0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39"
)

// Global cached data
var (
	latestLiveData  LiveData
	liveDataMutex   sync.RWMutex
	hexJSONData     HEXJSON
	hexJSONMutex    sync.RWMutex
	httpClient      = &http.Client{
		Timeout: 10 * time.Second,
		Transport: &http.Transport{
			MaxIdleConns:       10,
			IdleConnTimeout:    30 * time.Second,
			DisableCompression: true,
		},
	}

	// Config notification
	configSubsMu sync.Mutex
	configSubs   []chan struct{}

	// Optimized Miner Cache
	cachedMiners  []Miner
	minersCacheMu sync.RWMutex
)

// =============================================
// DATA STRUCTURES
// =============================================
type HEXJSONEntry struct {
	CurrentDay         int     `json:"currentDay"`
	TshareRateHEX      float64 `json:"tshareRateHEX"`
	DailyPayoutHEX     float64 `json:"dailyPayoutHEX"`
	PayoutPerTshareHEX float64 `json:"payoutPerTshareHEX"`
	PricePulseX        float64 `json:"pricePulseX"`
}

type HEXJSON []HEXJSONEntry

type LiveData struct {
	PricePulsechain           float64 `json:"price_Pulsechain"`
	TsharePricePulsechain     float64 `json:"tsharePrice_Pulsechain"`
	TshareRateHEXPulsechain   float64 `json:"tshareRateHEX_Pulsechain"`
	PenaltiesHEXPulsechain    float64 `json:"penaltiesHEX_Pulsechain"`
	PayoutPerTsharePulsechain float64 `json:"payoutPerTshare_Pulsechain"`
	Beat                      float64 `json:"beat"`
}

type Miner struct {
	StartDate string  `json:"startDate"`
	EndDate   string  `json:"endDate"`
	TShares   float64 `json:"tShares"`
	Status    string  `json:"status,omitempty"`
}

type Config struct {
	LiveDataFrequency  int     `json:"liveDataFrequency"`
	LiquidHEX          float64 `json:"liquidHEX"`
	HistoricalStartDay int     `json:"historicalStartDay"`
}

// RPC & DexScreener Structures
type RPCRequest struct {
	Jsonrpc string      `json:"jsonrpc"`
	Method  string      `json:"method"`
	Params  interface{} `json:"params"`
	ID      int         `json:"id"`
}

type RPCResponse struct {
	Jsonrpc string    `json:"jsonrpc"`
	ID      int       `json:"id"`
	Result  string    `json:"result"`
	Error   *RPCError `json:"error,omitempty"`
}

type RPCError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

type DexPair struct {
	PriceUsd interface{} `json:"priceUsd"`
}

type DexResponse struct {
	Pairs []DexPair `json:"pairs"`
}

// =============================================
// CONFIG MANAGER
// =============================================
type ConfigManager struct {
	mu     sync.RWMutex
	config Config
}

var configManager = &ConfigManager{
	config: Config{
		LiveDataFrequency:  defaultLiveDataFrequency,
		HistoricalStartDay: 1260,
	},
}

func (cm *ConfigManager) GetLiveDataFrequency() int {
	cm.mu.RLock()
	defer cm.mu.RUnlock()
	if cm.config.LiveDataFrequency <= 0 {
		return defaultLiveDataFrequency
	}
	return cm.config.LiveDataFrequency
}

func (cm *ConfigManager) SetLiveDataFrequency(frequency int) {
	cm.mu.Lock()
	defer cm.mu.Unlock()
	if frequency <= 0 {
		return
	}
	cm.config.LiveDataFrequency = frequency
	cm.broadcastChange()
}

func (cm *ConfigManager) GetHistoricalStartDay() int {
	cm.mu.RLock()
	defer cm.mu.RUnlock()
	if cm.config.HistoricalStartDay < 1 {
		return 1260
	}
	return cm.config.HistoricalStartDay
}

func (cm *ConfigManager) SetHistoricalStartDay(day int) {
	cm.mu.Lock()
	defer cm.mu.Unlock()
	if day < 1 {
		day = 1260
	}
	cm.config.HistoricalStartDay = day
	cm.broadcastChange()
}

func (cm *ConfigManager) GetLiquidHEX() float64 {
	cm.mu.RLock()
	defer cm.mu.RUnlock()
	return cm.config.LiquidHEX
}

func (cm *ConfigManager) SetLiquidHEX(val float64) {
	cm.mu.Lock()
	defer cm.mu.Unlock()
	if val >= 0 {
		cm.config.LiquidHEX = val
	}
	cm.broadcastChange()
}

func (cm *ConfigManager) Subscribe() chan struct{} {
	ch := make(chan struct{}, 1)
	configSubsMu.Lock()
	configSubs = append(configSubs, ch)
	configSubsMu.Unlock()
	return ch
}

func (cm *ConfigManager) Unsubscribe(ch chan struct{}) {
	configSubsMu.Lock()
	defer configSubsMu.Unlock()
	for i, sub := range configSubs {
		if sub == ch {
			configSubs[i] = configSubs[len(configSubs)-1]
			configSubs = configSubs[:len(configSubs)-1]
			close(ch)
			return
		}
	}
}

func (cm *ConfigManager) broadcastChange() {
	configSubsMu.Lock()
	subs := make([]chan struct{}, len(configSubs))
	copy(subs, configSubs)
	configSubsMu.Unlock()
	for _, ch := range subs {
		select {
		case ch <- struct{}{}:
		default:
		}
	}
}

// =============================================
// HELPERS
// =============================================
func debugLog(v ...any) {
	if os.Getenv("DEBUG") == "true" {
		log.Println(v...)
	}
}

func getMaxDay(data HEXJSON) int {
	if len(data) == 0 {
		return 0
	}
	max := data[0].CurrentDay
	for _, e := range data {
		if e.CurrentDay > max {
			max = e.CurrentDay
		}
	}
	return max
}

func HexToBigInt(hexStr string) *big.Int {
	hexStr = strings.TrimPrefix(hexStr, "0x")
	if len(hexStr)%2 != 0 {
		hexStr = "0" + hexStr
	}
	b, _ := hex.DecodeString(hexStr)
	return new(big.Int).SetBytes(b)
}

func CallRPC(method string, params interface{}) (string, error) {
	reqBody := RPCRequest{
		Jsonrpc: "2.0",
		Method:  method,
		Params:  params,
		ID:      1,
	}

	body, err := json.Marshal(reqBody)
	if err != nil {
		return "", err
	}

	resp, err := httpClient.Post(RpcURL, "application/json", bytes.NewBuffer(body))
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	var rpcResp RPCResponse
	if err := json.NewDecoder(resp.Body).Decode(&rpcResp); err != nil {
		return "", err
	}

	if rpcResp.Error != nil {
		return "", fmt.Errorf("RPC error: %s", rpcResp.Error.Message)
	}

	return rpcResp.Result, nil
}

func GetHexPrice() (float64, error) {
	resp, err := httpClient.Get(DexScreenerURL)
	if err != nil {
		return 0, err
	}
	defer resp.Body.Close()

	var dexResp DexResponse
	if err := json.NewDecoder(resp.Body).Decode(&dexResp); err != nil {
		return 0, err
	}

	if len(dexResp.Pairs) == 0 {
		return 0, fmt.Errorf("no pairs found")
	}

	priceRaw := dexResp.Pairs[0].PriceUsd
	switch v := priceRaw.(type) {
	case float64:
		return v, nil
	case string:
		var f float64
		fmt.Sscanf(v, "%f", &f)
		return f, nil
	}
	return 0, fmt.Errorf("unknown price format")
}

// =============================================
// DATA FETCHING
// =============================================
func fetchHEXJSON() (HEXJSON, error) {
	var data HEXJSON
	b := backoff.NewExponentialBackOff()
	b.MaxElapsedTime = 5 * time.Minute
	err := backoff.Retry(func() error {
		resp, err := httpClient.Get("https://hexdailystats.com/fulldatapulsechain")
		if err != nil {
			return err
		}
		defer resp.Body.Close()
		if resp.StatusCode != http.StatusOK {
			return fmt.Errorf("status %d", resp.StatusCode)
		}
		return json.NewDecoder(resp.Body).Decode(&data)
	}, b)
	return data, err
}

// fetchLiveData fetches data directly from PulseChain RPC and DexScreener
func fetchLiveData() (LiveData, error) {
	var data LiveData
	b := backoff.NewExponentialBackOff()
	b.MaxElapsedTime = 5 * time.Minute
	err := backoff.Retry(func() error {
		var res LiveData

		price, err := GetHexPrice()
		if err != nil {
			debugLog("DexScreener price error:", err)
			return err
		}
		res.PricePulsechain = price

		// 1 Beat = 1 Gwei = 1,000,000,000 Wei
		gasPriceHex, err := CallRPC("eth_gasPrice", []interface{}{})
		if err != nil {
			debugLog("RPC eth_gasPrice error:", err)
			return err
		}
		gasPriceWei := HexToBigInt(gasPriceHex)
		beatsFloat, _ := new(big.Float).Quo(
			new(big.Float).SetInt(gasPriceWei),
			big.NewFloat(1e9),
		).Float64()
		res.Beat = beatsFloat

		// Selector for globals(): 0xc3124525
		globalsRes, err := CallRPC("eth_call", []interface{}{
			map[string]string{"to": HexContract, "data": "0xc3124525"},
			"latest",
		})
		if err != nil {
			debugLog("RPC globals() error:", err)
			return err
		}

		globalsRes = strings.TrimPrefix(globalsRes, "0x")
		if len(globalsRes) >= 5*64 {
			shareRate := HexToBigInt(globalsRes[2*64 : 3*64])
			penaltyTotal := HexToBigInt(globalsRes[3*64 : 4*64])
			dailyDataCount := HexToBigInt(globalsRes[4*64 : 5*64])

			if shareRate.Cmp(big.NewInt(0)) > 0 {
				// tshareRateHEX = shareRate / 10.0
				tshareRateFloat := float64(shareRate.Int64()) / 10.0
				res.TshareRateHEXPulsechain = tshareRateFloat
				res.TsharePricePulsechain = tshareRateFloat * res.PricePulsechain
			}

			penaltiesFloat, _ := new(big.Float).Quo(
				new(big.Float).SetInt(penaltyTotal),
				big.NewFloat(1e8), // HEX has 8 decimals
			).Float64()
			res.PenaltiesHEXPulsechain = penaltiesFloat

			dayToQuery := new(big.Int).Sub(dailyDataCount, big.NewInt(1))
			if dayToQuery.Cmp(big.NewInt(0)) < 0 {
				dayToQuery = big.NewInt(0)
			}

			// Selector for dailyData(uint256): 0x90de6871
			dayPadded := fmt.Sprintf("%064x", dayToQuery)
			dailyRes, err := CallRPC("eth_call", []interface{}{
				map[string]string{"to": HexContract, "data": "0x90de6871" + dayPadded},
				"latest",
			})

			dayPayoutTotal := big.NewInt(0)
			dayStakeSharesTotal := big.NewInt(0)

			if err == nil {
				dailyRes = strings.TrimPrefix(dailyRes, "0x")
				if len(dailyRes) >= 3*64 {
					dayPayoutTotal = HexToBigInt(dailyRes[0*64 : 1*64])
					dayStakeSharesTotal = HexToBigInt(dailyRes[1*64 : 2*64])
				}
			}

			// Fallback to current day if previous day data is still 0
			if dayPayoutTotal.Cmp(big.NewInt(0)) == 0 {
				dayPaddedCurr := fmt.Sprintf("%064x", dailyDataCount)
				currDailyRes, err := CallRPC("eth_call", []interface{}{
					map[string]string{"to": HexContract, "data": "0x90de6871" + dayPaddedCurr},
					"latest",
				})
				if err == nil {
					currDailyRes = strings.TrimPrefix(currDailyRes, "0x")
					if len(currDailyRes) >= 3*64 {
						dayPayoutTotal = HexToBigInt(currDailyRes[0*64 : 1*64])
						dayStakeSharesTotal = HexToBigInt(currDailyRes[1*64 : 2*64])
					}
				}
			}

			if dayStakeSharesTotal.Cmp(big.NewInt(0)) > 0 {
				payoutNum := new(big.Float).SetInt(dayPayoutTotal)
				payoutDen := new(big.Float).SetInt(dayStakeSharesTotal)

				resFloat := new(big.Float).Quo(payoutNum, payoutDen)
				resFloat = resFloat.Mul(resFloat, big.NewFloat(10000.0))

				payoutPerTShareFloat, _ := resFloat.Float64()
				res.PayoutPerTsharePulsechain = payoutPerTShareFloat
			}
		} else {
			return fmt.Errorf("invalid globals() response length")
		}

		data = res
		debugLog("LiveData fetched successfully via RPC, beat:", data.Beat)
		return nil
	}, b)

	return data, err
}

// startLiveDataBackgroundUpdater starts a background goroutine that fetches live data
// at the frequency defined in ConfigManager. It automatically restarts when frequency changes.
func startLiveDataBackgroundUpdater() {
	var ticker *time.Ticker
	var stopChan = make(chan struct{})
	configSub := configManager.Subscribe()

	go func() {
		defer configManager.Unsubscribe(configSub)

		if data, err := fetchLiveData(); err == nil {
			liveDataMutex.Lock()
			latestLiveData = data
			liveDataMutex.Unlock()
			debugLog("Initial live data fetched successfully")
		} else {
			debugLog("Initial live data fetch failed:", err)
		}

		for {
			frequency := configManager.GetLiveDataFrequency()
			if ticker != nil {
				ticker.Stop()
			}
			ticker = time.NewTicker(time.Duration(frequency) * time.Minute)

			select {
			case <-ticker.C:
				if data, err := fetchLiveData(); err == nil {
					liveDataMutex.Lock()
					latestLiveData = data
					liveDataMutex.Unlock()
				} else {
					debugLog("Background live data fetch failed:", err)
				}

			case <-configSub:
				debugLog("Live data frequency changed, restarting updater...")
				continue

			case <-stopChan:
				ticker.Stop()
				return
			}
		}
	}()
}

// =============================================
// LOCAL STORAGE
// =============================================
func loadLocalHEXJSON() (HEXJSON, error) {
	hexJSONMutex.RLock()
	defer hexJSONMutex.RUnlock()
	return hexJSONData, nil
}

func saveLocalHEXJSON(data HEXJSON) {
	hexJSONMutex.Lock()
	sort.Slice(data, func(i, j int) bool {
		return data[i].CurrentDay < data[j].CurrentDay
	})
	hexJSONData = data
	hexJSONMutex.Unlock()
}

func updateLocalHEXJSON() error {
	local, _ := loadLocalHEXJSON()
	remote, err := fetchHEXJSON()
	if err != nil {
		return err
	}
	if len(local) == 0 {
		saveLocalHEXJSON(remote)
		return nil
	}
	localMax := getMaxDay(local)
	var newEntries []HEXJSONEntry
	for _, entry := range remote {
		if entry.CurrentDay > localMax {
			newEntries = append(newEntries, entry)
		}
	}

	if len(newEntries) > 0 {
		updated := append(newEntries, local...)
		saveLocalHEXJSON(updated)
	}
	return nil
}

func startDailyHEXJSONUpdate() {
	go func() {
		for {
			now := time.Now().UTC()
			nextUpdate := now.Truncate(24*time.Hour).Add(27 * time.Hour)
			time.Sleep(nextUpdate.Sub(now))
			debugLog("Running daily HEXJSON update...")
			if err := updateLocalHEXJSON(); err != nil {
				debugLog("HEXJSON update failed:", err)
			} else {
				debugLog("HEXJSON updated successfully")
			}
		}
	}()
}

// =============================================
// MINERS & CONFIG DISK IO
// =============================================
func initializeMiners() {
	path := filepath.Join(dataDir, "miners.json")
	file, err := os.Open(path)
	if err != nil {
		if os.IsNotExist(err) {
			minersCacheMu.Lock()
			cachedMiners = []Miner{}
			minersCacheMu.Unlock()
			return
		}
		log.Printf("Error opening miners file: %v", err)
		return
	}
	defer file.Close()
	var miners []Miner
	if err := json.NewDecoder(file).Decode(&miners); err != nil {
		log.Printf("Error decoding miners file: %v", err)
		return
	}

	minersCacheMu.Lock()
	cachedMiners = miners
	minersCacheMu.Unlock()
	debugLog("Loaded miners into RAM cache.")
}

func persistMiners(miners []Miner) error {
	current, _ := loadMinersFromDisk()
	if reflect.DeepEqual(current, miners) {
		return nil
	}
	path := filepath.Join(dataDir, "miners.json")
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0644)
	if err != nil {
		return err
	}

	enc := json.NewEncoder(file)
	enc.SetIndent("", "  ")
	if err := enc.Encode(miners); err != nil {
		file.Close()
		return err
	}

	return file.Close()
}

func loadConfig() (Config, error) {
	path := filepath.Join(dataDir, "config.json")
	file, err := os.Open(path)
	if err != nil {
		if os.IsNotExist(err) {
			return Config{LiveDataFrequency: defaultLiveDataFrequency, LiquidHEX: 0, HistoricalStartDay: 1260}, nil
		}
		return Config{}, err
	}
	defer file.Close()
	var cfg Config
	if err := json.NewDecoder(file).Decode(&cfg); err != nil {
		return Config{}, err
	}
	if cfg.LiveDataFrequency <= 0 {
		cfg.LiveDataFrequency = defaultLiveDataFrequency
	}
	if cfg.HistoricalStartDay < 1 {
		cfg.HistoricalStartDay = 1260
	}
	return cfg, nil
}

func saveConfig(cfg Config) error {
	if cfg.LiveDataFrequency <= 0 {
		cfg.LiveDataFrequency = defaultLiveDataFrequency
	}
	if cfg.HistoricalStartDay < 1 {
		cfg.HistoricalStartDay = 1260
	}
	if cfg.LiquidHEX < 0 {
		cfg.LiquidHEX = 0
	}
	current, _ := loadConfig()
	if reflect.DeepEqual(current, cfg) {
		return nil
	}

	path := filepath.Join(dataDir, "config.json")
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0644)
	if err != nil {
		return err
	}
	defer file.Close()

	enc := json.NewEncoder(file)
	enc.SetIndent("", "  ")
	return enc.Encode(cfg)
}

func loadMinersFromDisk() ([]Miner, error) {
	path := filepath.Join(dataDir, "miners.json")
	file, err := os.Open(path)
	if err != nil {
		return []Miner{}, nil
	}
	defer file.Close()
	var miners []Miner
	json.NewDecoder(file).Decode(&miners)
	return miners, nil
}

// =============================================
// API HANDLERS
// =============================================
func writeJSON(w http.ResponseWriter, data interface{}) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(data)
}

func handleLiveData(w http.ResponseWriter, r *http.Request) {
	liveDataMutex.RLock()
	data := latestLiveData
	liveDataMutex.RUnlock()
	writeJSON(w, data)
}

func handleHEXJSON(w http.ResponseWriter, r *http.Request) {
	data, _ := loadLocalHEXJSON()
	writeJSON(w, data)
}

func handleMiners(w http.ResponseWriter, r *http.Request) {
	minersCacheMu.RLock()
	result := make([]Miner, len(cachedMiners))
	copy(result, cachedMiners)
	minersCacheMu.RUnlock()
	writeJSON(w, result)
}

func handleAddMiner(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}
	var miner Miner
	if err := json.NewDecoder(r.Body).Decode(&miner); err != nil {
		http.Error(w, "Invalid JSON", http.StatusBadRequest)
		return
	}
	start, err := time.Parse(dateLayout, miner.StartDate)
	if err != nil {
		http.Error(w, "Invalid start date format (DD-MM-YYYY)", http.StatusBadRequest)
		return
	}
	end, err := time.Parse(dateLayout, miner.EndDate)
	if err != nil {
		http.Error(w, "Invalid end date format (DD-MM-YYYY)", http.StatusBadRequest)
		return
	}
	if end.Before(start) {
		http.Error(w, "End date must be after start date", http.StatusBadRequest)
		return
	}
	if miner.TShares <= 0 {
		http.Error(w, "T-Shares must be positive", http.StatusBadRequest)
		return
	}

	minersCacheMu.Lock()
	cachedMiners = append(cachedMiners, miner)
	minersCacheMu.Unlock()

	minersCacheMu.RLock()
	toPersist := make([]Miner, len(cachedMiners))
	copy(toPersist, cachedMiners)
	minersCacheMu.RUnlock()

	_ = persistMiners(toPersist)
	w.WriteHeader(http.StatusCreated)
}

func handleEndMiner(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}
	var req struct{ Index int `json:"index"` }
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "Invalid request body", http.StatusBadRequest)
		return
	}
	minersCacheMu.Lock()
	if req.Index < 0 || req.Index >= len(cachedMiners) {
		minersCacheMu.Unlock()
		http.Error(w, "Invalid miner index", http.StatusBadRequest)
		return
	}
	cachedMiners[req.Index].Status = "completed"
	toPersist := make([]Miner, len(cachedMiners))
	copy(toPersist, cachedMiners)
	minersCacheMu.Unlock()

	_ = persistMiners(toPersist)
	w.WriteHeader(http.StatusOK)
}

func handleDeleteMiner(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}
	var req struct{ Index int `json:"index"` }
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "Invalid request body", http.StatusBadRequest)
		return
	}
	minersCacheMu.Lock()
	if req.Index < 0 || req.Index >= len(cachedMiners) {
		minersCacheMu.Unlock()
		http.Error(w, "Invalid miner index", http.StatusBadRequest)
		return
	}
	cachedMiners = append(cachedMiners[:req.Index], cachedMiners[req.Index+1:]...)
	toPersist := make([]Miner, len(cachedMiners))
	copy(toPersist, cachedMiners)
	minersCacheMu.Unlock()

	_ = persistMiners(toPersist)
	w.WriteHeader(http.StatusOK)
}

func handleConfig(w http.ResponseWriter, r *http.Request) {
	if r.Method == http.MethodGet {
		configManager.mu.RLock()
		cfg := configManager.config
		configManager.mu.RUnlock()
		writeJSON(w, cfg)
		return
	}
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	var cfg Config
	if err := json.NewDecoder(r.Body).Decode(&cfg); err != nil {
		http.Error(w, "Invalid JSON", http.StatusBadRequest)
		return
	}

	if err := saveConfig(cfg); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	configManager.SetLiveDataFrequency(cfg.LiveDataFrequency)
	configManager.SetHistoricalStartDay(cfg.HistoricalStartDay)
	configManager.SetLiquidHEX(cfg.LiquidHEX)

	w.WriteHeader(http.StatusOK)
}

// =============================================
// MAIN
// =============================================
func main() {
	if err := os.MkdirAll(dataDir, 0755); err != nil {
		log.Fatal("Failed to create data directory:", err)
	}
	initializeMiners()

	cfg, _ := loadConfig()
	configManager.SetLiveDataFrequency(cfg.LiveDataFrequency)
	configManager.SetHistoricalStartDay(cfg.HistoricalStartDay)
	configManager.SetLiquidHEX(cfg.LiquidHEX)

	if err := updateLocalHEXJSON(); err != nil {
		debugLog("Initial HEXJSON load failed:", err)
	}
	startDailyHEXJSONUpdate()
	startLiveDataBackgroundUpdater()

	subFS, _ := fs.Sub(staticFiles, "static")
	http.Handle("/", http.FileServer(http.FS(subFS)))

	http.HandleFunc("/api/live-data", handleLiveData)
	http.HandleFunc("/api/hexjson", handleHEXJSON)
	http.HandleFunc("/api/miners", handleMiners)
	http.HandleFunc("/api/add-miner", handleAddMiner)
	http.HandleFunc("/api/end-miner", handleEndMiner)
	http.HandleFunc("/api/delete-miner", handleDeleteMiner)
	http.HandleFunc("/api/config", handleConfig)

	log.Println("⬢ HEX Stats server starting on :5555 ⬢")
	log.Fatal(http.ListenAndServe(":5555", nil))
}
