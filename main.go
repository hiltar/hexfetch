package main

import (
    "embed"
    "bytes"
    "fmt"
    "log"
    "net/http"
    "os"
    "reflect"
    "strconv"
    "sync"
    "time"

    jsoniter "github.com/json-iterator/go"
    "github.com/cenkalti/backoff/v4"
    "github.com/shirou/gopsutil/v3/cpu"
    "github.com/shirou/gopsutil/v3/disk"
    "github.com/shirou/gopsutil/v3/mem"
)

//go:embed static
var staticFiles embed.FS

// Global variables for cached live data
var (
    latestLiveData LiveData
    liveDataMutex  sync.RWMutex
    hexJSONData    HEXJSON
    hexJSONMutex   sync.RWMutex
    systemInfo     SystemInfo
    systemInfoMutex sync.RWMutex
    bufferPool     = sync.Pool{
        New: func() interface{} { return new(bytes.Buffer) },
    }
    httpClient = &http.Client{
        Timeout: 10 * time.Second,
        Transport: &http.Transport{
            MaxIdleConns:       10,
            IdleConnTimeout:    30 * time.Second,
            DisableCompression: true,
        },
    }
    json = jsoniter.ConfigCompatibleWithStandardLibrary
)

// SystemInfo holds system metrics
type SystemInfo struct {
    CPUUsage    float64 `json:"cpuUsage"`    // Percentage
    MemoryUsage float64 `json:"memoryUsage"` // Percentage
    DiskUsage   float64 `json:"diskUsage"`   // Percentage
    Timestamp   int64   `json:"timestamp"`   // Unix timestamp in seconds
}

// ConfigManager for thread-safe configuration
type ConfigManager struct {
    mu          sync.RWMutex
    config      Config
    changeChans []chan struct{}
}

var configManager = &ConfigManager{
    config: Config{LiveDataFrequency: defaultLiveDataFrequency},
}

func (cm *ConfigManager) GetLiveDataFrequency() int {
    cm.mu.RLock()
    defer cm.mu.RUnlock()
    freq := cm.config.LiveDataFrequency
    if freq <= 0 {
        debugLog("Invalid LiveDataFrequency, using default:", defaultLiveDataFrequency)
        return defaultLiveDataFrequency
    }
    return freq
}

func (cm *ConfigManager) SetLiveDataFrequency(frequency int) {
    cm.mu.Lock()
    defer cm.mu.Unlock()
    if frequency <= 0 {
        debugLog("Attempted to set invalid LiveDataFrequency, ignoring:", frequency)
        return
    }
    cm.config.LiveDataFrequency = frequency
    debugLog("Set LiveDataFrequency to", frequency)
    for i, ch := range cm.changeChans {
        select {
        case ch <- struct{}{}:
        default:
            debugLog("Warning: Frequency change channel full for subscriber", i)
        }
    }
}

func (cm *ConfigManager) Subscribe() chan struct{} {
    cm.mu.Lock()
    defer cm.mu.Unlock()
    ch := make(chan struct{}, 1)
    cm.changeChans = append(cm.changeChans, ch)
    return ch
}

// Data Structures
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
    LiveDataFrequency int     `json:"liveDataFrequency"`
    LiquidHEX         float64 `json:"liquidHEX"`
}

const (
    dateLayout               = "02-01-2006"
    defaultLiveDataFrequency = 15
)

// Debug logging function
func debugLog(v ...interface{}) {
    if os.Getenv("DEBUG") == "true" {
        log.Println(v...)
    }
}

// Fetch system information
func fetchSystemInfo() (SystemInfo, error) {
    var info SystemInfo
    info.Timestamp = time.Now().Unix()

    // CPU Usage
    cpuPercent, err := cpu.Percent(time.Second, false)
    if err != nil {
        return SystemInfo{}, fmt.Errorf("failed to fetch CPU usage: %w", err)
    }
    if len(cpuPercent) > 0 {
        info.CPUUsage = cpuPercent[0]
    }

    // Memory Usage
    memInfo, err := mem.VirtualMemory()
    if err != nil {
        return SystemInfo{}, fmt.Errorf("failed to fetch memory usage: %w", err)
    }
    info.MemoryUsage = memInfo.UsedPercent

    // Disk Usage (root partition)
    diskInfo, err := disk.Usage("/")
    if err != nil {
        return SystemInfo{}, fmt.Errorf("failed to fetch disk usage: %w", err)
    }
    info.DiskUsage = diskInfo.UsedPercent

    return info, nil
}

// Data Fetching and Management
func fetchHEXJSON() (HEXJSON, error) {
    b := backoff.NewExponentialBackOff()
    b.MaxElapsedTime = 5 * time.Minute
    var data HEXJSON
    err := backoff.Retry(func() error {
        resp, err := httpClient.Get("https://hexdailystats.com/fulldatapulsechain")
        if err != nil {
            return err
        }
        defer resp.Body.Close()
        if resp.StatusCode != http.StatusOK {
            return fmt.Errorf("unexpected status code: %d", resp.StatusCode)
        }
        return json.NewDecoder(resp.Body).Decode(&data)
    }, b)
    if err != nil {
        return HEXJSON{}, fmt.Errorf("failed to fetch HEXJSON: %w", err)
    }
    return data, nil
}

func fetchLiveData() (LiveData, error) {
    b := backoff.NewExponentialBackOff()
    b.MaxElapsedTime = 5 * time.Minute
    var data LiveData
    err := backoff.Retry(func() error {
        resp, err := httpClient.Get("https://hexdailystats.com/livedata")
        if err != nil {
            return err
        }
        defer resp.Body.Close()
        if resp.StatusCode != http.StatusOK {
            return fmt.Errorf("unexpected status code: %d", resp.StatusCode)
        }
        return json.NewDecoder(resp.Body).Decode(&data)
    }, b)
    if err != nil {
        return LiveData{}, fmt.Errorf("failed to fetch live data: %w", err)
    }
    return data, nil
}

func loadLocalHEXJSON() (HEXJSON, error) {
    hexJSONMutex.RLock()
    defer hexJSONMutex.RUnlock()
    return hexJSONData, nil
}

func saveLocalHEXJSON(data HEXJSON) error {
    hexJSONMutex.Lock()
    defer hexJSONMutex.Unlock()
    hexJSONData = data
    return nil
}

func updateLocalHEXJSON() error {
    localData, err := loadLocalHEXJSON()
    if err != nil {
        return err
    }
    remoteData, err := fetchHEXJSON()
    if err != nil {
        return err
    }
    if len(localData) == 0 {
        return saveLocalHEXJSON(remoteData)
    }
    localMaxDay := localData[0].CurrentDay
    var newEntries []HEXJSONEntry
    for _, entry := range remoteData {
        if entry.CurrentDay > localMaxDay {
            newEntries = append(newEntries, entry)
        } else {
            break
        }
    }
    if len(newEntries) > 0 {
        updatedData := append(newEntries, localData...)
        return saveLocalHEXJSON(updatedData)
    }
    return nil
}

// Schedule daily HEXJSON updates
func startDailyHEXJSONUpdate() {
    go func() {
        for {
            // Calculate time until next midnight UTC
            now := time.Now().UTC()
            nextMidnight := now.Truncate(24 * time.Hour).Add(24 * time.Hour)
            delay := nextMidnight.Sub(now)

            debugLog("Scheduling next HEXJSON update in", delay, "(at", nextMidnight, "UTC)")
            time.Sleep(delay)

            debugLog("Running daily HEXJSON update...")
            if err := updateLocalHEXJSON(); err != nil {
                debugLog("Error during daily HEXJSON update:", err)
            } else {
                debugLog("Daily HEXJSON update completed successfully")
            }
        }
    }()
}

// Periodic system info updates
func startSystemInfoUpdate() {
    go func() {
        ticker := time.NewTicker(5 * time.Second) // Update every 5 seconds
        defer ticker.Stop()
        for {
            select {
            case <-ticker.C:
                debugLog("Fetching system info...")
                info, err := fetchSystemInfo()
                if err != nil {
                    debugLog("Error fetching system info:", err)
                } else {
                    systemInfoMutex.Lock()
                    systemInfo = info
                    systemInfoMutex.Unlock()
                    debugLog("System info updated successfully")
                }
            }
        }
    }()
}

func loadMiners() ([]Miner, error) {
    file, err := os.Open("/opt/hexfetch/miners.json")
    if err != nil {
        if os.IsNotExist(err) {
            return []Miner{}, nil
        }
        return nil, err
    }
    defer file.Close()
    var miners []Miner
    err = json.NewDecoder(file).Decode(&miners)
    return miners, err
}

func saveMiners(miners []Miner) error {
    currentMiners, err := loadMiners()
    if err == nil && reflect.DeepEqual(currentMiners, miners) {
        return nil // Skip write if unchanged
    }
    file, err := os.Create("/opt/hexfetch/miners.json")
    if err != nil {
        return err
    }
    defer file.Close()
    encoder := json.NewEncoder(file)
    encoder.SetIndent("", "  ")
    return encoder.Encode(miners)
}

func loadConfig() (Config, error) {
    file, err := os.Open("/opt/hexfetch/config.json")
    if err != nil {
        if os.IsNotExist(err) {
            return Config{LiveDataFrequency: defaultLiveDataFrequency, LiquidHEX: 0}, nil
        }
        return Config{}, err
    }
    defer file.Close()
    var config Config
    err = json.NewDecoder(file).Decode(&config)
    if err != nil {
        return Config{}, err
    }
    if config.LiveDataFrequency <= 0 {
        config.LiveDataFrequency = defaultLiveDataFrequency
    }
    return config, err
}

func saveConfig(config Config) error {
    currentConfig, err := loadConfig()
    if err == nil && reflect.DeepEqual(currentConfig, config) {
        return nil // Skip write if unchanged
    }
    file, err := os.Create("/opt/hexfetch/config.json")
    if err != nil {
        return err
    }
    defer file.Close()
    encoder := json.NewEncoder(file)
    encoder.SetIndent("", "  ")
    return encoder.Encode(config)
}

// Utility Functions
func isMatured(endDate string) (bool, error) {
    endTime, err := time.Parse(dateLayout, endDate)
    if err != nil {
        return false, err
    }
    now := time.Now()
    endDateOnly := time.Date(endTime.Year(), endTime.Month(), endTime.Day(), 0, 0, 0, 0, endTime.Location())
    nowDateOnly := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location())
    return nowDateOnly.After(endDateOnly) || nowDateOnly.Equal(endDateOnly), nil
}

func daysLeft(endDate string) (int, error) {
    endTime, err := time.Parse(dateLayout, endDate)
    if err != nil {
        return 0, err
    }
    now := time.Now()
    endDateOnly := time.Date(endTime.Year(), endTime.Month(), endTime.Day(), 0, 0, 0, 0, endTime.Location())
    nowDateOnly := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location())
    if nowDateOnly.After(endDateOnly) {
        return 0, nil
    }
    duration := endDateOnly.Sub(nowDateOnly)
    return int(duration.Hours() / 24), nil
}

func formatWithCommas(num int) string {
    str := strconv.FormatInt(int64(num), 10)
    n := len(str)
    if n <= 3 {
        return str
    }
    buf := make([]byte, 0, n+(n-1)/3)
    for i := 0; i < n; i++ {
        if i > 0 && (n-i)%3 == 0 {
            buf = append(buf, ',')
        }
        buf = append(buf, str[i])
    }
    return string(buf)
}

func formatLongWithCommas(num int64) string {
    return formatWithCommas(int(num))
}

// API Handlers
func handleSystemInfo(w http.ResponseWriter, r *http.Request) {
    systemInfoMutex.RLock()
    data := systemInfo
    systemInfoMutex.RUnlock()
    buf := bufferPool.Get().(*bytes.Buffer)
    defer bufferPool.Put(buf)
    buf.Reset()
    if err := json.NewEncoder(buf).Encode(data); err != nil {
        debugLog("Error encoding system info response:", err)
        http.Error(w, "Internal server error", http.StatusInternalServerError)
        return
    }
    w.Write(buf.Bytes())
}

func handleLiveData(w http.ResponseWriter, r *http.Request) {
    liveDataMutex.RLock()
    data := latestLiveData
    liveDataMutex.RUnlock()
    buf := bufferPool.Get().(*bytes.Buffer)
    defer bufferPool.Put(buf)
    buf.Reset()
    if err := json.NewEncoder(buf).Encode(data); err != nil {
        debugLog("Error encoding live data response:", err)
        http.Error(w, "Internal server error", http.StatusInternalServerError)
        return
    }
    w.Write(buf.Bytes())
}

func handleHEXJSON(w http.ResponseWriter, r *http.Request) {
    data, err := loadLocalHEXJSON()
    if err != nil {
        debugLog("Error loading HEXJSON:", err)
        http.Error(w, err.Error(), http.StatusInternalServerError)
        return
    }
    buf := bufferPool.Get().(*bytes.Buffer)
    defer bufferPool.Put(buf)
    buf.Reset()
    if err := json.NewEncoder(buf).Encode(data); err != nil {
        debugLog("Error encoding HEXJSON response:", err)
        http.Error(w, "Internal server error", http.StatusInternalServerError)
        return
    }
    w.Write(buf.Bytes())
}

func handleMiners(w http.ResponseWriter, r *http.Request) {
    miners, err := loadMiners()
    if err != nil {
        debugLog("Error loading miners:", err)
        http.Error(w, err.Error(), http.StatusInternalServerError)
        return
    }
    buf := bufferPool.Get().(*bytes.Buffer)
    defer bufferPool.Put(buf)
    buf.Reset()
    if err := json.NewEncoder(buf).Encode(miners); err != nil {
        debugLog("Error encoding miners response:", err)
        http.Error(w, "Internal server error", http.StatusInternalServerError)
        return
    }
    w.Write(buf.Bytes())
}

func handleAddMiner(w http.ResponseWriter, r *http.Request) {
    if r.Method != http.MethodPost {
        http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
        return
    }
    var miner Miner
    if err := json.NewDecoder(r.Body).Decode(&miner); err != nil {
        debugLog("Error decoding add miner request:", err)
        http.Error(w, "Invalid request body", http.StatusBadRequest)
        return
    }
    if miner.StartDate == "" || miner.EndDate == "" || miner.TShares <= 0 {
        http.Error(w, "Invalid miner data", http.StatusBadRequest)
        return
    }
    if _, err := time.Parse(dateLayout, miner.StartDate); err != nil {
        debugLog("Invalid start date format:", err)
        http.Error(w, "Invalid start date format", http.StatusBadRequest)
        return
    }
    if _, err := time.Parse(dateLayout, miner.EndDate); err != nil {
        debugLog("Invalid end date format:", err)
        http.Error(w, "Invalid end date format", http.StatusBadRequest)
        return
    }
    miners, err := loadMiners()
    if err != nil {
        debugLog("Error loading miners for add:", err)
        http.Error(w, err.Error(), http.StatusInternalServerError)
        return
    }
    miners = append(miners, miner)
    if err := saveMiners(miners); err != nil {
        debugLog("Error saving miners:", err)
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
    var req struct {
        Index int `json:"index"`
    }
    if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
        debugLog("Error decoding end miner request:", err)
        http.Error(w, "Invalid request body", http.StatusBadRequest)
        return
    }
    miners, err := loadMiners()
    if err != nil || req.Index < 0 || req.Index >= len(miners) {
        debugLog("Invalid miner index or error loading miners:", err)
        http.Error(w, "Invalid miner index", http.StatusBadRequest)
        return
    }
    miners[req.Index].Status = "completed"
    if err := saveMiners(miners); err != nil {
        debugLog("Error saving miners for end:", err)
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
    var req struct {
        Index int `json:"index"`
    }
    if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
        debugLog("Error decoding delete miner request:", err)
        http.Error(w, "Invalid request body", http.StatusBadRequest)
        return
    }
    miners, err := loadMiners()
    if err != nil || req.Index < 0 || req.Index >= len(miners) {
        debugLog("Invalid miner index or error loading miners:", err)
        http.Error(w, "Invalid miner index", http.StatusBadRequest)
        return
    }
    miners = append(miners[:req.Index], miners[req.Index+1:]...)
    if err := saveMiners(miners); err != nil {
        debugLog("Error saving miners for delete:", err)
        http.Error(w, err.Error(), http.StatusInternalServerError)
        return
    }
    w.WriteHeader(http.StatusOK)
}

func handleConfig(w http.ResponseWriter, r *http.Request) {
    if r.Method == http.MethodGet {
        config, err := loadConfig()
        if err != nil {
            debugLog("Error loading config:", err)
            http.Error(w, err.Error(), http.StatusInternalServerError)
            return
        }
        buf := bufferPool.Get().(*bytes.Buffer)
        defer bufferPool.Put(buf)
        buf.Reset()
        if err := json.NewEncoder(buf).Encode(config); err != nil {
            debugLog("Error encoding config response:", err)
            http.Error(w, "Internal server error", http.StatusInternalServerError)
            return
        }
        w.Write(buf.Bytes())
    } else if r.Method == http.MethodPost {
        var config Config
        if err := json.NewDecoder(r.Body).Decode(&config); err != nil {
            debugLog("Error decoding config request:", err)
            http.Error(w, "Invalid request body", http.StatusBadRequest)
            return
        }
        if config.LiveDataFrequency <= 0 {
            debugLog("Invalid frequency in config request:", config.LiveDataFrequency)
            http.Error(w, "Frequency must be positive", http.StatusBadRequest)
            return
        }
        if config.LiquidHEX < 0 {
            debugLog("Invalid LiquidHEX in config request:", config.LiquidHEX)
            http.Error(w, "Liquid HEX must be non-negative", http.StatusBadRequest)
            return
        }
        if err := saveConfig(config); err != nil {
            debugLog("Error saving config:", err)
            http.Error(w, err.Error(), http.StatusInternalServerError)
            return
        }
        configManager.SetLiveDataFrequency(config.LiveDataFrequency)
        w.WriteHeader(http.StatusOK)
    } else {
        http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
    }
}

// Main Function
func main() {
    os.MkdirAll("/opt/hexfetch", 0755)

    // Initial HEXJSON update
    if err := updateLocalHEXJSON(); err != nil {
        debugLog("Error updating local HEXJSON:", err)
    }

    // Start daily HEXJSON updates
    startDailyHEXJSONUpdate()

    // Load configuration
    config, err := loadConfig()
    if err != nil {
        debugLog("Error loading config:", err)
        config.LiveDataFrequency = defaultLiveDataFrequency
    }
    configManager.SetLiveDataFrequency(config.LiveDataFrequency)

    // Initial live data fetch
    data, err := fetchLiveData()
    if err != nil {
        debugLog("Error during initial live data fetch:", err)
    } else {
        liveDataMutex.Lock()
        latestLiveData = data
        liveDataMutex.Unlock()
        debugLog("Initial live data fetched successfully")
    }

    // Initial system info fetch
    info, err := fetchSystemInfo()
    if err != nil {
        debugLog("Error during initial system info fetch:", err)
    } else {
        systemInfoMutex.Lock()
        systemInfo = info
        systemInfoMutex.Unlock()
        debugLog("Initial system info fetched successfully")
    }

    // Periodic live data fetching
    go func() {
        ticker := time.NewTicker(time.Duration(configManager.GetLiveDataFrequency()) * time.Minute)
        changeCh := configManager.Subscribe()
        defer ticker.Stop()
        for {
            select {
            case <-ticker.C:
                debugLog("Fetching live data from external API...")
                data, err := fetchLiveData()
                if err != nil {
                    debugLog("Error fetching live data:", err)
                } else {
                    liveDataMutex.Lock()
                    latestLiveData = data
                    liveDataMutex.Unlock()
                    debugLog("Live data updated successfully")
                }
            case <-changeCh:
                frequency := configManager.GetLiveDataFrequency()
                if frequency <= 0 {
                    frequency = defaultLiveDataFrequency
                }
                ticker.Reset(time.Duration(frequency) * time.Minute)
            }
        }
    }()

    // Start periodic system info updates
    startSystemInfoUpdate()

    // Serve embedded static files
    http.Handle("/", http.FileServer(http.FS(staticFiles)))

    // API endpoints
    http.HandleFunc("/api/system-info", handleSystemInfo)
    http.HandleFunc("/api/live-data", handleLiveData)
    http.HandleFunc("/api/hexjson", handleHEXJSON)
    http.HandleFunc("/api/miners", handleMiners)
    http.HandleFunc("/api/add-miner", handleAddMiner)
    http.HandleFunc("/api/end-miner", handleEndMiner)
    http.HandleFunc("/api/delete-miner", handleDeleteMiner)
    http.HandleFunc("/api/config", handleConfig)

    log.Println("Server starting on :5555")
    if err := http.ListenAndServe(":5555", nil); err != nil {
        log.Fatal("Server failed:", err)
    }
}
