# hexfetch-web

hexfetch-web is a web application for hexfetch with convenient features like HEX miners and periodical data fetching from Pulsechain API.   
This doesn't itself interact Pulsechain network but instead it uses HEXDailyStats API to fetch data.   
hexfetch-web doesn't need the 0x addresses at all so it's 100% privacy.

hexfetch-web is made with `go 1.24.2`.

Running hexfetch-web will create two folders into same directory where hexfetch-web is running.   
data directory contains hexdata.json.  
settings directory contains user defined config.json and miners.json.

## Upcoming features
Better UI/UX   
Optimization   
Charts   

---

# Build
```
go mod init hexfetch-web
go mod tidy
go build -o hexfecth-web

chmod a+x hexfetch-web
./hexfetch-web
```

# Docker
```
docker build hexfetch:v0.1.0 .
docker run -d --name hexfetch -p 5555:5555 -v $(pwd)/data:/data -v $(pwd)/settings:/settings hexfetch:v0.1.0
# Alternatively use run.sh

curl http://127.0.0.1:5555
```

---

# Tabs

## Profile
Profile tab shows user's miners and T-Shares and total value of T-Shares.   
If miner is matured, it will be shown **(MATURED)** with `END` button. Ending the miner will move it into `Completed Miners` container.



Viewing Completed Miners button opens a window of completed HEX miners.




## Live Data
Live Data tab shows periodically fetched data from Pulsechain API.




# Charts
Charts tab show historical charts of HEX price, T-Share rate and other useful information of HEX.


## Settings
Settings tab shows:  
  - Live Data Settings for changing the frequency of fetching data (in minutes)  
  - Add New Miner for adding HEX miner with start date, end date and amount of T-Shares  
  - Existing Miners for list of HEX miners with Delete function


