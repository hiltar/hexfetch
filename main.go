package main

import (
	"context"
	"encoding/json"
	"embed"
	"fmt"
	"io/fs"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"sync"
	"time"

	"github.com/cenkalti/backoff/v4"
	"github.com/gorilla/websocket"
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
)

// Global cached data
var (
	latestLiveData LiveData
	liveDataMutex  sync.RWMutex
	hexJSONData    HEXJSON
	hexJSONMutex   sync.RWMutex

	httpClient = &http.Client{
		Timeout: 10 * time.Second,
		Transport: &http.Transport{
			MaxIdleConns:       10,
			IdleConnTimeout:    30 * time.Second,
			DisableCompression: true,
		},
	}

	// WebSocket broadcaster
	wsClients   = make(map[*websocket.Conn]bool)
	wsClientsMu sync.Mutex
	wsUpgrader  = websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}

	// Live data fetcher state
	fetcherCtx    context.Context
	fetcherCancel context.CancelFunc
	fetcherDone   chan struct{} // closed when goroutine exits
	fetcherMu     sync.Mutex
	fetcherActive bool

	// Config notification
	configSubsMu sync.Mutex
	configSubs   []chan struct{}
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
	Beat                      int64   `json:"beat"`
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

// Subscribe returns a channel that receives a notification when config changes.
func (cm *ConfigManager) Subscribe() chan struct{} {
	ch := make(chan struct{}, 1)
	configSubsMu.Lock()
	configSubs = append(configSubs, ch)
	configSubsMu.Unlock()
	return ch
}

// Unsubscribe removes the channel from the subscriber list.
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

func fetchLiveData() (LiveData, error) {
	var data LiveData
	b := backoff.NewExponentialBackOff()
	b.MaxElapsedTime = 5 * time.Minute

	err := backoff.Retry(func() error {
		resp, err := httpClient.Get("https://hexdailystats.com/livedata")
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
	// keep data sorted by day
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

func loadMiners() ([]Miner, error) {
	path := filepath.Join(dataDir, "miners.json")
	file, err := os.Open(path)
	if err != nil {
		if os.IsNotExist(err) {
			return []Miner{}, nil
		}
		return nil, err
	}
	defer file.Close()
	var miners []Miner
	return miners, json.NewDecoder(file).Decode(&miners)
}

func saveMiners(miners []Miner) error {
	current, _ := loadMiners()
	if reflect.DeepEqual(current, miners) {
		return nil
	}
	path := filepath.Join(dataDir, "miners.json")
	file, err := os.Create(path)
	if err != nil {
		return err
	}
	defer file.Close()
	enc := json.NewEncoder(file)
	enc.SetIndent("", "  ")
	return enc.Encode(miners)
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
	file, err := os.Create(path)
	if err != nil {
		return err
	}
	defer file.Close()
	enc := json.NewEncoder(file)
	enc.SetIndent("", "  ")
	return enc.Encode(cfg)
}

// =============================================
// WEB SOCKET MANAGEMENT
// =============================================

func broadcastLiveData(data LiveData) {
	wsClientsMu.Lock()
	conns := make([]*websocket.Conn, 0, len(wsClients))
	for conn := range wsClients {
		conns = append(conns, conn)
	}
	wsClientsMu.Unlock()

	for _, conn := range conns {
		if err := conn.WriteJSON(data); err != nil {
			conn.Close()
			wsClientsMu.Lock()
			delete(wsClients, conn)
			wsClientsMu.Unlock()
		}
	}
}

// startLiveDataFetcher runs a goroutine that fetches live data at the configured interval.
func startLiveDataFetcher() {
	fetcherMu.Lock()
	defer fetcherMu.Unlock()

	if fetcherActive {
		return
	}

	ctx, cancel := context.WithCancel(context.Background())
	fetcherCtx = ctx
	fetcherCancel = cancel
	fetcherActive = true
	fetcherDone = make(chan struct{})

	configCh := configManager.Subscribe()

	go func() {
		defer func() {
			fetcherMu.Lock()
			fetcherActive = false
			if fetcherDone != nil {
				close(fetcherDone)
				fetcherDone = nil
			}
			fetcherMu.Unlock()
			configManager.Unsubscribe(configCh)
		}()

		freq := configManager.GetLiveDataFrequency()
		timer := time.NewTimer(0)
		defer timer.Stop()

		for {
			select {
			case <-ctx.Done():
				return
			case <-timer.C:
				data, err := fetchLiveData()
				if err != nil {
					debugLog("fetchLiveData error:", err)
				} else {
					liveDataMutex.Lock()
					latestLiveData = data
					liveDataMutex.Unlock()
					broadcastLiveData(data)
				}
				freq = configManager.GetLiveDataFrequency()
				timer.Reset(time.Duration(freq) * time.Minute)
			case <-configCh:
				newFreq := configManager.GetLiveDataFrequency()
				if newFreq != freq {
					freq = newFreq
					if !timer.Stop() {
						select {
						case <-timer.C:
						default:
						}
					}
					timer.Reset(time.Duration(freq) * time.Minute)
				}
			}
		}
	}()
}

func stopLiveDataFetcher() {
	fetcherMu.Lock()
	defer fetcherMu.Unlock()
	if fetcherActive {
		fetcherCancel()
		<-fetcherDone // wait for goroutine to finish
		fetcherActive = false
	}
}

func handleLiveWebSocket(w http.ResponseWriter, r *http.Request) {
	conn, err := wsUpgrader.Upgrade(w, r, nil)
	if err != nil {
		log.Println("WebSocket upgrade error:", err)
		return
	}

	wsClientsMu.Lock()
	wasEmpty := len(wsClients) == 0
	wsClients[conn] = true
	wsClientsMu.Unlock()

	if wasEmpty {
		startLiveDataFetcher()
	}

	// send current live data immediately
	liveDataMutex.RLock()
	currentData := latestLiveData
	liveDataMutex.RUnlock()
	if err := conn.WriteJSON(currentData); err != nil {
		conn.Close()
		wsClientsMu.Lock()
		delete(wsClients, conn)
		wsClientsMu.Unlock()
		return
	}

	for {
		_, _, err := conn.ReadMessage()
		if err != nil {
			wsClientsMu.Lock()
			delete(wsClients, conn)
			nowEmpty := len(wsClients) == 0
			wsClientsMu.Unlock()

			if nowEmpty {
				stopLiveDataFetcher()
			}
			conn.Close()
			return
		}
	}
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
	miners, _ := loadMiners()
	writeJSON(w, miners)
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

	miners, _ := loadMiners()
	miners = append(miners, miner)
	if err := saveMiners(miners); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
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
	miners, err := loadMiners()
	if err != nil || req.Index < 0 || req.Index >= len(miners) {
		http.Error(w, "Invalid miner index", http.StatusBadRequest)
		return
	}
	miners[req.Index].Status = "completed"
	if err := saveMiners(miners); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
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
	miners, err := loadMiners()
	if err != nil || req.Index < 0 || req.Index >= len(miners) {
		http.Error(w, "Invalid miner index", http.StatusBadRequest)
		return
	}
	miners = append(miners[:req.Index], miners[req.Index+1:]...)
	if err := saveMiners(miners); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusOK)
}

func handleConfig(w http.ResponseWriter, r *http.Request) {
	if r.Method == http.MethodGet {
		cfg, _ := loadConfig()
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
	w.WriteHeader(http.StatusOK)
}

// =============================================
// MAIN
// =============================================

func main() {
	if err := os.MkdirAll(dataDir, 0755); err != nil {
		log.Fatal("Failed to create data directory:", err)
	}

	if err := updateLocalHEXJSON(); err != nil {
		debugLog("Initial HEXJSON load failed:", err)
	}
	startDailyHEXJSONUpdate()

	cfg, _ := loadConfig()
	configManager.SetLiveDataFrequency(cfg.LiveDataFrequency)
	configManager.SetHistoricalStartDay(cfg.HistoricalStartDay)

	subFS, _ := fs.Sub(staticFiles, "static")
	http.Handle("/", http.FileServer(http.FS(subFS)))

	http.HandleFunc("/api/live-data", handleLiveData)
	http.HandleFunc("/api/hexjson", handleHEXJSON)
	http.HandleFunc("/api/miners", handleMiners)
	http.HandleFunc("/api/add-miner", handleAddMiner)
	http.HandleFunc("/api/end-miner", handleEndMiner)
	http.HandleFunc("/api/delete-miner", handleDeleteMiner)
	http.HandleFunc("/api/config", handleConfig)
	http.HandleFunc("/ws/live-data", handleLiveWebSocket)

	log.Println("⬢ HEX Stats server starting on :5555 ⬢")
	log.Fatal(http.ListenAndServe(":5555", nil))
}
