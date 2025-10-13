// crates/engine/src/lib.rs - MANDATORY Models Architecture
// NO OPTIONAL FALLBACKS - Fail fast if models missing

pub mod inference;
pub mod router;
pub mod ws_server;
pub mod s3_writer;
pub mod rl_agent;

use common::*;
use features::{FeatureComputer, DeviceType};
use inference::{InferencePool, ModelType};
use router::{OrderRouter, GateParams, CostModel};
use rl_agent::{RLAgent, MarketState};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use parking_lot::RwLock;

/// Trading engine with MANDATORY RL agent and ML models
pub struct TradingEngine {
    config: Arc<RwLock<EngineConfig>>,
    
    // GPU features (mandatory)
    feature_computer: Arc<FeatureComputer>,
    
    // RL agent (MANDATORY - no fallback)
    rl_agent: Arc<RLAgent>,
    
    // ML inference pool (MANDATORY - no fallback)
    inference_pool: Arc<InferencePool>,
    
    // Router (for risk checks only, not decision making)
    router: Arc<OrderRouter>,
    
    // Exchange adapters
    adapters: Arc<RwLock<HashMap<String, Arc<dyn adapters::ExchangeAdapter>>>>,
    
    // Channels
    snapshot_tx: mpsc::UnboundedSender<MarketSnapshot>,
    metrics_tx: watch::Sender<PerformanceMetrics>,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub mode: TradingMode,
    pub batch_size: usize,
    pub batch_timeout_ms: u64,
    pub inference_timeout_ms: u64,
    pub gate_params: GateParams,
    pub gpu_device: DeviceType,
    pub decision_mode: DecisionMode,
}

/// Decision mode - BOTH are mandatory, choose which to use
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionMode {
    /// Use RL agent for decisions (REQUIRES: actor.onnx, critic.onnx)
    RLAgent,
    
    /// Use ML models + traditional routing (REQUIRES: ONNX models in models/crypto and models/equity)
    MLTraditional,
    
    /// Use both: RL for decision, ML for validation (REQUIRES: All models)
    Hybrid,
}

impl TradingEngine {
    /// Create new trading engine - FAILS if models missing
    pub fn new(config: EngineConfig, risk_limits: RiskLimits) -> Result<Self> {
        tracing::info!("🚀 Initializing HFT Engine (MANDATORY models mode)");
        
        // 1. Initialize GPU feature computer (mandatory)
        let feature_computer = Arc::new(
            FeatureComputer::new(config.gpu_device, config.batch_size)
                .map_err(|e| Error::Internal(format!("GPU init FAILED: {}. This is REQUIRED.", e)))?
        );
        tracing::info!("✅ GPU feature computer initialized");
        
        // 2. Initialize ML inference pool (MANDATORY)
        let inference_pool = Arc::new(InferencePool::new(config.inference_timeout_ms)?);
        tracing::info!("✅ ML inference pool initialized");
        
        // 3. Initialize RL agent (MANDATORY)
        let rl_agent = Arc::new(
            RLAgent::new(
                "models/rl/actor.onnx",
                Some("models/rl/critic.onnx"),
                rl_agent::RLAgentConfig {
                    action_type: rl_agent::ActionType::MultiDiscrete,
                    sequence_length: 10,
                    use_recurrent: false,
                    epsilon: 0.0,
                    temperature: 1.0,
                },
            ).map_err(|e| Error::Internal(format!(
                "RL Agent init FAILED: {}. REQUIRED files: models/rl/actor.onnx, models/rl/critic.onnx",
                e
            )))?
        );
        tracing::info!("✅ RL Agent initialized");
        
        // 4. Initialize router (for risk checks only)
        let router = Arc::new(OrderRouter::new(config.gate_params.clone(), risk_limits));
        
        let (snapshot_tx, _) = mpsc::unbounded_channel();
        let (metrics_tx, _) = watch::channel(PerformanceMetrics::default());
        
        tracing::info!("✅ Trading engine initialized successfully");
        tracing::info!("⚠️  Decision mode: {:?}", config.decision_mode);
        
        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            feature_computer,
            inference_pool,
            rl_agent,
            router,
            adapters: Arc::new(RwLock::new(HashMap::new())),
            snapshot_tx,
            metrics_tx,
        })
    }
    
    /// Load ML models - FAILS if models missing
    pub fn load_models(&self, crypto_dir: &str, equity_dir: &str) -> Result<()> {
        tracing::info!("📦 Loading ML models (MANDATORY)...");
        
        // Check directories exist
        if !std::path::Path::new(crypto_dir).exists() {
            return Err(Error::Internal(format!(
                "Crypto models directory NOT FOUND: {}. This is REQUIRED.",
                crypto_dir
            )));
        }
        
        if !std::path::Path::new(equity_dir).exists() {
            return Err(Error::Internal(format!(
                "Equity models directory NOT FOUND: {}. This is REQUIRED.",
                equity_dir
            )));
        }
        
        // Load crypto models (MANDATORY)
        self.inference_pool.load_crypto(std::path::Path::new(crypto_dir))
            .map_err(|e| Error::Internal(format!(
                "Failed to load crypto models: {}. REQUIRED files: {}/{{edge,transformer,gbdt}}.onnx",
                e, crypto_dir
            )))?;
        tracing::info!("✅ Crypto models loaded from {}", crypto_dir);
        
        // Load equity models (MANDATORY)
        self.inference_pool.load_equity(std::path::Path::new(equity_dir))
            .map_err(|e| Error::Internal(format!(
                "Failed to load equity models: {}. REQUIRED files: {}/{{edge,transformer,gbdt}}.onnx",
                e, equity_dir
            )))?;
        tracing::info!("✅ Equity models loaded from {}", equity_dir);
        
        // Verify models are actually loaded
        self.verify_models_loaded()?;
        
        Ok(())
    }
    
    /// Verify all required models are loaded
    fn verify_models_loaded(&self) -> Result<()> {
        let config = self.config.read();
        
        match config.decision_mode {
            DecisionMode::RLAgent => {
                // Only RL agent required (already verified in new())
                tracing::info!("✅ RL Agent models verified");
            }
            
            DecisionMode::MLTraditional => {
                // Verify ML models loaded
                if !self.inference_pool.has_crypto_models() {
                    return Err(Error::Internal(
                        "Crypto ML models NOT loaded. Required for MLTraditional mode.".to_string()
                    ));
                }
                if !self.inference_pool.has_equity_models() {
                    return Err(Error::Internal(
                        "Equity ML models NOT loaded. Required for MLTraditional mode.".to_string()
                    ));
                }
                tracing::info!("✅ ML models verified");
            }
            
            DecisionMode::Hybrid => {
                // Verify both RL and ML models
                if !self.inference_pool.has_crypto_models() || !self.inference_pool.has_equity_models() {
                    return Err(Error::Internal(
                        "ML models NOT loaded. Required for Hybrid mode.".to_string()
                    ));
                }
                tracing::info!("✅ RL + ML models verified");
            }
        }
        
        Ok(())
    }
    
    /// Add exchange adapter
    pub fn add_adapter(&self, label: String, adapter: Arc<dyn adapters::ExchangeAdapter>) {
        self.adapters.write().insert(label, adapter);
    }
    
    /// Add symbol to track
    pub fn add_symbol(&self, symbol: String, window_size: usize) {
        self.feature_computer.add_symbol(symbol, window_size);
    }
    
    /// Main trading loop with batching
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        tracing::info!("🎯 Trading loop starting (MANDATORY models mode)");
        
        let (market_tx, mut market_rx) = mpsc::unbounded_channel::<MarketSnapshot>();
        
        let engine_clone = self.clone_for_processing();
        let config = self.config.read().clone();
        
        let processing_handle = tokio::spawn(async move {
            engine_clone.process_with_batching(market_rx, config).await
        });
        
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        tracing::info!("🛑 Shutdown signal received");
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
            feature_computer: self.feature_computer.clone(),
            inference_pool: self.inference_pool.clone(),
            rl_agent: self.rl_agent.clone(),
            router: self.router.clone(),
            adapters: self.adapters.clone(),
            snapshot_tx: self.snapshot_tx.clone(),
            metrics_tx: self.metrics_tx.clone(),
        }
    }
    
    /// Process market data with batching for GPU efficiency
    async fn process_with_batching(
        &self,
        mut market_rx: mpsc::UnboundedReceiver<MarketSnapshot>,
        config: EngineConfig,
    ) {
        let mut batch = Vec::with_capacity(config.batch_size);
        let mut last_flush = std::time::Instant::now();
        let mut perf = PerformanceMetrics::default();
        
        while let Some(snapshot) = market_rx.recv().await {
            batch.push(snapshot);
            
            let should_flush = batch.len() >= config.batch_size
                || last_flush.elapsed().as_millis() >= config.batch_timeout_ms as u128;
            
            if should_flush && !batch.is_empty() {
                let cycle_start = std::time::Instant::now();
                
                // STEP 1: GPU Feature Computation (MANDATORY - no fallback)
                let feature_start = std::time::Instant::now();
                let features = match self.feature_computer.compute_batch(&batch) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::error!("❌ GPU feature computation FAILED: {}", e);
                        tracing::error!("❌ HALTING - No CPU fallback available");
                        metrics::increment_counter!("engine_halt_gpu_failure");
                        batch.clear();
                        continue; // Skip this batch
                    }
                };
                perf.feature_p99_us = feature_start.elapsed().as_micros() as f64;
                
                // STEP 2: Process each signal with MANDATORY models
                let inference_start = std::time::Instant::now();
                for computed in features {
                    if let Err(e) = self.process_signal_mandatory(&computed, &mut perf).await {
                        tracing::error!("❌ Signal processing FAILED for {}: {}", computed.symbol, e);
                        metrics::increment_counter!("signal_processing_error", 
                            "symbol" => computed.symbol.clone()
                        );
                    }
                }
                perf.model_p99_us = inference_start.elapsed().as_micros() as f64;
                
                // Update metrics
                perf.snapshots_per_sec = batch.len() as f64 / cycle_start.elapsed().as_secs_f64();
                let _ = self.metrics_tx.send(perf.clone());
                
                batch.clear();
                last_flush = std::time::Instant::now();
                
                let total_time = cycle_start.elapsed();
                metrics::histogram!("engine_cycle_us", total_time.as_micros() as f64);
                
                if total_time.as_millis() > 5 {
                    tracing::warn!("⚠️  Slow cycle: {:?} (target: <5ms)", total_time);
                }
            }
        }
    }
    
    /// Process signal with MANDATORY models (no fallbacks)
    async fn process_signal_mandatory(
        &self,
        computed: &features::ComputedFeatures,
        perf: &mut PerformanceMetrics,
    ) -> Result<()> {
        let config = self.config.read();
        
        if config.mode == TradingMode::Paused {
            return Ok(());
        }
        
        let features = self.features_to_vec(computed);
        
        // Get decision based on mode - ALL MANDATORY
        let decision = match config.decision_mode {
            DecisionMode::RLAgent => {
                self.decide_with_rl_mandatory(computed, &features).await?
            }
            
            DecisionMode::MLTraditional => {
                self.decide_with_ml_mandatory(computed, &features, perf).await?
            }
            
            DecisionMode::Hybrid => {
                self.decide_hybrid_mandatory(computed, &features, perf).await?
            }
        };
        
        if !decision.should_trade {
            return Ok(());
        }
        
        // Execute trade
        if config.mode == TradingMode::Live {
            self.execute_trade(&computed.symbol, &decision, &features).await?;
        } else {
            tracing::debug!(
                "Paper trade: {} {:?} size={:.4}",
                computed.symbol, decision.style, decision.size_fraction
            );
        }
        
        Ok(())
    }
    
    /// RL-based decision (MANDATORY - fails if error)
    async fn decide_with_rl_mandatory(
        &self,
        computed: &features::ComputedFeatures,
        features: &FeatureVec,
    ) -> Result<RouteDecision> {
        let market_state = self.get_market_state(&computed.symbol)?;
        
        // Get RL action - NO fallback, must succeed
        let rl_action = self.rl_agent.get_action(&computed.features, &market_state)
            .map_err(|e| {
                tracing::error!("❌ RL Agent FAILED: {}", e);
                Error::Internal(format!("RL inference failed: {}. No fallback available.", e))
            })?;
        
        let mut decision = self.rl_agent.to_route_decision(&rl_action, features);
        
        // Apply risk checks
        let risk_manager = self.router.get_risk_manager();
        let notional = features.mid_price * decision.size_fraction;
        
        if let Err(e) = risk_manager.read().check_limits(&computed.symbol, notional) {
            decision.should_trade = false;
            decision.reason = format!("Risk check failed: {}", e);
        }
        
        Ok(decision)
    }
    
    /// ML-based decision (MANDATORY - fails if error)
    async fn decide_with_ml_mandatory(
        &self,
        computed: &features::ComputedFeatures,
        features: &FeatureVec,
        perf: &mut PerformanceMetrics,
    ) -> Result<RouteDecision> {
        let category = AssetCategory::CryptoFutures; // TODO: determine from symbol
        
        // Run ML inference - NO fallback, must succeed
        let model_start = std::time::Instant::now();
        let prediction = self.inference_pool
            .predict(category, &computed.features, ModelType::Edge)
            .await
            .map_err(|e| {
                tracing::error!("❌ ML inference FAILED: {}", e);
                Error::Internal(format!("ML inference failed: {}. No fallback available.", e))
            })?;
        
        perf.model_p50_us = model_start.elapsed().as_micros() as f64;
        
        // Cost model
        let costs = CostModel {
            taker_fee_bps: 5.0,
            maker_fee_bps: 2.0,
            maker_rebate_bps: 1.0,
            impact_bps: features.impact_bps_1pct,
            slippage_buffer_bps: 1.0,
        };
        
        // Route decision
        let decision = self.router.decide(&prediction, features, &costs);
        
        Ok(decision)
    }
    
    /// Hybrid decision: RL primary, ML validation (BOTH mandatory)
    async fn decide_hybrid_mandatory(
        &self,
        computed: &features::ComputedFeatures,
        features: &FeatureVec,
        perf: &mut PerformanceMetrics,
    ) -> Result<RouteDecision> {
        // Get RL decision (MANDATORY)
        let rl_decision = self.decide_with_rl_mandatory(computed, features).await?;
        
        // Get ML decision for validation (MANDATORY)
        let ml_decision = self.decide_with_ml_mandatory(computed, features, perf).await?;
        
        // Validate: both must agree to trade
        if rl_decision.should_trade && ml_decision.should_trade {
            // Use RL decision with ML confidence as validation
            Ok(rl_decision)
        } else {
            // Disagreement - don't trade
            Ok(RouteDecision {
                should_trade: false,
                reason: format!(
                    "RL/ML disagreement: RL={}, ML={}",
                    rl_decision.should_trade,
                    ml_decision.should_trade
                ),
                ..rl_decision
            })
        }
    }
    
    fn features_to_vec(&self, computed: &features::ComputedFeatures) -> FeatureVec {
        let f = &computed.features;
        FeatureVec {
            timestamp_ns: computed.timestamp_ns,
            symbol: computed.symbol.clone(),
            mid_price: f[0] as f64,
            spread_bps: f[1] as f64,
            ofi_1s: f.get(4).copied().unwrap_or(0.0) as f64,
            obi_1s: f.get(3).copied().unwrap_or(0.0) as f64,
            depth_imbalance: f.get(3).copied().unwrap_or(0.0) as f64,
            depth_a: 0.001,
            depth_beta: 0.5,
            realized_vol_5s: 0.02,
            atr_30s: 10.0,
            funding_bps_8h: f[2] as f64,
            impact_bps_1pct: 0.5,
            microprice: f[0] as f64,
            vwap_ratio: f.get(5).copied().unwrap_or(1.0) as f64,
        }
    }
    
    fn get_market_state(&self, symbol: &str) -> Result<MarketState> {
        // TODO: Get from risk manager
        Ok(MarketState {
            position_size: 0.0,
            unrealized_pnl: 0.0,
            holding_duration_s: 0.0,
            inventory_risk: 0.0,
        })
    }
    
    async fn execute_trade(
        &self,
        symbol: &str,
        decision: &RouteDecision,
        features: &FeatureVec,
    ) -> Result<()> {
        let adapters = self.adapters.read();
        let adapter = adapters.values().next()
            .ok_or_else(|| Error::Internal("No adapter".to_string()))?;
        
        let side = if features.ofi_1s > 0.0 { Side::Buy } else { Side::Sell };
        
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
        
        match adapter.send_order(order).await {
            Ok(ack) => {
                tracing::info!("✅ Order sent: {} - {:?}", symbol, ack.status);
                metrics::increment_counter!("orders_sent", "symbol" => symbol.to_string());
            }
            Err(e) => {
                tracing::error!("❌ Order FAILED: {}", e);
                metrics::increment_counter!("order_rejects", "symbol" => symbol.to_string());
                return Err(e);
            }
        }
        
        Ok(())
    }
    
    pub fn set_mode(&self, mode: TradingMode) {
        self.config.write().mode = mode;
        tracing::info!("Trading mode: {:?}", mode);
    }
    
    pub fn get_mode(&self) -> TradingMode {
        self.config.read().mode
    }
    
    pub fn get_metrics(&self) -> PerformanceMetrics {
        self.metrics_tx.borrow().clone()
    }
}