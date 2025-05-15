# hexfetch

hexfetch is a web application for hexfetch-cli with convenient features like HEX miners, charts and periodical data fetching from Pulsechain API.   
This doesn't itself interact Pulsechain network but instead it uses HEXDailyStats API to fetch data.   
hexfetch doesn't need the 0x addresses at all so it's 100% privacy.

hexfetch is made with `go 1.24.2`.

Running hexfetch will create two folders into same directory where hexfetch is running.   
data directory contains hexdata.json.  
settings directory contains user defined config.json and miners.json.

## Upcoming features
Optimization   
Charts with functions   
Enable System tab with env var  

---

# Build
```
go mod init hexfetch
go mod tidy
# Run go get to download dependencies

CGO_ENABLED=0 GOOS=linux go build -a -ldflags="-s -w" -o hexfetch

chmod a+x hexfetch
./hexfetch
```

# Docker
```
docker build hexfetch:latest .
docker run -d --name hexfetch -m 256m -p 5555:5555 -v $(pwd)/data:/data -v $(pwd)/settings:/settings hexfetch:latest
# Alternatively use run.sh

curl http://127.0.0.1:5555
```

## Environment variables
| env  | value  | explanation  |
|---|---|---|
| DEBUG  | true/false  | Enable logging. Default value: false  |   

---

# Tabs

## Profile
Profile tab shows user's miners and T-Shares and total value of T-Shares.   
If miner is matured, it will be shown **(MATURED)** with `END` button. Ending the miner will move it into `Completed Miners` container.  

Viewing Completed Miners button opens a window of completed HEX miners.


## Live Data
Live Data tab shows periodically fetched data from Pulsechain API.


## Charts
Charts tab show historical charts of HEX price, T-Share rate and other useful information of HEX.


## System
System related information:
  - CPU
  - Disk
  - Memory


## Settings
Settings tab shows:  
  - Live Data Settings for changing the frequency of fetching data (in minutes)
  - Liquid HEX to calculate and show the value of liquid HEX in Profile tab  
  - Add New Miner for adding HEX miner with start date, end date and amount of T-Shares  
  - Existing Miners for list of HEX miners with Delete function


