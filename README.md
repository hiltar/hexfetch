# hexfetch

hexfetch is a web application for hexfetch with convenient features like HEX miners, charts and periodical data fetching from Pulsechain API.  
hexfetch doesn't need the 0x addresses at all so it's 100% privacy.

Running hexfetch will create a folder into `/opt/` path.  
settings directory contains user defined config.json and miners.json.

---

# Tabs

## Profile
Profile tab shows user's historical portfolio value chart, miners, daily interest, T-Shares and total value of T-Shares.   
If miner is matured, it will be shown **(MATURED)** with `END` button. Ending the miner will move it into `Completed Miners` container.  

Viewing Completed Miners button opens a window of completed HEX miners.

## Live Data
Live Data tab shows periodically fetched data from Pulsechain API.

## Charts
Charts tab show historical charts of HEX price, T-Share rate and other useful information of HEX.

## Settings
Settings tab shows:  
  - Live Data Settings for changing the frequency of fetching data (in minutes)
  - Liquid HEX to calculate and show the value of liquid HEX in Profile tab
  - Historical chart's starting day  
  - Add New Miner for adding HEX miner with start date, end date and amount of T-Shares  
  - Existing Miners for list of HEX miners with Delete function
