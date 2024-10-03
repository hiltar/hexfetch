# hexfetch
Fetching HEX Pulsechain API with Golang


## Usage

`go run main.go` - This will use 1 T-Share  
`go run main.go 100` - T-Share Payout * 100 T-Shares

## Building Go package
```
go mod init hexfetch
go build
./hexfetch <T-Shares>
```
Alternatively, add alias into .bashrc: `alias hexfetch="~/hexfetch/hexfetch <T-Shares>"`  
Use new .bashrc: `source ~/.bashrc`  
Run `hexfetch`

Output:
```
HEX Price      : 0.004128 $
T-Share Price  : 149.92 $
T-Share Rate   : 36321.7 HEX
T-Share Payout : 753.181 HEX
T-Shares       : 100
```
If there are changes, it will output the changed values:
```
HEX Price      : 0.004128 $
T-Share Price  : 149.92 $
T-Share Rate   : 36321.7 HEX
T-Share Payout : 753.181 HEX
T-Shares       : 100

Changes since last fetch:
HEX Price      : 0.004129 $ (+0.000002)
T-Share Price  : 149.96 $ (+0.056300)
T-Share Rate   : 36321.7 HEX
T-Share Payout : 753.181 HEX
T-Shares       : 100
```
