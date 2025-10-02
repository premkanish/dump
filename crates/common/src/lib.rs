// crates/common/src/lib.rs
use serde::{Deserialize, Serialize};
use std::fmt;
use ordered_float::OrderedFloat;

pub mod security;
pub mod error;
pub mod metrics;
pub mod config;

pub use error::{Result, Error};

/// Asset categories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetCategory {
    Equity,
    CryptoFutures,
}

/// Supported venues
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Venue {
    IBKR,
    Hyperliquid,
    BinanceFutures,
}

impl Venue {
    pub fn category(&self) -> AssetCategory {
        match self {
            Venue::IBKR => AssetCategory::Equity,
            Venue::Hyperliquid | Venue::BinanceFutures => AssetCategory::CryptoFutures,
        }
    }
}

/// Trading mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradingMode {
    Backtest,
    Paper,
    Live,
    Paused,
}

/// Order side
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

/// Order type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    Market,
    Limit,
    PostOnly,
    IOC,
    FOK,
}

/// Order style for routing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStyle {
    MakerPassive,
    TakerNow,
    Sniper,
}

/// Order request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub client_id: String,
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub quantity: f64,
    pub price: Option<f64>,
    pub reduce_only: bool,
    pub time_in_force: TimeInForce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce {
    GTC,
    IOC,
    FOK,
    GTX,
}

/// Order acknowledgment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAck {
    pub venue_order_id: String,
    pub client_id: String,
    pub status: OrderStatus,
    pub timestamp_ns: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    Pending,
    Accepted,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

/// Position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    pub size: f64, // positive = long, negative = short
    pub entry_price: f64,
    pub mark_price: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub leverage: f64,
    pub margin_used: f64,
    pub liquidation_price: Option<f64>,
}

/// Balance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    pub asset: String,
    pub free: f64,
    pub locked: f64,
    pub total: f64,
}

/// Fee tier information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeTier {
    pub maker_fee_bps: f64,
    pub taker_fee_bps: f64,
    pub volume_30d: f64,
}

/// Order book level
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Level {
    pub price: OrderedFloat<f64>,
    pub quantity: f64,
}

/// Order book snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBook {
    pub symbol: String,
    pub timestamp_ns: i64,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
    pub sequence: u64,
}

impl OrderBook {
    pub fn best_bid(&self) -> Option<&Level> {
        self.bids.first()
    }
    
    pub fn best_ask(&self) -> Option<&Level> {
        self.asks.first()
    }
    
    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid.price.0 + ask.price.0) / 2.0),
            _ => None,
        }
    }
    
    pub fn spread_bps(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => {
                let mid = (bid.price.0 + ask.price.0) / 2.0;
                Some((ask.price.0 - bid.price.0) / mid * 10000.0)
            }
            _ => None,
        }
    }
}

/// Trade execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub symbol: String,
    pub timestamp_ns: i64,
    pub price: f64,
    pub quantity: f64,
    pub side: Side,
    pub trade_id: String,
}

/// Market data snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub timestamp_ns: i64,
    pub symbol: String,
    pub orderbook: OrderBook,
    pub recent_trades: Vec<Trade>,
    pub funding_rate_bps: Option<f64>,
    pub open_interest: Option<f64>,
    pub volume_24h: f64,
}

/// Risk limits per account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskLimits {
    pub max_notional_per_symbol: f64,
    pub max_total_notional: f64,
    pub max_leverage: f64,
    pub max_loss_per_day: f64,
    pub max_position_concentration: f64, // % of portfolio
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            max_notional_per_symbol: 100_000.0,
            max_total_notional: 500_000.0,
            max_leverage: 3.0,
            max_loss_per_day: 10_000.0,
            max_position_concentration: 0.25,
        }
    }
}

/// Account configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    pub label: String,
    pub venue: Venue,
    pub is_paper: bool,
    pub risk_limits: RiskLimits,
}

/// Universe asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniverseAsset {
    pub symbol: String,
    pub venue: Venue,
    pub category: AssetCategory,
    pub score: f64,
    pub rank: usize,
    pub metrics: AssetMetrics,
}

/// Asset metrics for scoring
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetMetrics {
    // Common
    pub volume_24h_usd: f64,
    pub liquidity_usd: f64,
    
    // Crypto-specific
    pub funding_rate_bps: Option<f64>,
    pub open_interest_usd: Option<f64>,
    pub tx_count_1h: Option<u64>,
    pub social_mentions_24h: Option<u64>,
    
    // Equity-specific
    pub market_cap_usd: Option<f64>,
    pub short_interest_pct: Option<f64>,
    pub options_volume: Option<u64>,
    pub analyst_rating: Option<f64>,
    pub volatility_30d: Option<f64>,
}

/// Feature vector for ML models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureVec {
    pub timestamp_ns: i64,
    pub symbol: String,
    pub mid_price: f64,
    pub spread_bps: f64,
    pub ofi_1s: f64,
    pub obi_1s: f64,
    pub depth_imbalance: f64,
    pub depth_a: f64,
    pub depth_beta: f64,
    pub realized_vol_5s: f64,
    pub atr_30s: f64,
    pub funding_bps_8h: f64,
    pub impact_bps_1pct: f64,
    pub microprice: f64,
    pub vwap_ratio: f64,
}

/// Model prediction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    pub timestamp_ns: i64,
    pub symbol: String,
    pub edge_bps: f64,
    pub confidence: f64,
    pub horizon_ms: u64,
    pub model_version: String,
}

/// Routing decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDecision {
    pub style: OrderStyle,
    pub size_fraction: f64,
    pub hold_duration_s: f64,
    pub urgency: f64, // 0.0 = patient, 1.0 = urgent
    pub should_trade: bool,
    pub reason: String,
}

/// Performance metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    // Latency (microseconds)
    pub ingest_p50_us: f64,
    pub ingest_p95_us: f64,
    pub ingest_p99_us: f64,
    pub feature_p50_us: f64,
    pub feature_p95_us: f64,
    pub feature_p99_us: f64,
    pub model_p50_us: f64,
    pub model_p95_us: f64,
    pub model_p99_us: f64,
    pub route_p50_us: f64,
    pub route_p95_us: f64,
    pub route_p99_us: f64,
    
    // Throughput
    pub snapshots_per_sec: f64,
    pub orders_per_sec: f64,
    
    // Errors
    pub dropped_frames: u64,
    pub model_timeouts: u64,
    pub order_rejects: u64,
}

/// Risk snapshot for UI
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RiskSnapshot {
    pub timestamp_ns: i64,
    pub gross_notional: f64,
    pub net_notional: f64,
    pub num_positions: usize,
    pub total_margin_used: f64,
    pub available_margin: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub total_pnl: f64,
    pub daily_pnl: f64,
    pub var_95: f64,
    pub max_leverage: f64,
    pub kill_switch_active: bool,
}

/// System alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub timestamp_ns: i64,
    pub level: AlertLevel,
    pub source: String,
    pub message: String,
    pub metadata: serde_json::Value,
}