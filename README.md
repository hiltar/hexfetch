# hexfetch

hexfetch is a web application for hexfetch with convenient features like HEX miners, charts and periodical data fetching from Pulsechain API.   
This doesn't itself interact Pulsechain network but instead it uses HEXDailyStats API to fetch data.   
hexfetch doesn't need the 0x addresses at all so it's 100% privacy.

hexfetch is made with `go 1.24.2`.

Running hexfetch will create a folder into `/opt/` path.     
settings directory contains user defined config.json and miners.json.

## Environment variables
| env  | value  | explanation  |
|---|---|---|
| DEBUG  | true/false  | Enable logging. Default value: false  |   


---

# hexfetch for arm architecture
This branch called `arm` is for arm based architecture microcontrollers such as `Raspberry Pi`.   
We are using `Raspberry Pi Zero 2W` for this build.

**NOTE: ALL COMMANDS MUST BE RUN AS SUDO!**

## Prepare SD-card
Format SD-card with `FAT32` partition called `ALPINE`.

## Initialize ALPINE partition
```
sudo mkdir /mnt/sdcard
sudo mount /dev/sdc1 /mnt/sdcard

# Extract archive into /mnt/sdcard
sudo tar -xzf alpine-rpi-<version>-aarch64.tar.gz -C /mnt/sdcard
```


## WiFi setup
`sudo nano /mnt/sdcard/wpa_supplicant.conf`
```
country=FI

network={
	key_mgmt=WPA-PSK
	ssid="mySSID"
	psk="myPassPhrase"
}
```

## Headless Alpine bootstrap
```
# Download from https://github.com/macmpi/alpine-linux-headless-bootstrap/releases
sudo mv headless.apkovl.tar.gz /mnt/sdcard
```

## hexfetch contents into sdcard
```
sudo mkdir /mnt/sdcard/hexfetch
# Copy hexfetch-arm executable into /mnt/sdcard/hexfetch

sync
sudo umount mnt/sdcard
```

## Unmount SD-card
```
sync
sudo umount /mnt/sdcard
```

## Connecting into rpi0
`ssh root@192.168.68.102`

## Setup the Alpine Linux
```
setup-alpine
# Proceed with default options.

# SSH
# Allow root ssh login: prohibit-password
# input public SSH key

# Disk & Install
none

# ramdisk filesystem
echo "tmpfs /mnt/ramdisk tmpfs size=8m,mode=0755 0 0" >> /etc/fstab
mount -a

# LBU
lbu add /etc/wpa_supplicant/
lbu commit -d

# You may reboot and check ramdisk filesystem
df -h
```

## Setup hexfetch
```
# SSH into rpi
ssh root@<rpi-ip>

# Copy hexfetch file contents from init.d folder into /etc/init.d/hexfetch

# Update RC configuration
chmod +x /etc/init.d/hexfetch
rc-update add hexfetch default
lbu add /etc/init.d/hexfetch

# Settings folder
mkdir /opt/hexfetch/
# Optionally move config.json and miners.json files into /opt/hexfetch/ folder

lbu add /opt/hexfetch/
lbu commit
reboot # after reboot hexfetch should be available from <rpi-ip>:5555

rc-service hexfetch status

# Add a daily cronjob:
crontab -e
1 3 * * * /etc/init.d/hexfetch restart # Restart hexfetch service every day at 03:01.
0 1 * * 5 lbu commit

# Disable SSH
rc-service sshd zap
apk del openssh

rc-status
lbu commit
```

---

## Updating Alpine Linux version
```
# https://alpinelinux.org/downloads/
sudo mount /dev/sdc1 /mnt/sdcard
sudo tar -xzf alpine-rpi-3.22.1-aarch64.tar.gz -C /mnt/sdcard --no-same-owner
sync
sudo unmount /mnt/sdcard

vi /etc/apk/repositories
# Change versions
apk update
apk upgrade
lbu commit
reboot
```

## Updating hexfetch
To run newer hexfetch executable, power off the raspberry pi and pull sd-card out. Plug it into PC.  
```
sudo mount /dev/sdc1 /mnt/sdcard
sudo mv hexfetch-arm /mnt/sdcard/hexfetch
# If static files are updated:
sudo rm -r /mnt/sdcard/hexfetch/static
sudo cp -r /static /mnt/sdcard/hexfetch

sync
sudo umount /mnt/sdcard
```

## Notes
Any changes in settings or ending a miner in profile must be saved with `lbu commit`.  
Daily cronjob is for fetching latest data for Charts tab because code isn't doing it every day at 00:00 UTC.

## Alpine Local Backup tool (LBU)
Local backup utility(lbu) is the Alpine Linux tool to manage Diskless Mode installations. For these installations, `lbu` tool must be used to commit the changes whenever Alpine Package Keeper is used.

When Alpine Linux boots in diskless mode, it initially only loads a few required packages from the boot device. However, local adjustments to what-gets-loaded-into-RAM are possible, e.g. installing a package or adjusting the configuration files in /etc. The modifications can be saved with `lbu` tool to an overlay file i.e apkovl file that can be automatically loaded when booting, to restore the saved state.

By default, an `lbu commit` only stores modifications below /etc, with the exception of the /etc/init.d/ directory. If a user was created during the setup-alpine script, that user's home directory is also added to the paths that lbu will backup up. However, `lbu add` enables modifying that set of included files, and can be used to specify additional files or folders. 

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

## Settings
Settings tab shows:  
  - Live Data Settings for changing the frequency of fetching data (in minutes)
  - Liquid HEX to calculate and show the value of liquid HEX in Profile tab  
  - Add New Miner for adding HEX miner with start date, end date and amount of T-Shares  
  - Existing Miners for list of HEX miners with Delete function
