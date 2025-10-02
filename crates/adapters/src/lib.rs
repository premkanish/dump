// crates/adapters/src/lib.rs
use async_trait::async_trait;
use common::*;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub mod hyperliquid;
pub mod binance;
pub mod ibkr;
mod rate_limiter;

pub use hyperliquid::HyperliquidAdapter;
pub use binance::BinanceAdapter;
pub use ibkr::IbkrAdapter;
pub use rate_limiter::RateLimiter;

/// Market data stream interface
#[async_trait]
pub trait MarketDataStream: Send + Sync {
    /// Subscribe to order book updates
    async fn subscribe_orderbook(&mut self, symbols: &[String]) -> Result<()>;
    
    /// Subscribe to trade stream
    async fn subscribe_trades(&mut self, symbols: &[String]) -> Result<()>;
    
    /// Get the receiver for market snapshots
    fn snapshot_receiver(&self) -> mpsc::UnboundedReceiver<MarketSnapshot>;
}

/// Account data interface
#[async_trait]
pub trait AccountData: Send + Sync {
    /// Get current balances
    async fn balances(&self) -> Result<HashMap<String, Balance>>;
    
    /// Get current positions
    async fn positions(&self) -> Result<Vec<Position>>;
    
    /// Get fee tier information
    async fn fee_tier(&self) -> Result<FeeTier>;
    
    /// Get account leverage
    async fn leverage(&self) -> Result<f64>;
}

/// Order execution interface
#[async_trait]
pub trait OrderRouter: Send + Sync {
    /// Submit an order
    async fn send_order(&self, order: OrderRequest) -> Result<OrderAck>;
    
    /// Cancel an order
    async fn cancel_order(&self, order_id: &str) -> Result<()>;
    
    /// Cancel all orders for a symbol
    async fn cancel_all(&self, symbol: &str) -> Result<()>;
    
    /// Get order status
    async fn get_order(&self, order_id: &str) -> Result<OrderAck>;
}

/// Market info interface
#[async_trait]
pub trait MarketInfo: Send + Sync {
    /// Get list of available symbols
    async fn list_symbols(&self) -> Result<Vec<String>>;
    
    /// Search symbols by prefix
    async fn search_symbols(&self, prefix: &str) -> Result<Vec<String>>;
    
    /// Get funding rate (for futures)
    async fn funding_rate(&self, symbol: &str) -> Result<f64>;
    
    /// Get open interest (for futures)
    async fn open_interest(&self, symbol: &str) -> Result<f64>;
    
    /// Get 24h volume
    async fn volume_24h(&self, symbol: &str) -> Result<f64>;
}

/// Complete exchange adapter
#[async_trait]
pub trait ExchangeAdapter: 
    MarketDataStream + AccountData + OrderRouter + MarketInfo + Send + Sync 
{
    fn venue(&self) -> Venue;
    fn is_connected(&self) -> bool;
    async fn connect(&mut self) -> Result<()>;
    async fn disconnect(&mut self) -> Result<()>;
}

/// Impact curve parameters (A * notional^beta)
#[derive(Debug, Clone, Copy)]
pub struct ImpactCurve {
    pub a: f64,
    pub beta: f64,
}

impl ImpactCurve {
    pub fn compute_bps(&self, notional: f64) -> f64 {
        self.a * notional.powf(self.beta) * 10000.0
    }
}

/// Order book delta
#[derive(Debug, Clone)]
pub enum BookDelta {
    Insert { side: Side, price: f64, quantity: f64 },
    Update { side: Side, price: f64, quantity: f64 },
    Delete { side: Side, price: f64 },
    Clear,
}

/// Maintains an order book from deltas
pub struct OrderBookMaintainer {
    pub symbol: String,
    pub bids: std::collections::BTreeMap<ordered_float::OrderedFloat<f64>, f64>,
    pub asks: std::collections::BTreeMap<ordered_float::OrderedFloat<f64>, f64>,
    pub sequence: u64,
}

impl OrderBookMaintainer {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            bids: std::collections::BTreeMap::new(),
            asks: std::collections::BTreeMap::new(),
            sequence: 0,
        }
    }
    
    pub fn apply_delta(&mut self, delta: BookDelta) {
        use ordered_float::OrderedFloat;
        
        match delta {
            BookDelta::Insert { side, price, quantity } | 
            BookDelta::Update { side, price, quantity } => {
                let book = match side {
                    Side::Buy => &mut self.bids,
                    Side::Sell => &mut self.asks,
                };
                if quantity > 0.0 {
                    book.insert(OrderedFloat(price), quantity);
                } else {
                    book.remove(&OrderedFloat(price));
                }
            }
            BookDelta::Delete { side, price } => {
                let book = match side {
                    Side::Buy => &mut self.bids,
                    Side::Sell => &mut self.asks,
                };
                book.remove(&ordered_float::OrderedFloat(price));
            }
            BookDelta::Clear => {
                self.bids.clear();
                self.asks.clear();
            }
        }
        self.sequence += 1;
    }
    
    pub fn to_orderbook(&self, timestamp_ns: i64, depth: usize) -> OrderBook {
        use ordered_float::OrderedFloat;
        
        let bids: Vec<Level> = self.bids
            .iter()
            .rev()
            .take(depth)
            .map(|(p, q)| Level { price: *p, quantity: *q })
            .collect();
        
        let asks: Vec<Level> = self.asks
            .iter()
            .take(depth)
            .map(|(p, q)| Level { price: *p, quantity: *q })
            .collect();
        
        OrderBook {
            symbol: self.symbol.clone(),
            timestamp_ns,
            bids,
            asks,
            sequence: self.sequence,
        }
    }
}