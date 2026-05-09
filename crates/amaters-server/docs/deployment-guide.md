# Deployment Guide

Patterns for deploying `amaters-server` on systemd, Docker, and Kubernetes, including TLS preparation and rate-limit tuning.

Related docs: [Configuration Reference](configuration-reference.md) | [Operations Guide](operations-guide.md) | [Troubleshooting Guide](troubleshooting-guide.md)

## Build

`amaters-server` is Pure Rust by default. Build a release binary:

```bash
cargo build --release --bin amaters-server
```

For a static `musl` binary suitable for `FROM scratch` containers:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --bin amaters-server
```

Binary path: `target/x86_64-unknown-linux-musl/release/amaters-server`.

The default features are 100% Pure Rust. Optional features (FHE backend, hardware accelerators) are gated and may pull C/C++ deps — read each `Cargo.toml` `[features]` section before enabling.

## Filesystem layout

A typical production layout:

```
/etc/amaters/
├── config.toml              # Server config (TOML)
├── server.crt               # PEM cert chain
├── server.key               # PEM private key
├── ca.crt                   # CA bundle for mTLS (optional)
├── api-keys.json            # API key list (when auth.api_key.enabled)
├── roles.toml               # Custom RBAC roles (optional)
└── policies.toml            # Custom RBAC policies (optional)

/var/lib/amaters/
├── data/                    # server.data_dir
├── data/wal/                # storage.wal.dir (relative to data_dir)
└── snapshots/               # backup destination

/var/log/amaters/
└── amaters.log              # logging.file_path target

/var/run/
└── amaters-server.pid       # server.pid_file
```

Match the `[server]`, `[storage.wal]`, `[logging]` and `[network]` keys to these paths. See [Configuration Reference](configuration-reference.md) for the full schema.

## TLS / mTLS preparation

### Self-signed cert (testing only)

```bash
openssl req -x509 -newkey rsa:4096 -nodes \
  -keyout /etc/amaters/server.key \
  -out    /etc/amaters/server.crt \
  -days 365 \
  -subj "/CN=amaters.internal"

chmod 600 /etc/amaters/server.key
```

### Production: integrate with your CA

```bash
# Generate CSR
openssl req -new -newkey rsa:4096 -nodes \
  -keyout /etc/amaters/server.key \
  -out    /etc/amaters/server.csr \
  -subj   "/CN=amaters.example.com/O=Example Corp"

# Submit /etc/amaters/server.csr to your CA, receive server.crt
```

### mTLS: prepare client certs

```bash
# Trust bundle for the server (set network.tls_ca to this path)
cat client-ca-1.pem client-ca-2.pem > /etc/amaters/ca.crt

# In config.toml
# [network]
# tls_enabled         = true
# tls_cert            = "/etc/amaters/server.crt"
# tls_key             = "/etc/amaters/server.key"
# tls_ca              = "/etc/amaters/ca.crt"
# require_client_cert = true
```

For `auth.mtls.enabled = true`, point `ca_certs_dir` at a directory of trusted CAs (one PEM per file).

### Permissions

```bash
chown -R amaters:amaters /etc/amaters /var/lib/amaters /var/log/amaters
chmod 700 /etc/amaters
chmod 600 /etc/amaters/*.key
```

## systemd

`/etc/systemd/system/amaters-server.service`:

```ini
[Unit]
Description=AmateRS Database Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=amaters
Group=amaters
ExecStart=/usr/local/bin/amaters-server start --config /etc/amaters/config.toml
ExecStop=/usr/local/bin/amaters-server stop  --config /etc/amaters/config.toml
ExecReload=/bin/kill -HUP $MAINPID
KillSignal=SIGTERM
TimeoutStopSec=60
Restart=on-failure
RestartSec=5

# Hardening
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true
ReadWritePaths=/var/lib/amaters /var/log/amaters /var/run

# Resource caps
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
```

Activate:

```bash
systemctl daemon-reload
systemctl enable --now amaters-server
systemctl status amaters-server
journalctl -u amaters-server -f
```

`SIGHUP` reload integrates cleanly with `systemctl reload`:

```bash
systemctl reload amaters-server
```

`Type=notify` is not currently used because `amaters-server` does not call `sd_notify()`. Use `Type=simple` and a small `RestartSec` to recover from start-time failures. If you instrument with `sd-notify`, switch to `Type=notify`.

## Docker

### Pure Rust scratch image (musl static build)

```dockerfile
# syntax=docker/dockerfile:1
FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY . .
RUN rustup target add x86_64-unknown-linux-musl && \
    cargo build --release --target x86_64-unknown-linux-musl --bin amaters-server

FROM scratch
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/amaters-server /amaters-server
COPY config.toml /etc/amaters/config.toml
EXPOSE 7878 9090
ENTRYPOINT ["/amaters-server"]
CMD ["start", "--config", "/etc/amaters/config.toml"]
```

A `FROM scratch` image keeps the runtime to a single static binary. No C runtime, no CA bundle — provide CA certs as mounted volumes if you need TLS verification of upstreams.

### Distroless alternative (dynamic build)

```dockerfile
FROM rust:1.85 AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin amaters-server

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /build/target/release/amaters-server /usr/local/bin/amaters-server
COPY config.toml /etc/amaters/config.toml
EXPOSE 7878 9090
ENTRYPOINT ["/usr/local/bin/amaters-server"]
CMD ["start", "--config", "/etc/amaters/config.toml"]
```

Distroless includes the C runtime and CA bundle (`/etc/ssl/certs/ca-certificates.crt`), avoiding a static-link build at the cost of a slightly larger image.

### docker-compose

```yaml
services:
  amaters:
    image: amaters-server:0.2.1
    ports:
      - "7878:7878"
      - "9090:9090"
    environment:
      AMATERS_LOG_LEVEL: info
      AMATERS_BIND_ADDRESS: 0.0.0.0:7878
    volumes:
      - ./config.toml:/etc/amaters/config.toml:ro
      - amaters-data:/var/lib/amaters
      - amaters-logs:/var/log/amaters
    healthcheck:
      test: ["CMD", "/amaters-server", "status"]
      interval: 30s
      timeout: 5s
      retries: 3
volumes:
  amaters-data:
  amaters-logs:
```

## Kubernetes

A minimal `StatefulSet`:

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: amaters
spec:
  serviceName: amaters
  replicas: 3
  selector:
    matchLabels:
      app: amaters
  template:
    metadata:
      labels:
        app: amaters
    spec:
      containers:
        - name: amaters
          image: amaters-server:0.2.1
          ports:
            - containerPort: 7878
              name: grpc
            - containerPort: 9090
              name: metrics
          env:
            - name: AMATERS_LOG_LEVEL
              value: info
          volumeMounts:
            - name: config
              mountPath: /etc/amaters
              readOnly: true
            - name: data
              mountPath: /var/lib/amaters
          readinessProbe:
            httpGet:
              path: /metrics
              port: metrics
            initialDelaySeconds: 5
            periodSeconds: 10
          livenessProbe:
            httpGet:
              path: /metrics
              port: metrics
            initialDelaySeconds: 30
            periodSeconds: 30
      volumes:
        - name: config
          configMap:
            name: amaters-config
  volumeClaimTemplates:
    - metadata:
        name: data
      spec:
        accessModes: ["ReadWriteOnce"]
        resources:
          requests:
            storage: 10Gi
```

`/metrics` doubles as a coarse readiness/liveness probe today. A dedicated `/healthz` endpoint is planned.

## Rate-limit and connection-tuning rules of thumb

The relevant knobs in `[server]` and `[network]`:

| Workload profile | `max_connections` | `connection_timeout_secs` | `keepalive_interval_secs` |
|------------------|-------------------|---------------------------|---------------------------|
| Internal API behind a load balancer | 256–512 | 10 | 30 |
| Public TLS endpoint | 1000–4000 | 30 | 60 |
| Long-lived FHE clients | 256 | 60 | 120 |
| High-churn batch jobs | 4000–8000 | 5 | 15 |

Guidance:

- `max_connections` is hot-reloadable via `SIGHUP`; tune in production without a restart.
- `connection_timeout_secs` is the initial-handshake budget. Lower it (5–10 s) to fail fast under stampedes; raise it (30–60 s) for FHE workloads where handshake includes large public-key uploads.
- `keepalive_interval_secs` should be smaller than the smallest network idle timeout in front of the server (LB, NAT). 60 s is a safe default for most deployments.
- HTTP/2 keepalives can be tuned at the tonic builder level; today the server takes them implicitly from `keepalive_interval_secs`.

## mTLS client setup

To talk to a server with `network.require_client_cert = true`:

```bash
openssl s_client -connect amaters.example.com:7878 \
  -cert client.pem -key client.key \
  -CAfile /etc/amaters/ca.crt -alpn h2

# AmateRS Rust SDK
let tls = TlsConfig::new()
    .with_ca_cert("/etc/amaters/ca.crt")
    .with_client_cert("/path/to/client.pem", "/path/to/client.key")
    .with_domain_name("amaters.example.com");
let config = ClientConfig::new("https://amaters.example.com:7878").with_tls(tls);
let client = AmateRSClient::connect_with_config(config).await?;
```

When `auth.mtls.allowed_organizations` is non-empty, the client cert's `O=` field must match one of the entries.
