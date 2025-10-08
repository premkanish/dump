# HFT Trading System

**Institutional-grade high-frequency trading system built with Rust**

> Built with Rust 2024 Edition and latest stable dependencies (Sep 2025)

## 🎯 Features

- **Low-latency execution**: Sub-millisecond order routing with P99 tracking
- **Multi-venue support**: Hyperliquid, Binance Futures, Interactive Brokers
- **ML-powered signals**: ONNX Runtime integration with ensemble models
- **Real-time risk management**: Kill switches, position limits, PnL tracking
- **Universe rotation**: Automated asset selection with multi-source scoring
- **Professional UI**: 60fps egui-based terminal with Bloomberg-style density
- **Secure credential storage**: OS keychain integration with AES-256 encryption

## 🚀 Quick Start

### Prerequisites

**Rust Installation** (requires Rust 1.82+ for edition 2024):
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable
```

**System Dependencies**:

macOS:
```bash
brew install pkg-config openssl dbus
```

Ubuntu/Debian:
```bash
sudo apt-get install -y pkg-config libssl-dev libdbus-1-dev
```

Windows:
- Install Visual Studio Build Tools 2022
- Enable "Desktop development with C++"

### Setup

```bash
# Clone repository
git clone https://github.com/vroha/toomo.git
cd toomo

# Configure environment
cp .env.example .env
# Edit .env with your API keys:
# HYPERLIQUID_API_KEY=your_key
# HYPERLIQUID_SECRET=your_secret
# ENABLE_UNIVERSE=false

# Build release binaries
cargo build --release

# Run tests
cargo test --all

# Start trading engine (paper trading mode)
cargo run --release -p engine

# In another terminal, launch desktop UI
cargo run --release -p terminal
```

## 📊 Architecture

### Stack Overview

- **Runtime**: Tokio 1.47 async runtime
- **Web**: Axum 0.8, Hyper 1.7, Tower 0.5
- **Data**: Arrow/Parquet 56.2, AWS SDK 1.106
- **ML**: ONNX Runtime 2.0-rc.10 with ndarray
- **UI**: egui 0.32.3 with 60fps rendering
- **Messaging**: async-nats 0.42 (optional)

### Components

```
┌─────────────────┐     WebSocket     ┌──────────────────┐
│   Terminal UI   │ ◄────────────────► │  Trading Engine  │
│   (egui 0.32)   │    Metrics/Risk    │   (Tokio 1.47)   │
└─────────────────┘                    └──────────────────┘
                                              │
                        ┌─────────────────────┼─────────────────────┐
                        │                     │                     │
                   ┌────▼────┐          ┌────▼────┐          ┌────▼────┐
                   │ Adapters│          │ Features│          │ ML Pool │
                   │ Layer   │          │ Builder │          │ (ONNX)  │
                   └────┬────┘          └─────────┘          └─────────┘
                        │
           ┌────────────┼────────────┐
           │            │            │
      ┌────▼────┐  ┌───▼────┐  ┌───▼────┐
      │Hyperlqd │  │Binance │  │  IBKR  │
      └─────────┘  └────────┘  └────────┘
```

## 🔧 Configuration

Edit `config/engine.toml`:

```toml
[engine]
mode = "paper"  # "paper" | "live" | "backtest"
feature_window_size = 1000
inference_timeout_ms = 3

[gate]
min_edge_bps = 5.0
min_confidence = 0.5
max_spread_bps = 10.0

[risk]
max_notional_per_symbol = 100000.0
max_total_notional = 500000.0
max_loss_per_day = 10000.0
```

## 🔐 Security

Credentials are stored in OS keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service):

```rust
use common::security::{CredentialStore, ApiCredentials};

let store = CredentialStore::new()?;
let creds = ApiCredentials::new(api_key, api_secret, is_paper);
store.save(Venue::Hyperliquid, "main", &creds)?;
```

## 📈 Performance Targets

- **Latency**: P99 < 1ms for feature computation
- **Throughput**: 1000+ snapshots/sec per symbol
- **ML Inference**: < 3ms timeout with fallback
- **UI**: Stable 60fps rendering

## 🧪 Testing

```bash
# Unit tests
cargo test

# Integration tests
cargo test --test integration

# Benchmarks
cargo bench

# Load testing (requires live connection)
cargo test --release --test load -- --ignored
```

## 📦 Deployment

### Docker

```bash
# Build engine image
docker build -t hft-engine -f Dockerfile.engine .

# Run with docker-compose
docker-compose up -d

# View logs
docker-compose logs -f engine
```

### AWS Deployment

```bash
# Build for Linux (if on macOS)
cargo build --release --target x86_64-unknown-linux-gnu

# Deploy to EC2 with systemd
scp target/release/engine user@ec2:~/
scp hft-engine.service user@ec2:/etc/systemd/system/
ssh user@ec2 'sudo systemctl enable --now hft-engine'
```

## 📚 Documentation

- [Architecture Deep Dive](./docs/ARCHITECTURE.md)
- [API Reference](./docs/API.md)
- [Feature Engineering](./docs/FEATURES.md)
- [Model Training](./docs/MODELS.md)

## 🛠️ Development

```bash
# Watch mode with auto-rebuild
cargo watch -x 'check --all' -x 'test --all'

# Format code
cargo fmt --all

# Lint with Clippy
cargo clippy --all-targets --all-features -- -D warnings

# Generate documentation
cargo doc --no-deps --open

# Profile with flamegraph (requires cargo-flamegraph)
cargo flamegraph --bin engine
```

## 🎮 Terminal Hotkeys

| Key | Action |
|-----|--------|
| `Ctrl+P` | Toggle Paper/Live mode |
| `Ctrl+K` | Activate kill switch |
| `Ctrl+R` | Refresh universe |
| `Ctrl+L` | Clear logs |
| `Ctrl+Q` | Quit |
| `Space` | Pause/Resume trading |

## 📊 Monitoring

### Prometheus Metrics

Engine exposes metrics on `http://localhost:9090/metrics`:

```
# Latency metrics
hft_ingest_duration_us
hft_feature_duration_us
hft_model_duration_us
hft_route_duration_us

# Throughput
hft_snapshots_per_sec
hft_orders_per_sec

# Errors
hft_dropped_frames_total
hft_model_timeouts_total
hft_order_rejects_total
```

### Grafana Dashboard

Import the dashboard from `config/grafana/hft-dashboard.json`

### WebSocket Streams

Connect to real-time metrics:

```javascript
// Performance metrics
ws://localhost:8081/metrics

// Risk snapshot
ws://localhost:8081/risk

// Critical alerts
ws://localhost:8081/alerts
```

## 🔬 ML Model Integration

Place ONNX models in `models/` directory:

```
models/
├── crypto/
│   ├── idec.onnx       # Deep classifier
│   ├── transformer.onnx # Attention model
│   ├── gbdt.onnx       # Gradient boosting
│   └── edge.onnx       # Fast edge model (< 1ms)
└── equity/
    ├── idec.onnx
    ├── transformer.onnx
    ├── gbdt.onnx
    └── edge.onnx
```

Models automatically loaded on startup. Falls back to rule-based if unavailable.

## 🌐 Supported Venues

### Hyperliquid
- Perpetual futures
- WebSocket L2 book updates
- Sub-millisecond order placement
- Native USDC settlement

### Binance Futures
- USDT and COIN-M futures
- REST + WebSocket APIs
- Position mode: One-way or Hedge
- Auto-deleverage protection

### Interactive Brokers (IBKR)
- US equities
- TWS Gateway integration
- Real-time market data (Level 1/2)
- Pre/post market trading

## 🧮 Feature Engineering

The system computes 13 core features per symbol:

1. **Order Flow Imbalance (OFI)**: Bid/ask flow delta
2. **Order Book Imbalance (OBI)**: Size imbalance at BBO
3. **Microprice**: Volume-weighted mid
4. **Spread (bps)**: Relative bid-ask spread
5. **Depth Imbalance**: Total bid/ask size ratio
6. **Realized Volatility**: Returns variance
7. **ATR**: Average true range
8. **Funding Rate**: 8h funding (futures)
9. **Impact Curve**: A * notional^β
10. **VWAP Ratio**: Current mid / recent VWAP
11. **Depth Alpha/Beta**: Power law parameters
12. **Trade Intensity**: Recent print frequency
13. **Time Features**: Time-of-day, day-of-week

Features normalized and fed to ONNX models at ~1ms latency.

## 🎯 Routing Logic

```rust
// Routing decision tree
if urgency > 0.8 && edge > 10bps {
    OrderStyle::TakerNow  // Market order
} else if urgency > 0.5 && spread < 3bps {
    OrderStyle::Sniper    // Join BBO
} else {
    OrderStyle::MakerPassive  // Post-only limit
}
```

Size calculation uses Kelly criterion with conservative scaling:
```rust
size = base_size * confidence² * (1 + urgency * 0.5)
```

## ⚠️ Risk Management

### Kill Switch Triggers
- Daily loss exceeds configured limit
- Position concentration > 25% of portfolio
- Unrealized loss > 2x average daily range
- Manual activation via UI or API

### Position Limits
- Per-symbol notional cap
- Total portfolio notional cap
- Max leverage multiplier
- Max holding time per position

### Circuit Breakers
- Wide spread detection (> 10bps)
- Low liquidity warning (< $500k)
- Extreme volatility pause (> 3 ATR move)

## 🔄 Universe Rotation

**Full Rebuild (every 120 min)**:
1. Fetch data from 10+ sources (Hyperliquid, DexScreener, GeckoTerminal, Birdeye, The Graph, CryptoPanic, etc.)
2. Compute composite scores per asset
3. Filter by volume, liquidity, availability
4. Select top 30 crypto + 20 equity
5. Store snapshot to database

**Quick Refresh (every 15 min)**:
1. Re-score current 50-asset universe
2. Refresh real-time metrics only
3. Select top 7 crypto + 3 equity for active trading
4. Apply anti-whiplash rules (10% score delta minimum)

## 📝 Logging

Structured JSON logs with tracing:

```bash
# View real-time logs
RUST_LOG=debug cargo run -p engine 2>&1 | jq

# Filter by component
RUST_LOG=engine::router=trace cargo run -p engine

# Production logging
RUST_LOG=info,engine::router=debug cargo run --release -p engine
```

## 🐛 Troubleshooting

### High Latency
```bash
# Check system load
top -o cpu

# Verify P99 latency
curl http://localhost:9090/metrics | grep p99

# Reduce feature window
# Edit config/engine.toml: feature_window_size = 500
```

### Model Timeouts
```bash
# Check ONNX Runtime logs
RUST_LOG=ort=debug cargo run -p engine

# Increase timeout (default 3ms)
# Edit config/engine.toml: inference_timeout_ms = 5

# Disable slow models
# Remove transformer.onnx and gbdt.onnx, keep edge.onnx only
```

### WebSocket Disconnects
```bash
# Check connection status
curl http://localhost:8081/health

# Review WS logs
RUST_LOG=engine::ws_server=trace cargo run -p engine

# Increase buffer size in crates/adapters/src/hyperliquid.rs
```

## 🤝 Contributing

1. Fork the repository
2. Create feature branch: `git checkout -b feature/amazing-feature`
3. Commit changes: `git commit -m 'Add amazing feature'`
4. Push to branch: `git push origin feature/amazing-feature`
5. Open Pull Request

### Code Standards
- Follow Rust API Guidelines
- Maintain test coverage > 80%
- Document all public APIs
- Run `cargo fmt` and `cargo clippy` before committing

## 📄 License

MIT License - See [LICENSE](./LICENSE) file for details

## ⚠️ Disclaimer

**This software is for educational and research purposes only.**

- Trading cryptocurrencies and equities involves substantial risk of loss
- Past performance does not guarantee future results
- The authors are not responsible for any financial losses
- Always test thoroughly in paper trading mode first
- Consult a qualified financial advisor before live trading
- This is NOT investment advice

## 🌟 Acknowledgments

- **Rust Community** for exceptional tooling
- **ONNX Runtime** team for high-performance inference
- **egui** for immediate-mode GUI framework
- **Tokio** for async runtime excellence
- **HFT Research Community** for pioneering techniques

## 📞 Support

- 📧 Email: team@toomo.ai
- 💬 Discord: [Join Server](https://discord.gg/toomo)
- 🐛 Issues: [GitHub Issues](https://github.com/vroha/toomo/issues)
- 📖 Wiki: [Project Wiki](https://github.com/vroha/toomo/wiki)

## 🗺️ Roadmap

### Q4 2025
- [ ] Multi-threading for order book processing
- [ ] Parquet export for ML training pipeline
- [ ] Advanced universe filters (sentiment, on-chain)
- [ ] Mobile monitoring app (React Native)

### Q1 2026
- [ ] Options market making strategies
- [ ] Cross-venue arbitrage detection
- [ ] Reinforcement learning agent integration
- [ ] Multi-account portfolio optimization

### Q2 2026
- [ ] FIX protocol support
- [ ] Market replay for backtesting
- [ ] GPU-accelerated feature computation
- [ ] Real-time strategy parameter tuning

---

**Built with ❤️ by the Toomo Team**

*Last Updated: October 2025 • Rust Edition 2024*
