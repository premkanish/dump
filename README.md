# HFT Trading System

**Institutional-grade high-frequency trading system built with Rust**

## 🚀 Quick Start

### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# macOS
brew install pkg-config openssl dbus

# Ubuntu/Debian
sudo apt-get install -y pkg-config libssl-dev libdbus-1-dev
```

### Setup

```bash
# Clone the repository
git clone https://github.com/vroha/toomo.git
cd toomo

# Configure environment
cp .env.example .env
# Edit .env with your API keys

# Build
cargo build --release

# Run tests
cargo test

# Start engine (paper trading)
cargo run --release -p engine

# In another terminal, start UI
cargo run --release -p terminal
```

## 📚 Full Documentation

See [ARCHITECTURE.md](./docs/ARCHITECTURE.md) for system design details.

## ⚠️ Disclaimer

**Educational purposes only. Trading involves substantial risk.**

## 📄 License

MIT License - See LICENSE file
