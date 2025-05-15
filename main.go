package main

import (
    "encoding/json"
    "fmt"
    "log"
    "net/http"
    "os"
    "strconv"
    "sync"
    "time"
    "github.com/shirou/gopsutil/v3/cpu"
    "github.com/shirou/gopsutil/v3/disk"
    "github.com/shirou/gopsutil/v3/mem"
    "github.com/shirou/gopsutil/v3/net"
)

// Global variables for cached live data
var (
    latestLiveData LiveData
    liveDataMutex  sync.Mutex
)

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

type SystemMetrics struct {
    CPUUsagePercent    float64 `json:"cpuUsagePercent"`
    MemoryUsedPercent  float64 `json:"memoryUsedPercent"`
    MemoryUsedGB       float64 `json:"memoryUsedGB"`
    MemoryTotalGB      float64 `json:"memoryTotalGB"`
    DiskUsedPercent    float64 `json:"diskUsedPercent"`
    DiskUsedGB         float64 `json:"diskUsedGB"`
    DiskTotalGB        float64 `json:"diskTotalGB"`
    NetworkSentMB      float64 `json:"networkSentMB"`
    NetworkReceivedMB  float64 `json:"networkReceivedMB"`
    Timestamp          int64   `json:"timestamp"`
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

// Data Fetching and Management
func fetchHEXJSON() (HEXJSON, error) {
    resp, err := http.Get("https://hexdailystats.com/fulldatapulsechain")
    if err != nil {
        return HEXJSON{}, fmt.Errorf("failed to fetch HEXJSON: %w", err)
    }
    defer resp.Body.Close()
    if resp.StatusCode != http.StatusOK {
        return HEXJSON{}, fmt.Errorf("unexpected status code: %d", resp.StatusCode)
    }
    var data HEXJSON
    err = json.NewDecoder(resp.Body).Decode(&data)
    if err != nil {
        return HEXJSON{}, fmt.Errorf("failed to decode HEXJSON: %w", err)
    }
    return data, nil
}

func fetchLiveData() (LiveData, error) {
    resp, err := http.Get("https://hexdailystats.com/livedata")
    if err != nil {
        return LiveData{}, fmt.Errorf("failed to fetch live data: %w", err)
    }
    defer resp.Body.Close()
    if resp.StatusCode != http.StatusOK {
        return LiveData{}, fmt.Errorf("unexpected status code: %d", resp.StatusCode)
    }
    var data LiveData
    err = json.NewDecoder(resp.Body).Decode(&data)
    if err != nil {
        return LiveData{}, fmt.Errorf("failed to decode live data: %w", err)
    }
    return data, nil
}

func loadLocalHEXJSON() (HEXJSON, error) {
    file, err := os.Open("data/hexjson.json")
    if err != nil {
        if os.IsNotExist(err) {
            return HEXJSON{}, nil
        }
        return HEXJSON{}, err
    }
    defer file.Close()
    var data HEXJSON
    err = json.NewDecoder(file).Decode(&data)
    return data, err
}

func saveLocalHEXJSON(data HEXJSON) error {
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
    err = json.NewDecoder(file).Decode(&miners)
    return miners, err
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

func loadConfig() (Config, error) {
    file, err := os.Open("settings/config.json")
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
    file, err := os.Create("settings/config.json")
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
    return int(duration.Hours()/24), nil
}

func formatWithCommas(num int) string {
    str := strconv.Itoa(num)
    n := len(str)
    if n <= 3 {
        return str
    }
    var result []byte
    for i := 0; i < n; i++ {
        if i > 0 && (n-i)%3 == 0 {
            result = append(result, ',')
        }
        result = append(result, str[i])
    }
    return string(result)
}

func formatLongWithCommas(num int64) string {
    return formatWithCommas(int(num))
}

// System Metrics Handler
func handleSystemMetrics(w http.ResponseWriter, r *http.Request) {
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

    netInfo, err := net.IOCounters(false)
    networkSentMB := 0.0
    networkReceivedMB := 0.0
    if err == nil && len(netInfo) > 0 {
        networkSentMB = float64(netInfo[0].BytesSent) / 1e6
        networkReceivedMB = float64(netInfo[0].BytesRecv) / 1e6
    }

    metrics := SystemMetrics{
        CPUUsagePercent:   cpuUsage,
        MemoryUsedPercent: memoryUsedPercent,
        MemoryUsedGB:      memoryUsedGB,
        MemoryTotalGB:     memoryTotalGB,
        DiskUsedPercent:   diskUsedPercent,
        DiskUsedGB:        diskUsedGB,
        DiskTotalGB:       diskTotalGB,
        NetworkSentMB:     networkSentMB,
        NetworkReceivedMB: networkReceivedMB,
        Timestamp:         time.Now().Unix(),
    }

    if err := json.NewEncoder(w).Encode(metrics); err != nil {
        debugLog("Error encoding system metrics response:", err)
        http.Error(w, "Internal server error", http.StatusInternalServerError)
        return
    }
}

// API Handlers
func handleLiveData(w http.ResponseWriter, r *http.Request) {
    liveDataMutex.Lock()
    data := latestLiveData
    liveDataMutex.Unlock()
    if err := json.NewEncoder(w).Encode(data); err != nil {
        debugLog("Error encoding live data response:", err)
        http.Error(w, "Internal server error", http.StatusInternalServerError)
        return
    }
}

func handleHEXJSON(w http.ResponseWriter, r *http.Request) {
    data, err := loadLocalHEXJSON()
    if err != nil {
        debugLog("Error loading HEXJSON:", err)
        http.Error(w, err.Error(), http.StatusInternalServerError)
        return
    }
    if err := json.NewEncoder(w).Encode(data); err != nil {
        debugLog("Error encoding HEXJSON response:", err)
        http.Error(w, "Internal server error", http.StatusInternalServerError)
        return
    }
}

func handleMiners(w http.ResponseWriter, r *http.Request) {
    miners, err := loadMiners()
    if err != nil {
        debugLog("Error loading miners:", err)
        http.Error(w, err.Error(), http.StatusInternalServerError)
        return
    }
    if err := json.NewEncoder(w).Encode(miners); err != nil {
        debugLog("Error encoding miners response:", err)
        http.Error(w, "Internal server error", http.StatusInternalServerError)
        return
    }
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
        if err := json.NewEncoder(w).Encode(config); err != nil {
            debugLog("Error encoding config response:", err)
            http.Error(w, "Internal server error", http.StatusInternalServerError)
            return
        }
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
    os.MkdirAll("data", 0755)
    os.MkdirAll("settings", 0755)

    if err := updateLocalHEXJSON(); err != nil {
        debugLog("Error updating local HEXJSON:", err)
    }

    startDailyHEXJSONUpdate()

    config, err := loadConfig()
    if err != nil {
        debugLog("Error loading config:", err)
        config.LiveDataFrequency = defaultLiveDataFrequency
    }
    configManager.SetLiveDataFrequency(config.LiveDataFrequency)

    data, err := fetchLiveData()
    if err != nil {
        debugLog("Error during initial live data fetch:", err)
    } else {
        liveDataMutex.Lock()
        latestLiveData = data
        liveDataMutex.Unlock()
        debugLog("Initial live data fetched successfully")
    }

    go func() {
        frequency := configManager.GetLiveDataFrequency()
        if frequency <= 0 {
            debugLog("Invalid initial frequency, using default:", defaultLiveDataFrequency)
            frequency = defaultLiveDataFrequency
        }
        ticker := time.NewTicker(time.Duration(frequency) * time.Minute)
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
                frequency = configManager.GetLiveDataFrequency()
                if frequency <= 0 {
                    debugLog("Invalid frequency, using default:", defaultLiveDataFrequency)
                    frequency = defaultLiveDataFrequency
                }
                ticker.Reset(time.Duration(frequency) * time.Minute)
            case <-changeCh:
                frequency = configManager.GetLiveDataFrequency()
                if frequency <= 0 {
                    debugLog("Invalid frequency from change, using default:", defaultLiveDataFrequency)
                    frequency = defaultLiveDataFrequency
                }
                debugLog("Resetting ticker with frequency:", frequency)
                ticker.Reset(time.Duration(frequency) * time.Minute)
            }
        }
    }()

    http.Handle("/", http.FileServer(http.Dir("static")))
    http.HandleFunc("/api/live-data", handleLiveData)
    http.HandleFunc("/api/hexjson", handleHEXJSON)
    http.HandleFunc("/api/miners", handleMiners)
    http.HandleFunc("/api/add-miner", handleAddMiner)
    http.HandleFunc("/api/end-miner", handleEndMiner)
    http.HandleFunc("/api/delete-miner", handleDeleteMiner)
    http.HandleFunc("/api/config", handleConfig)
    http.HandleFunc("/api/system", handleSystemMetrics)

    log.Println("Server starting on :5555")
    if err := http.ListenAndServe(":5555", nil); err != nil {
        log.Fatal("Server failed:", err)
    }
}
