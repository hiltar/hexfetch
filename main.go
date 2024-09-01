package main

import (
	"encoding/json"
	"fmt"
	"io/ioutil"
	"net/http"
	"strconv"
	"os"
)


type ApiResp struct {
	HexPrice         float64 `json:"price_Pulsechain"`
	TSharePrice   float64 `json:"tsharePrice_Pulsechain"`
	TShareRateHEX float64 `json:"tshareRateHEX_Pulsechain"`
	TSharePayout float64 `json:"payoutPerTshare_Pulsechain"`
}


func main() {

	TShares := 1
	if len(os.Args) > 1 {
		var err error
		TShares, err = strconv.Atoi(os.Args[1])
		if err != nil {
			fmt.Println("Invalid number of TShares:", err)
			os.Exit(1)
		}
	}


	apiresponse, err := fetchApiData()
	if err != nil {
		fmt.Println("Error fetching API data:", err)
		return
	}


	TSharesPayout := calculateTSharePayout(TShares, apiresponse)

	currentData := map[string]interface{}{
		"HEX Price":       apiresponse.HexPrice,
		"T-Share Price":   apiresponse.TSharePrice,
		"T-Share Rate":    apiresponse.TShareRateHEX,
		"T-Share Payout":  TSharesPayout,
		"TShares":         TShares,
	}

	const filename = "saved_hexdata.json"
	savedData, err := loadFromFile(filename)
	if err != nil && !os.IsNotExist(err) {
		fmt.Println(err)
	}

	displayData(apiresponse, TSharesPayout, TShares)
	compareData(currentData, savedData, TShares)

	if err := saveToFile(filename, currentData); err != nil {
		fmt.Println(err)
	}
}


func calculateTSharePayout(TShares int, apiresponse ApiResp) float64 {
	return apiresponse.TSharePayout * float64(TShares)
}


func saveToFile(filename string, data map[string]interface{}) error {
	file, err := json.MarshalIndent(data, "", "	")
	if err != nil {
		return err
	}
	return ioutil.WriteFile(filename, file, 0644)
}


func loadFromFile(filename string) (map[string]interface{}, error) {
	data := make(map[string]interface{})
	file, err := ioutil.ReadFile(filename)
	if err != nil {
		return data, err
	}
	err = json.Unmarshal(file, &data)
	return data, err
}


func fetchApiData() (ApiResp, error) {
	var apiresponse ApiResp

	req, err := http.NewRequest("GET", "https://hexdailystats.com/livedata", nil)
	if err != nil {
		return apiresponse, err
	}
	req.Header.Set("Accept", "application/json")
	client := &http.Client{}
	resp, err := client.Do(req)
	if err != nil {
		return apiresponse, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return apiresponse, fmt.Errorf("Received non-OK HTTP status: %d", resp.StatusCode)
	}

	body, err := ioutil.ReadAll(resp.Body)
	if err != nil {
		return apiresponse, err
	}

	err = json.Unmarshal(body, &apiresponse)
	if err != nil {
		return apiresponse, err
	}

	return apiresponse, nil
}

func displayData(apiresponse ApiResp, TSharesPayout float64, TShares int) {
	// Structure output
	fmt.Printf("%-14s : %3.6f $\n", "HEX Price", apiresponse.HexPrice)
	fmt.Printf("%-14s : %3.2f $\n", "T-Share Price", apiresponse.TSharePrice)
	fmt.Printf("%-14s : %3.1f HEX\n", "T-Share Rate", apiresponse.TShareRateHEX)
	fmt.Printf("%-14s : %3.3f HEX\n", "T-Share Payout", TSharesPayout)
	fmt.Printf("%-14s : %1d\n", "T-Shares", TShares)
}


func compareData(currentData, savedData map[string]interface{}, TShares int) {

	hasChanges := false
	var changes []string

	keys := []string{
		"HEX Price",
		"T-Share Price",
		"T-Share Rate",
		"T-Share Payout",
		"T-Shares",
	}

	formatting := map[string]string{
		"HEX Price":       "%3.6f $",
		"T-Share Price":   "%3.2f $",
		"T-Share Rate":    "%3.1f HEX",
		"T-Share Payout":  "%3.3f HEX",
		"T-Shares":        "%d",
	}


	currentData["T-Shares"] = TShares

	for _, key := range keys {
		currentValue, currentOk := currentData[key]
		savedValue, savedOk := savedData[key]
		format := formatting[key]

		if currentOk {
			switch key {
			case "T-Shares":
				cv, _ := currentValue.(int)
				sv, savedOk := savedValue.(float64)
				if savedOk {
					svInt := int(sv)
					if cv != svInt {
						hasChanges = true
						changes = append(changes, fmt.Sprintf("%-14s : %d (%+d)", key, cv, cv-svInt))
					} else {
						changes = append(changes, fmt.Sprintf("%-14s : %d", key, cv))
					}
				} else {
					changes = append(changes, fmt.Sprintf("%-14s : %d", key, cv))
				}
			default:
				if savedOk {
					switch cv := currentValue.(type) {
					case float64:
						sv := savedValue.(float64)
						diff := cv - sv
						if diff != 0 {
							hasChanges = true
							changes = append(changes, fmt.Sprintf("%-14s : "+format+" (%+1.6f)", key, cv, diff))
						} else {
							changes = append(changes, fmt.Sprintf("%-14s : "+format, key, cv))
						}
					}
				} else {
					if key == "T-Shares" {
						changes = append(changes, fmt.Sprintf("%-14s : %d", key, TShares))
					} else {
						changes = append(changes, fmt.Sprintf("%-14s : %+12.6f HEX", key, currentValue))
					}
				}
			}
		} else {
			if key == "T-Shares" {
				changes = append(changes, fmt.Sprintf("%-14s : %d", key, TShares))
			} else {
				changes = append(changes, fmt.Sprintf("%-14s : %+12.6f HEX", key, currentValue))
			}
		}
	}

	if hasChanges {
		fmt.Println("\nChanges since last fetch:")
		for _, change := range changes {
			fmt.Println(change)
		}
	}
}
