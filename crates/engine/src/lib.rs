// crates/engine/src/lib.rs
pub mod inference;
pub mod router;
pub mod ws_server;
pub mod s3_writer;

use common::*;
use features::BatchFeatureBuilder;
use inference::{InferencePool, ModelType, RuleBasedPredictor};
use router::{OrderRouter, GateParams, CostModel};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use parking_lot::RwLock;

/// Trading engine configuration
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub mode: TradingMode,
    pub feature_window_size: usize,
    pub inference_timeout_ms: u64,
    pub gate_params: GateParams,
    pub enable_s3_writer: bool,
    pub s3_bucket: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            mode: TradingMode::Paper,
            feature_window_size: 1000,
            inference_timeout_ms: 3,
            gate_params: GateParams::default(),
            enable_s3_writer: false,
            s3_bucket: None,
        }
    }
}

/// Main trading engine
pub struct TradingEngine {
    config: Arc<RwLock<EngineConfig>>,
    feature_builder: Arc<RwLock<BatchFeatureBuilder>>,
    inference_pool: Arc<InferencePool>,
    router: Arc<OrderRouter>,
    adapters: Arc<RwLock<HashMap<String, Arc<dyn adapters::ExchangeAdapter>>>>,
    snapshot_tx: mpsc::UnboundedSender<MarketSnapshot>,
    metrics_tx: watch::Sender<PerformanceMetrics>,
}

impl TradingEngine {
    pub fn new(
        config: EngineConfig,
        risk_limits: RiskLimits,
    ) -> Result<Self> {
        let inference_pool = Arc::new(InferencePool::new(config.inference_timeout_ms)?);
        let router = Arc::new(OrderRouter::new(config.gate_params.clone(), risk_limits));
        let feature_builder = Arc::new(RwLock::new(BatchFeatureBuilder::new()));
        
        let (snapshot_tx, _) = mpsc::unbounded_channel();
        let (metrics_tx, _) = watch::channel(PerformanceMetrics::default());
        
        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            feature_builder,
            inference_pool,
            router,
            adapters: Arc::new(RwLock::new(HashMap::new())),
            snapshot_tx,
            metrics_tx,
        })
    }
    
    /// Load ML models
    pub fn load_models(&self, crypto_dir: &str, equity_dir: &str) -> Result<()> {
        if std::path::Path::new(crypto_dir).exists() {
            self.inference_pool.load_crypto(std::path::Path::new(crypto_dir))?;
        }
        
        if std::path::Path::new(equity_dir).exists() {
            self.inference_pool.load_equity(std::path::Path::new(equity_dir))?;
        }
        
        Ok(())
    }
    
    /// Add exchange adapter
    pub fn add_adapter(&self, label: String, adapter: Arc<dyn adapters::ExchangeAdapter>) {
        self.adapters.write().insert(label, adapter);
    }
    
    /// Add symbol to track
    pub fn add_symbol(&self, symbol: String, window_size: usize) {
        self.feature_builder.write().add_symbol(symbol, window_size);
    }
    
    /// Run the trading engine
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        tracing::info!("Trading engine starting");
        
        let (market_tx, mut market_rx) = mpsc::unbounded_channel::<MarketSnapshot>();
        
        // Spawn market data processing loop
        let engine_clone = self.clone_for_processing();
        let processing_handle = tokio::spawn(async move {
            engine_clone.process_market_data_loop(market_rx).await
        });
        
        // Wait for shutdown
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        tracing::info!("Trading engine shutting down");
                        break;
                    }
                }
            }
        }
        
        processing_handle.abort();
        Ok(())
    }
    
    fn clone_for_processing(&self) -> Self {
        Self {
            config: self.config.clone(),
            feature_builder: self.feature_builder.clone(),
            inference_pool: self.inference_pool.clone(),
            router: self.router.clone(),
            adapters: self.adapters.clone(),
            snapshot_tx: self.snapshot_tx.clone(),
            metrics_tx: self.metrics_tx.clone(),
        }
    }
    
    /// Process market data loop
    async fn process_market_data_loop(&self, mut market_rx: mpsc::UnboundedReceiver<MarketSnapshot>) {
        let mut frame_count = 0u64;
        let mut last_metrics_update = std::time::Instant::now();
        let mut perf = PerformanceMetrics::default();
        
        while let Some(snapshot) = market_rx.recv().await {
            let frame_start = std::time::Instant::now();
            
            // Update features
            let ingest_start = std::time::Instant::now();
            self.feature_builder.write().update_book(&snapshot.orderbook);
            let ingest_elapsed = ingest_start.elapsed().as_micros() as f64;
            
            // Compute features
            let feature_start = std::time::Instant::now();
            let funding_rates = HashMap::new(); // TODO: populate
            let impact_curves = HashMap::new();  // TODO: populate
            let features = self.feature_builder.read().compute_all(&funding_rates, &impact_curves);
            let feature_elapsed = feature_start.elapsed().as_micros() as f64;
            
            // Process each feature
            for feature_vec in features {
                if let Err(e) = self.process_signal(&feature_vec, &mut perf).await {
                    tracing::warn!("Failed to process signal for {}: {}", feature_vec.symbol, e);
                }
            }
            
            frame_count += 1;
            
            // Update metrics
            if last_metrics_update.elapsed().as_secs() >= 1 {
                perf.ingest_p50_us = ingest_elapsed;
                perf.feature_p50_us = feature_elapsed;
                perf.snapshots_per_sec = frame_count as f64 / last_metrics_update.elapsed().as_secs_f64();
                
                let _ = self.metrics_tx.send(perf.clone());
                
                last_metrics_update = std::time::Instant::now();
                frame_count = 0;
            }
            
            // Backpressure: measure loop time
            let frame_elapsed = frame_start.elapsed();
            if frame_elapsed.as_millis() > 100 {
                tracing::warn!("Slow frame: {:?}", frame_elapsed);
                metrics::increment_counter!("slow_frames");
            }
        }
    }
    
    /// Process individual signal
    async fn process_signal(
        &self,
        features: &FeatureVec,
        perf: &mut PerformanceMetrics,
    ) -> Result<()> {
        let config = self.config.read();
        
        if config.mode == TradingMode::Paused {
            return Ok(());
        }
        
        // Convert to array for ML
        let feature_builder = self.feature_builder.read();
        let feature_array = feature_builder.builders.get(&features.symbol)
            .map(|b| b.to_array(features))
            .ok_or_else(|| Error::Internal("Feature builder not found".to_string()))?;
        
        // Run inference with timeout and fallback
        let model_start = std::time::Instant::now();
        let category = AssetCategory::CryptoFutures; // TODO: determine from symbol
        
        let mut prediction = match self.inference_pool
            .predict(category, &feature_array, ModelType::Edge)
            .await
        {
            Ok(mut pred) => {
                pred.symbol = features.symbol.clone();
                pred
            }
            Err(e) => {
                tracing::debug!("Model inference failed, using rule-based: {}", e);
                metrics::increment_counter!("fallback_predictions");
                perf.model_timeouts += 1;
                RuleBasedPredictor::predict(features)
            }
        };
        
        let model_elapsed = model_start.elapsed().as_micros() as f64;
        perf.model_p50_us = model_elapsed;
        
        // Cost model (TODO: get from adapter)
        let costs = CostModel {
            taker_fee_bps: 5.0,
            maker_fee_bps: 2.0,
            maker_rebate_bps: 1.0,
            impact_bps: features.impact_bps_1pct,
            slippage_buffer_bps: 1.0,
        };
        
        // Route decision
        let route_start = std::time::Instant::now();
        let decision = self.router.decide(&prediction, features, &costs);
        let route_elapsed = route_start.elapsed().as_micros() as f64;
        perf.route_p50_us = route_elapsed;
        
        if !decision.should_trade {
            tracing::debug!("No trade for {}: {}", features.symbol, decision.reason);
            return Ok(());
        }
        
        // Execute trade
        if config.mode == TradingMode::Live {
            self.execute_trade(&features.symbol, &decision, features).await?;
        } else {
            tracing::info!(
                "Paper trade for {}: style={:?}, size={:.4}, edge={:.2}bps",
                features.symbol,
                decision.style,
                decision.size_fraction,
                prediction.edge_bps
            );
        }
        
        Ok(())
    }
    
    /// Execute actual trade
    async fn execute_trade(
        &self,
        symbol: &str,
        decision: &RouteDecision,
        features: &FeatureVec,
    ) -> Result<()> {
        // Find appropriate adapter
        let adapters = self.adapters.read();
        let adapter = adapters.values().next()
            .ok_or_else(|| Error::Internal("No adapter available".to_string()))?;
        
        // Check risk limits
        let risk_manager = self.router.get_risk_manager();
        let notional = features.mid_price * decision.size_fraction;
        risk_manager.read().check_limits(symbol, notional)?;
        
        // Determine side (TODO: based on prediction direction)
        let side = if features.ofi_1s > 0.0 { Side::Buy } else { Side::Sell };
        
        // Create order
        let order = OrderRequest {
            client_id: format!("{}_{}", symbol, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            symbol: symbol.to_string(),
            side,
            order_type: match decision.style {
                OrderStyle::TakerNow => OrderType::Market,
                OrderStyle::MakerPassive => OrderType::PostOnly,
                OrderStyle::Sniper => OrderType::Limit,
            },
            quantity: decision.size_fraction,
            price: if decision.style == OrderStyle::Sniper {
                Some(features.mid_price)
            } else {
                None
            },
            reduce_only: false,
            time_in_force: TimeInForce::GTC,
        };
        
        // Send order
        match adapter.send_order(order).await {
            Ok(ack) => {
                tracing::info!("Order sent: {} - {:?}", symbol, ack.status);
                metrics::increment_counter!("orders_sent", "symbol" => symbol.to_string());
            }
            Err(e) => {
                tracing::error!("Order failed: {}", e);
                metrics::increment_counter!("order_rejects", "symbol" => symbol.to_string());
                return Err(e);
            }
        }
        
        Ok(())
    }
    
    /// Change trading mode
    pub fn set_mode(&self, mode: TradingMode) {
        self.config.write().mode = mode;
        tracing::info!("Trading mode changed to {:?}", mode);
    }
    
    /// Get current mode
    pub fn get_mode(&self) -> TradingMode {
        self.config.read().mode
    }
    
    /// Get performance metrics
    pub fn get_metrics(&self) -> PerformanceMetrics {
        self.metrics_tx.borrow().clone()
    }
}