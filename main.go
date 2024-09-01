package main

import (
	"encoding/json"
	"fmt"
	"io/ioutil"
	"net/http"
	"strconv"
	"os"
)

// ApiResp matches the structure of the JSON response
type ApiResp struct {
	HexPrice         float64 `json:"price_Pulsechain"`
	TSharePrice   float64 `json:"tsharePrice_Pulsechain"`
	TShareRateHEX float64 `json:"tshareRateHEX_Pulsechain"`
	TSharePayout float64 `json:"payoutPerTshare_Pulsechain"`
}

func calculateTSharePayout(TShares int, apiresponse ApiResp) float64 {
	return apiresponse.TSharePayout * float64(TShares)
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

	req, err := http.NewRequest("GET", "https://hexdailystats.com/livedata", nil)
	if err != nil {
		fmt.Println("Error creating request:", err)
		return
	}
	req.Header.Set("Accept", "application/json")
	client := &http.Client{}
	resp, err := client.Do(req)
	if err != nil {
		fmt.Println("Error making request:", err)
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		fmt.Println("Received non-OK HTTP status:", resp.StatusCode)
		return
	}

	body, err := ioutil.ReadAll(resp.Body)
	if err != nil {
		fmt.Println("Error reading response body:", err)
		return
	}

	var apiresponse ApiResp
	err = json.Unmarshal(body, &apiresponse)
	if err != nil {
		fmt.Println("Error unmarshaling JSON:", err)
		return
	}


	TSharesPayout := calculateTSharePayout(TShares, apiresponse)

	//fmt.Println("Raw Response Body:", string(body))
	// Structure output
	fmt.Printf("%-14s : %3.6f $\n", "    HEX Price", apiresponse.HexPrice)
	fmt.Printf("%-14s : %3.2f $\n", "T-Share Price", apiresponse.TSharePrice)
	fmt.Printf("%-14s : %3.1f HEX\n", "T-Share Rate", apiresponse.TShareRateHEX)
	fmt.Printf("%-14s : %3.3f HEX\n", "T-Share Payout", TSharesPayout)

}
