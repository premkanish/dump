// crates/universe/src/lib.rs
use common::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::time::{interval, Duration};

pub mod scoring;
pub mod data_sources;

pub use scoring::{CryptoScorer, EquityScorer};
pub use data_sources::*;

/// Universe configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniverseConfig {
    pub crypto_count: usize,
    pub equity_count: usize,
    pub top_selection_count: (usize, usize), // (crypto, equity)
    pub rebuild_interval_mins: u64,
    pub refresh_interval_mins: u64,
    pub min_volume_usd: f64,
    pub min_liquidity_usd: f64,
}

impl Default for UniverseConfig {
    fn default() -> Self {
        Self {
            crypto_count: 30,
            equity_count: 20,
            top_selection_count: (7, 3),
            rebuild_interval_mins: 120,
            refresh_interval_mins: 15,
            min_volume_usd: 1_000_000.0,
            min_liquidity_usd: 500_000.0,
        }
    }
}

/// Universe manager
pub struct UniverseManager {
    config: UniverseConfig,
    crypto_scorer: CryptoScorer,
    equity_scorer: EquityScorer,
    current_universe: parking_lot::RwLock<Vec<UniverseAsset>>,
    data_sources: DataSources,
}

impl UniverseManager {
    pub fn new(config: UniverseConfig, data_sources: DataSources) -> Self {
        Self {
            config,
            crypto_scorer: CryptoScorer::new(),
            equity_scorer: EquityScorer::new(),
            current_universe: parking_lot::RwLock::new(Vec::new()),
            data_sources,
        }
    }
    
    /// Run the universe management loop
    pub async fn run(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) -> Result<()> {
        let mut rebuild_timer = interval(Duration::from_secs(self.config.rebuild_interval_mins * 60));
        let mut refresh_timer = interval(Duration::from_secs(self.config.refresh_interval_mins * 60));
        
        // Initial rebuild
        self.rebuild_master_universe().await?;
        
        loop {
            tokio::select! {
                _ = rebuild_timer.tick() => {
                    if let Err(e) = self.rebuild_master_universe().await {
                        tracing::error!("Failed to rebuild universe: {}", e);
                    }
                }
                _ = refresh_timer.tick() => {
                    if let Err(e) = self.refresh_top_selection().await {
                        tracing::error!("Failed to refresh top selection: {}", e);
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        tracing::info!("Universe manager shutting down");
                        break;
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Rebuild master universe (every 120 minutes)
    async fn rebuild_master_universe(&self) -> Result<()> {
        tracing::info!("Rebuilding master universe");
        
        let start = std::time::Instant::now();
        
        // Collect crypto metrics
        let crypto_metrics = self.collect_crypto_metrics().await?;
        tracing::debug!("Collected {} crypto assets", crypto_metrics.len());
        
        // Collect equity metrics
        let equity_metrics = self.collect_equity_metrics().await?;
        tracing::debug!("Collected {} equity assets", equity_metrics.len());
        
        // Score and filter crypto
        let mut crypto_assets = self.score_crypto(&crypto_metrics)?;
        crypto_assets.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        crypto_assets.truncate(self.config.crypto_count);
        
        // Score and filter equity
        let mut equity_assets = self.score_equity(&equity_metrics)?;
        equity_assets.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        equity_assets.truncate(self.config.equity_count);
        
        // Combine and store
        let mut universe = Vec::new();
        universe.extend(crypto_assets);
        universe.extend(equity_assets);
        
        // Assign ranks
        for (i, asset) in universe.iter_mut().enumerate() {
            asset.rank = i + 1;
        }
        
        *self.current_universe.write() = universe;
        
        let elapsed = start.elapsed();
        tracing::info!("Universe rebuilt in {:?}", elapsed);
        
        metrics::histogram!("universe_rebuild_duration_ms", elapsed.as_millis() as f64);
        
        Ok(())
    }
    
    /// Refresh top selection (every 15 minutes)
    async fn refresh_top_selection(&self) -> Result<()> {
        tracing::debug!("Refreshing top selection");
        
        let start = std::time::Instant::now();
        
        let universe = self.current_universe.read().clone();
        
        // Refresh metrics for current universe
        let crypto_symbols: Vec<String> = universe
            .iter()
            .filter(|a| a.category == AssetCategory::CryptoFutures)
            .map(|a| a.symbol.clone())
            .collect();
        
        let equity_symbols: Vec<String> = universe
            .iter()
            .filter(|a| a.category == AssetCategory::Equity)
            .map(|a| a.symbol.clone())
            .collect();
        
        // Refresh real-time metrics
        let crypto_metrics = self.refresh_crypto_metrics(&crypto_symbols).await?;
        let equity_metrics = self.refresh_equity_metrics(&equity_symbols).await?;
        
        // Rescore
        let mut crypto_assets = self.score_crypto(&crypto_metrics)?;
        crypto_assets.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        
        let mut equity_assets = self.score_equity(&equity_metrics)?;
        equity_assets.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        
        // Apply anti-whiplash: only rotate if score difference > 10%
        let (top_crypto_count, top_equity_count) = self.config.top_selection_count;
        
        crypto_assets.truncate(top_crypto_count);
        equity_assets.truncate(top_equity_count);
        
        let elapsed = start.elapsed();
        tracing::debug!("Top selection refreshed in {:?}", elapsed);
        
        metrics::histogram!("universe_refresh_duration_ms", elapsed.as_millis() as f64);
        
        Ok(())
    }
    
    /// Get current universe
    pub fn get_universe(&self) -> Vec<UniverseAsset> {
        self.current_universe.read().clone()
    }
    
    /// Get top N assets
    pub fn get_top(&self, n: usize) -> Vec<UniverseAsset> {
        let universe = self.current_universe.read();
        universe.iter().take(n).cloned().collect()
    }
    
    async fn collect_crypto_metrics(&self) -> Result<HashMap<String, AssetMetrics>> {
        let mut metrics = HashMap::new();
        
        // Hyperliquid data
        if let Ok(hl_data) = self.data_sources.hyperliquid.fetch_universe().await {
            for item in hl_data {
                metrics.insert(item.symbol, item.metrics);
            }
        }
        
        // DexScreener data (parallel fetch)
        // GeckoTerminal data
        // Birdeye data
        // The Graph data
        // CryptoPanic data
        
        Ok(metrics)
    }
    
    async fn collect_equity_metrics(&self) -> Result<HashMap<String, AssetMetrics>> {
        let mut metrics = HashMap::new();
        
        // IBKR data
        // Yahoo Finance / Alpha Vantage
        // SEC filings
        
        Ok(metrics)
    }
    
    async fn refresh_crypto_metrics(&self, symbols: &[String]) -> Result<HashMap<String, AssetMetrics>> {
        // Fast refresh of key metrics only
        Ok(HashMap::new())
    }
    
    async fn refresh_equity_metrics(&self, symbols: &[String]) -> Result<HashMap<String, AssetMetrics>> {
        // Fast refresh of key metrics only
        Ok(HashMap::new())
    }
    
    fn score_crypto(&self, metrics: &HashMap<String, AssetMetrics>) -> Result<Vec<UniverseAsset>> {
        let mut assets = Vec::new();
        
        for (symbol, metric) in metrics {
            // Apply filters
            if metric.volume_24h_usd < self.config.min_volume_usd {
                continue;
            }
            if metric.liquidity_usd < self.config.min_liquidity_usd {
                continue;
            }
            
            let score = self.crypto_scorer.score(metric);
            
            assets.push(UniverseAsset {
                symbol: symbol.clone(),
                venue: Venue::Hyperliquid, // TODO: determine best venue
                category: AssetCategory::CryptoFutures,
                score,
                rank: 0,
                metrics: metric.clone(),
            });
        }
        
        Ok(assets)
    }
    
    fn score_equity(&self, metrics: &HashMap<String, AssetMetrics>) -> Result<Vec<UniverseAsset>> {
        let mut assets = Vec::new();
        
        for (symbol, metric) in metrics {
            // Apply filters
            if metric.volume_24h_usd < 10_000_000.0 {
                continue;
            }
            if let Some(mcap) = metric.market_cap_usd {
                if mcap < 500_000_000.0 {
                    continue;
                }
            }
            
            let score = self.equity_scorer.score(metric);
            
            assets.push(UniverseAsset {
                symbol: symbol.clone(),
                venue: Venue::IBKR,
                category: AssetCategory::Equity,
                score,
                rank: 0,
                metrics: metric.clone(),
            });
        }
        
        Ok(assets)
    }
}