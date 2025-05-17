FROM golang:1.24-alpine AS builder
WORKDIR /app
COPY go.mod ./
RUN go mod download

COPY main.go ./
COPY static/ ./static/
COPY data/ ./data/
COPY settings/ ./settings/

RUN go get github.com/shirou/gopsutil/v3/cpu
RUN go get github.com/shirou/gopsutil/v3/disk
RUN go get github.com/shirou/gopsutil/v3/mem

RUN CGO_ENABLED=0 GOOS=linux go build -a -ldflags="-s -w" -o hexfetch .
RUN apk add upx
RUN upx --best hexfetch

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /app/hexfetch /hexfetch
COPY --from=builder --chown=1000:1000 /app/static /static
COPY --from=builder --chown=1000:1000 /app/data /data
COPY --from=builder --chown=1000:1000 /app/settings /settings

USER 1000:1000
WORKDIR /
VOLUME ["/data", "/settings"]

CMD ["/hexfetch"]
