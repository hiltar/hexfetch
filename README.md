# hexfetch-web

hexfetch-web is a web application for hexfetch with convenient features like HEX miners and periodical data fetching from Pulsechain API.   
This doesn't itself interact Pulsechain network but instead it uses HEXDailyStats API to fetch data.   
hexfetch-web doesn't need the 0x addresses at all so it's 100% privacy.

hexfetch-web is made with `go 1.24.2`.

Running hexfetch-web will create two folders into same directory where hexfetch-web is running.   
data directory contains hexdata.json.  
settings directory contains user defined config.json and miners.json.

## Upcoming features
Charts tab is disabled in the code because it's not ready.   
For now, it only shows chart as image without any functions.

Better UI/UX   
Optimization   
Docker container   

---

# Build
```
go mod init hexfetch-web
go mod tidy
go build -o hexfecth-web

./hexfetch-web
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
Not yet implemented


## Settings
Settings tab shows:  
  - Live Data Settings for changing the frequency of fetching data (in minutes)  
  - Add New Miner for adding HEX miner with start date, end date and amount of T-Shares  
  - Existing Miners for list of HEX miners with Delete function  


# hexfetch-web
