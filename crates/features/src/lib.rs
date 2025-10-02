// crates/features/src/lib.rs
use common::*;
use ndarray::{Array1, Array2, ArrayView1};
use std::collections::VecDeque;

pub struct FeatureBuilder {
    symbol: String,
    window_size: usize,
    book_history: VecDeque<BookSnapshot>,
    trade_history: VecDeque<Trade>,
}

#[derive(Clone)]
struct BookSnapshot {
    timestamp_ns: i64,
    bid: f64,
    ask: f64,
    bid_size: f64,
    ask_size: f64,
}

impl FeatureBuilder {
    pub fn new(symbol: String, window_size: usize) -> Self {
        Self {
            symbol,
            window_size,
            book_history: VecDeque::with_capacity(window_size),
            trade_history: VecDeque::with_capacity(window_size * 10),
        }
    }
    
    pub fn update_book(&mut self, orderbook: &OrderBook) {
        if let (Some(bid), Some(ask)) = (orderbook.best_bid(), orderbook.best_ask()) {
            let snapshot = BookSnapshot {
                timestamp_ns: orderbook.timestamp_ns,
                bid: bid.price.0,
                ask: ask.price.0,
                bid_size: bid.quantity,
                ask_size: ask.quantity,
            };
            
            self.book_history.push_back(snapshot);
            if self.book_history.len() > self.window_size {
                self.book_history.pop_front();
            }
        }
    }
    
    pub fn update_trades(&mut self, trades: &[Trade]) {
        for trade in trades {
            self.trade_history.push_back(trade.clone());
        }
        
        // Keep only recent trades (last 60 seconds)
        if let Some(latest) = self.trade_history.back() {
            let cutoff = latest.timestamp_ns - 60_000_000_000;
            while let Some(front) = self.trade_history.front() {
                if front.timestamp_ns < cutoff {
                    self.trade_history.pop_front();
                } else {
                    break;
                }
            }
        }
    }
    
    pub fn compute(&self, funding_bps: f64, impact_curve: (f64, f64)) -> Option<FeatureVec> {
        if self.book_history.len() < 2 {
            return None;
        }
        
        let latest = self.book_history.back()?;
        let mid = (latest.bid + latest.ask) / 2.0;
        let spread_bps = (latest.ask - latest.bid) / mid * 10000.0;
        
        // Order Flow Imbalance (OFI)
        let ofi_1s = self.compute_ofi(1_000_000_000);
        
        // Order Book Imbalance (OBI)
        let obi_1s = (latest.bid_size - latest.ask_size) / (latest.bid_size + latest.ask_size + 1e-9);
        
        // Depth imbalance
        let depth_imbalance = self.compute_depth_imbalance();
        
        // Realized volatility
        let realized_vol_5s = self.compute_realized_vol(5_000_000_000);
        
        // ATR (Average True Range)
        let atr_30s = self.compute_atr(30_000_000_000);
        
        // Microprice (weighted mid based on sizes)
        let microprice = (latest.bid * latest.ask_size + latest.ask * latest.bid_size) 
            / (latest.bid_size + latest.ask_size + 1e-9);
        
        // VWAP ratio
        let vwap_ratio = self.compute_vwap_ratio(mid);
        
        Some(FeatureVec {
            timestamp_ns: latest.timestamp_ns,
            symbol: self.symbol.clone(),
            mid_price: mid,
            spread_bps,
            ofi_1s,
            obi_1s,
            depth_imbalance,
            depth_a: impact_curve.0,
            depth_beta: impact_curve.1,
            realized_vol_5s,
            atr_30s,
            funding_bps_8h: funding_bps,
            impact_bps_1pct: impact_curve.0 * 0.01f64.powf(impact_curve.1) * 10000.0,
            microprice,
            vwap_ratio,
        })
    }
    
    fn compute_ofi(&self, window_ns: i64) -> f64 {
        if self.book_history.len() < 2 {
            return 0.0;
        }
        
        let latest = self.book_history.back().unwrap();
        let cutoff = latest.timestamp_ns - window_ns;
        
        let mut ofi = 0.0;
        let mut prev: Option<&BookSnapshot> = None;
        
        for snap in self.book_history.iter().rev() {
            if snap.timestamp_ns < cutoff {
                break;
            }
            
            if let Some(p) = prev {
                let bid_flow = if snap.bid >= p.bid {
                    snap.bid_size
                } else if snap.bid < p.bid {
                    -p.bid_size
                } else {
                    snap.bid_size - p.bid_size
                };
                
                let ask_flow = if snap.ask <= p.ask {
                    snap.ask_size
                } else if snap.ask > p.ask {
                    -p.ask_size
                } else {
                    snap.ask_size - p.ask_size
                };
                
                ofi += bid_flow - ask_flow;
            }
            
            prev = Some(snap);
        }
        
        ofi
    }
    
    fn compute_depth_imbalance(&self) -> f64 {
        if let Some(latest) = self.book_history.back() {
            let total = latest.bid_size + latest.ask_size + 1e-9;
            (latest.bid_size - latest.ask_size) / total
        } else {
            0.0
        }
    }
    
    fn compute_realized_vol(&self, window_ns: i64) -> f64 {
        if self.book_history.len() < 2 {
            return 0.0;
        }
        
        let latest = self.book_history.back().unwrap();
        let cutoff = latest.timestamp_ns - window_ns;
        
        let prices: Vec<f64> = self.book_history
            .iter()
            .rev()
            .take_while(|s| s.timestamp_ns >= cutoff)
            .map(|s| (s.bid + s.ask) / 2.0)
            .collect();
        
        if prices.len() < 2 {
            return 0.0;
        }
        
        let returns: Vec<f64> = prices
            .windows(2)
            .map(|w| (w[0] / w[1]).ln())
            .collect();
        
        if returns.is_empty() {
            return 0.0;
        }
        
        let mean = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance = returns.iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>() / returns.len() as f64;
        
        variance.sqrt() * (252.0 * 86400.0 / (window_ns as f64 / 1e9)).sqrt()
    }
    
    fn compute_atr(&self, window_ns: i64) -> f64 {
        if self.book_history.len() < 2 {
            return 0.0;
        }
        
        let latest = self.book_history.back().unwrap();
        let cutoff = latest.timestamp_ns - window_ns;
        
        let ranges: Vec<f64> = self.book_history
            .iter()
            .rev()
            .take_while(|s| s.timestamp_ns >= cutoff)
            .map(|s| s.ask - s.bid)
            .collect();
        
        if ranges.is_empty() {
            return 0.0;
        }
        
        ranges.iter().sum::<f64>() / ranges.len() as f64
    }
    
    fn compute_vwap_ratio(&self, current_mid: f64) -> f64 {
        if self.trade_history.is_empty() {
            return 1.0;
        }
        
        let mut total_volume = 0.0;
        let mut total_pv = 0.0;
        
        for trade in self.trade_history.iter().rev().take(100) {
            total_volume += trade.quantity;
            total_pv += trade.price * trade.quantity;
        }
        
        if total_volume < 1e-9 {
            return 1.0;
        }
        
        let vwap = total_pv / total_volume;
        current_mid / vwap
    }
    
    /// Export features as ndarray for ML models
    pub fn to_array(&self, feature: &FeatureVec) -> Array1<f32> {
        Array1::from_vec(vec![
            feature.mid_price as f32,
            feature.spread_bps as f32,
            feature.ofi_1s as f32,
            feature.obi_1s as f32,
            feature.depth_imbalance as f32,
            feature.depth_a as f32,
            feature.depth_beta as f32,
            feature.realized_vol_5s as f32,
            feature.atr_30s as f32,
            feature.funding_bps_8h as f32,
            feature.impact_bps_1pct as f32,
            feature.microprice as f32,
            feature.vwap_ratio as f32,
        ])
    }
}

/// Batch feature computation for multiple symbols
pub struct BatchFeatureBuilder {
    builders: std::collections::HashMap<String, FeatureBuilder>,
}

impl BatchFeatureBuilder {
    pub fn new() -> Self {
        Self {
            builders: std::collections::HashMap::new(),
        }
    }
    
    pub fn add_symbol(&mut self, symbol: String, window_size: usize) {
        self.builders.insert(symbol.clone(), FeatureBuilder::new(symbol, window_size));
    }
    
    pub fn update_book(&mut self, orderbook: &OrderBook) {
        if let Some(builder) = self.builders.get_mut(&orderbook.symbol) {
            builder.update_book(orderbook);
        }
    }
    
    pub fn update_trades(&mut self, symbol: &str, trades: &[Trade]) {
        if let Some(builder) = self.builders.get_mut(symbol) {
            builder.update_trades(trades);
        }
    }
    
    pub fn compute_all(&self, 
        funding_rates: &std::collections::HashMap<String, f64>,
        impact_curves: &std::collections::HashMap<String, (f64, f64)>,
    ) -> Vec<FeatureVec> {
        self.builders
            .iter()
            .filter_map(|(symbol, builder)| {
                let funding = funding_rates.get(symbol).copied().unwrap_or(0.0);
                let impact = impact_curves.get(symbol).copied().unwrap_or((0.001, 0.5));
                builder.compute(funding, impact)
            })
            .collect()
    }
}

/// Technical indicators
pub mod indicators {
    use super::*;
    
    pub fn ema(values: &[f64], period: usize) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        
        let alpha = 2.0 / (period as f64 + 1.0);
        let mut ema = values[0];
        
        for &value in values.iter().skip(1) {
            ema = alpha * value + (1.0 - alpha) * ema;
        }
        
        ema
    }
    
    pub fn rsi(prices: &[f64], period: usize) -> f64 {
        if prices.len() < period + 1 {
            return 50.0;
        }
        
        let mut gains = 0.0;
        let mut losses = 0.0;
        
        for i in 1..=period {
            let change = prices[i] - prices[i - 1];
            if change > 0.0 {
                gains += change;
            } else {
                losses += change.abs();
            }
        }
        
        let avg_gain = gains / period as f64;
        let avg_loss = losses / period as f64;
        
        if avg_loss < 1e-9 {
            return 100.0;
        }
        
        let rs = avg_gain / avg_loss;
        100.0 - (100.0 / (1.0 + rs))
    }
    
    pub fn bollinger_bands(prices: &[f64], period: usize, std_dev: f64) -> (f64, f64, f64) {
        if prices.len() < period {
            return (0.0, 0.0, 0.0);
        }
        
        let recent = &prices[prices.len() - period..];
        let mean = recent.iter().sum::<f64>() / period as f64;
        
        let variance = recent.iter()
            .map(|&p| (p - mean).powi(2))
            .sum::<f64>() / period as f64;
        
        let std = variance.sqrt();
        
        (mean - std_dev * std, mean, mean + std_dev * std)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordered_float::OrderedFloat;
    
    #[test]
    fn test_feature_builder() {
        let mut builder = FeatureBuilder::new("BTC".to_string(), 100);
        
        let book = OrderBook {
            symbol: "BTC".to_string(),
            timestamp_ns: 1000000000,
            bids: vec![Level { price: OrderedFloat(50000.0), quantity: 1.0 }],
            asks: vec![Level { price: OrderedFloat(50010.0), quantity: 1.0 }],
            sequence: 1,
        };
        
        builder.update_book(&book);
        builder.update_book(&book);
        
        let features = builder.compute(0.01, (0.001, 0.5));
        assert!(features.is_some());
        
        let f = features.unwrap();
        assert_eq!(f.symbol, "BTC");
        assert!(f.mid_price > 0.0);
        assert!(f.spread_bps > 0.0);
    }
    
    #[test]
    fn test_indicators() {
        let prices = vec![100.0, 102.0, 101.0, 103.0, 104.0];
        
        let ema = indicators::ema(&prices, 3);
        assert!(ema > 0.0);
        
        let rsi = indicators::rsi(&prices, 3);
        assert!(rsi >= 0.0 && rsi <= 100.0);
        
        let (lower, mid, upper) = indicators::bollinger_bands(&prices, 3, 2.0);
        assert!(lower < mid);
        assert!(mid < upper);
    }
}