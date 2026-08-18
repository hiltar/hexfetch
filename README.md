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

---

# Flashing into ESP32 microcontroller

```
# Clone the repository
git clone https://github.com/hiltar/hexfetch.git -b esp32
cd hexfetch/esp32

# Depending on ESP32 microcontroller, there are variations. e.g. -S3, -C3, -C5 etc.
# Some of them have different configurations. For this guide, we use ESP32-S3.
# It's possible to change the variation in .cargo/config.toml.

 # Install ESP32 and Rust related tools - Fedora Linux
sudo dnf install -y git gcc gcc-c++ cmake ninja-build ccache \
                    python3 python3-pip systemd-devel perl

# Rust via rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"

# ESP-specific tools
cargo +stable install --locked espup ldproxy espflash

# Xtensa Rust toolchain ("esp" channel) + GCC + libclang
espup install --targets esp32s3

# Put xtensa gcc / libclang on PATH & LIBCLANG_PATH
. "$HOME/export-esp.sh"
echo '. "$HOME/export-esp.sh"' >> "$HOME/.bashrc"   # persist

# WiFi credentials
export HEX_WIFI_SSID="YourSSID"
export HEX_WIFI_PASS="YourPassword"

# Build in esp32 directory
cargo build --release

# Flash into ESP32-S3
sudo usermod -aG dialout $USER && newgrp dialout # IF PERMISSION DENIED
cargo espflash flash --release --monitor
```
