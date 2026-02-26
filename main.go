package main

import (
    "embed"
    "encoding/json"
    "fmt"
    "io/fs"
    "log"
    "net/http"
    "os"
    "sync"
    "time"

    "github.com/shirou/gopsutil/v3/cpu"
    "github.com/shirou/gopsutil/v3/disk"
    "github.com/shirou/gopsutil/v3/mem"
)

//go:embed static
var staticFiles embed.FS

var (
    latestLiveData LiveData
    liveDataMutex  sync.Mutex
)

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
        return defaultLiveDataFrequency
    }
    return freq
}

func (cm *ConfigManager) SetLiveDataFrequency(frequency int) {
    cm.mu.Lock()
    defer cm.mu.Unlock()
    if frequency <= 0 {
        return
    }
    cm.config.LiveDataFrequency = frequency
    for _, ch := range cm.changeChans {
        select {
        case ch <- struct{}{}:
        default:
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

// ==================== DATA STRUCTURES ====================

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
    ChartStartDay     int     `json:"chartStartDay"`
}

type SystemMetrics struct {
    CPUUsagePercent    float64 `json:"cpuUsagePercent"`
    MemoryUsedPercent  float64 `json:"memoryUsedPercent"`
    MemoryUsedGB       float64 `json:"memoryUsedGB"`
    MemoryTotalGB      float64 `json:"memoryTotalGB"`
    DiskUsedPercent    float64 `json:"diskUsedPercent"`
    DiskUsedGB         float64 `json:"diskUsedGB"`
    DiskTotalGB        float64 `json:"diskTotalGB"`
    Timestamp          int64   `json:"timestamp"`
}

const (
    dateLayout               = "02-01-2006"
    defaultLiveDataFrequency = 15
)

// ==================== UTILITY FUNCTIONS ====================

func debugLog(v ...interface{}) {
    if os.Getenv("DEBUG") == "true" {
        log.Println(v...)
    }
}

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

// ==================== DATA FETCHING ====================

func fetchHEXJSON() (HEXJSON, error) {
    resp, err := http.Get("https://hexdailystats.com/fulldatapulsechain")
    if err != nil {
        return nil, err
    }
    defer resp.Body.Close()
    var data HEXJSON
    if err := json.NewDecoder(resp.Body).Decode(&data); err != nil {
        return nil, err
    }
    return data, nil
}

func fetchLiveData() (LiveData, error) {
    resp, err := http.Get("https://hexdailystats.com/livedata")
    if err != nil {
        return LiveData{}, err
    }
    defer resp.Body.Close()
    var data LiveData
    if err := json.NewDecoder(resp.Body).Decode(&data); err != nil {
        return LiveData{}, err
    }
    return data, nil
}

func loadLocalHEXJSON() (HEXJSON, error) {
    file, err := os.Open("data/hexjson.json")
    if err != nil {
        if os.IsNotExist(err) {
            return HEXJSON{}, nil
        }
        return nil, err
    }
    defer file.Close()
    var data HEXJSON
    json.NewDecoder(file).Decode(&data)
    return data, nil
}

func saveLocalHEXJSON(data HEXJSON) error {
    os.MkdirAll("data", 0755)
    file, err := os.Create("data/hexjson.json")
    if err != nil {
        return err
    }
    defer file.Close()
    encoder := json.NewEncoder(file)
    encoder.SetIndent("", "  ")
    return encoder.Encode(data)
}

func updateLocalHEXJSON() error {
    local, _ := loadLocalHEXJSON()
    remote, err := fetchHEXJSON()
    if err != nil {
        return err
    }
    if len(local) == 0 {
        return saveLocalHEXJSON(remote)
    }
    updated := append(remote, local...)
    seen := make(map[int]bool)
    var unique HEXJSON
    for i := len(updated) - 1; i >= 0; i-- {
        if !seen[updated[i].CurrentDay] {
            seen[updated[i].CurrentDay] = true
            unique = append([]HEXJSONEntry{updated[i]}, unique...)
        }
    }
    return saveLocalHEXJSON(unique)
}

func startDailyHEXJSONUpdate() {
    go func() {
        for {
            now := time.Now().UTC()
            next := now.Truncate(24*time.Hour).Add(24 * time.Hour)
            time.Sleep(next.Sub(now))
            if err := updateLocalHEXJSON(); err != nil {
                debugLog("Daily update error:", err)
            }
        }
    }()
}

// ==================== CONFIG & MINERS ====================

func loadConfig() (Config, error) {
    os.MkdirAll("settings", 0755)
    file, err := os.Open("settings/config.json")
    if err != nil {
        if os.IsNotExist(err) {
            return Config{LiveDataFrequency: defaultLiveDataFrequency, LiquidHEX: 0, ChartStartDay: 0}, nil
        }
        return Config{}, err
    }
    defer file.Close()
    var config Config
    json.NewDecoder(file).Decode(&config)
    if config.LiveDataFrequency <= 0 {
        config.LiveDataFrequency = defaultLiveDataFrequency
    }
    if config.ChartStartDay < 0 {
        config.ChartStartDay = 0
    }
    return config, nil
}

func saveConfig(config Config) error {
    file, err := os.Create("settings/config.json")
    if err != nil {
        return err
    }
    defer file.Close()
    encoder := json.NewEncoder(file)
    encoder.SetIndent("", "  ")
    return encoder.Encode(config)
}

func loadMiners() ([]Miner, error) {
    file, err := os.Open("settings/miners.json")
    if err != nil {
        if os.IsNotExist(err) {
            return []Miner{}, nil
        }
        return nil, err
    }
    defer file.Close()
    var miners []Miner
    json.NewDecoder(file).Decode(&miners)
    return miners, nil
}

func saveMiners(miners []Miner) error {
    file, err := os.Create("settings/miners.json")
    if err != nil {
        return err
    }
    defer file.Close()
    encoder := json.NewEncoder(file)
    encoder.SetIndent("", "  ")
    return encoder.Encode(miners)
}

// ==================== HANDLERS ====================

func handleLiveData(w http.ResponseWriter, r *http.Request) {
    liveDataMutex.Lock()
    data := latestLiveData
    liveDataMutex.Unlock()
    json.NewEncoder(w).Encode(data)
}

func handleHEXJSON(w http.ResponseWriter, r *http.Request) {
    data, err := loadLocalHEXJSON()
    if err != nil {
        http.Error(w, err.Error(), http.StatusInternalServerError)
        return
    }
    json.NewEncoder(w).Encode(data)
}

func handleMiners(w http.ResponseWriter, r *http.Request) {
    miners, err := loadMiners()
    if err != nil {
        http.Error(w, err.Error(), http.StatusInternalServerError)
        return
    }
    json.NewEncoder(w).Encode(miners)
}

func handleAddMiner(w http.ResponseWriter, r *http.Request) {
    if r.Method != http.MethodPost {
        http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
        return
    }
    var miner Miner
    if err := json.NewDecoder(r.Body).Decode(&miner); err != nil {
        http.Error(w, "Invalid request body", http.StatusBadRequest)
        return
    }
    miners, _ := loadMiners()
    miners = append(miners, miner)
    saveMiners(miners)
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
    matured, err := isMatured(miners[req.Index].EndDate)
    if err != nil {
        debugLog("Error checking miner maturity:", err)
        http.Error(w, "Invalid end date format", http.StatusBadRequest)
        return
    }
    if !matured {
        debugLog("Attempted to end non-matured miner at index:", req.Index)
        http.Error(w, "Miner is not yet matured", http.StatusBadRequest)
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
        config, _ := loadConfig()
        json.NewEncoder(w).Encode(config)
    } else if r.Method == http.MethodPost {
        var config Config
        json.NewDecoder(r.Body).Decode(&config)
        if config.LiveDataFrequency <= 0 {
            config.LiveDataFrequency = defaultLiveDataFrequency
        }
        if config.LiquidHEX < 0 {
            http.Error(w, "Liquid HEX must be non-negative", http.StatusBadRequest)
            return
        }
        if config.ChartStartDay < 0 {
            config.ChartStartDay = 0
        }
        saveConfig(config)
        configManager.SetLiveDataFrequency(config.LiveDataFrequency)
        w.WriteHeader(http.StatusOK)
    }
}

func handleSystemMetrics(w http.ResponseWriter, r *http.Request) {
    if os.Getenv("SYSTEM") != "true" {
        debugLog("System metrics endpoint disabled (SYSTEM != true)")
        http.Error(w, "System metrics endpoint disabled", http.StatusNotFound)
        return
    }

    cpuPercent, err := cpu.Percent(0, false)
    cpuUsage := 0.0
    if err == nil && len(cpuPercent) > 0 {
        cpuUsage = cpuPercent[0]
    }

    memInfo, err := mem.VirtualMemory()
    memoryUsedPercent := 0.0
    memoryUsedGB := 0.0
    memoryTotalGB := 0.0
    if err == nil {
        memoryUsedPercent = memInfo.UsedPercent
        memoryUsedGB = float64(memInfo.Used) / 1e9
        memoryTotalGB = float64(memInfo.Total) / 1e9
    }

    diskInfo, err := disk.Usage("/")
    diskUsedPercent := 0.0
    diskUsedGB := 0.0
    diskTotalGB := 0.0
    if err == nil {
        diskUsedPercent = diskInfo.UsedPercent
        diskUsedGB = float64(diskInfo.Used) / 1e9
        diskTotalGB = float64(diskInfo.Total) / 1e9
    }

    metrics := SystemMetrics{
        CPUUsagePercent:   cpuUsage,
        MemoryUsedPercent: memoryUsedPercent,
        MemoryUsedGB:      memoryUsedGB,
        MemoryTotalGB:     memoryTotalGB,
        DiskUsedPercent:   diskUsedPercent,
        DiskUsedGB:        diskUsedGB,
        DiskTotalGB:       diskTotalGB,
        Timestamp:         time.Now().Unix(),
    }

    if err := json.NewEncoder(w).Encode(metrics); err != nil {
        debugLog("Error encoding system metrics response:", err)
        http.Error(w, "Internal server error", http.StatusInternalServerError)
        return
    }
}

func main() {
    os.MkdirAll("data", 0755)
    os.MkdirAll("settings", 0755)

    updateLocalHEXJSON()
    startDailyHEXJSONUpdate()

    config, _ := loadConfig()
    configManager.SetLiveDataFrequency(config.LiveDataFrequency)

    if data, err := fetchLiveData(); err == nil {
        liveDataMutex.Lock()
        latestLiveData = data
        liveDataMutex.Unlock()
    }

    go func() {
        ticker := time.NewTicker(time.Duration(configManager.GetLiveDataFrequency()) * time.Minute)
        changeCh := configManager.Subscribe()
        defer ticker.Stop()
        for {
            select {
            case <-ticker.C:
                if data, err := fetchLiveData(); err == nil {
                    liveDataMutex.Lock()
                    latestLiveData = data
                    liveDataMutex.Unlock()
                }
                ticker.Reset(time.Duration(configManager.GetLiveDataFrequency()) * time.Minute)
            case <-changeCh:
                ticker.Reset(time.Duration(configManager.GetLiveDataFrequency()) * time.Minute)
            }
        }
    }()

    fs, _ := fs.Sub(staticFiles, "static")
    http.Handle("/", http.FileServer(http.FS(fs)))

    http.HandleFunc("/api/live-data", handleLiveData)
    http.HandleFunc("/api/hexjson", handleHEXJSON)
    http.HandleFunc("/api/miners", handleMiners)
    http.HandleFunc("/api/add-miner", handleAddMiner)
    http.HandleFunc("/api/end-miner", handleEndMiner)
    http.HandleFunc("/api/delete-miner", handleDeleteMiner)
    http.HandleFunc("/api/config", handleConfig)
    http.HandleFunc("/api/system", handleSystemMetrics)

    log.Println("HEX Stats server started on :5555")
    log.Fatal(http.ListenAndServe(":5555", nil))
}
