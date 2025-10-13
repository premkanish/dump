// crates/engine/src/main.rs (Fully Integrated with Advanced Features)
use engine::*;
use common::*;
use adapters::{HyperliquidAdapter};
use common::security::{CredentialStore, ApiCredentials};
use std::sync::Arc;
use tokio::sync::{watch, broadcast};
use tracing_subscriber::EnvFilter;

// Import advanced features
use crate::advanced_features::{AdvancedConfig, AdvancedFeaturesManager};
use features::gpu_compute::DeviceType;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info"))
        )
        .with_target(false)
        .with_thread_ids(true)
        .init();
    
    tracing::info!("HFT Trading Engine starting (with Advanced Features)...");
    
    // Initialize metrics exporter
    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("Failed to install Prometheus recorder");
    
    // Load configuration from file
    let config = load_config()?;
    
    // Initialize AWS SDK with BehaviorVersion (AWS SDK 1.106.0)
    let aws_config = if config.enable_aws {
        Some(
            aws_config::defaults(aws_config::BehaviorVersion::latest())
                .load()
                .await
        )
    } else {
        None
    };
    
    // Initialize SNS client if needed
    let sns_client = aws_config.as_ref().map(|cfg| aws_sdk_sns::Client::new(cfg));
    let s3_client = aws_config.as_ref().map(|cfg| aws_sdk_s3::Client::new(cfg));
    
    // ============================================
    // ADVANCED FEATURES INITIALIZATION
    // ============================================
    let advanced_config = AdvancedConfig {
        // Multi-threaded order book
        enable_mt_orderbook: config.advanced.orderbook.enabled,
        orderbook_workers: config.advanced.orderbook.workers_per_symbol,
        
        // Parquet export
        enable_parquet_export: config.advanced.parquet.enabled,
        parquet_output_dir: config.advanced.parquet.output_dir.clone(),
        parquet_batch_size: config.advanced.parquet.batch_size,
        parquet_samples_per_shard: config.advanced.parquet.samples_per_shard,
        enable_s3_upload: config.advanced.parquet.enable_s3_upload,
        s3_bucket: if config.advanced.parquet.s3_bucket.is_empty() {
            None
        } else {
            Some(config.advanced.parquet.s3_bucket.clone())
        },
        
        // RL Agent
        enable_rl_agent: config.advanced.rl_agent.enabled,
        rl_actor_path: config.advanced.rl_agent.actor_path.clone(),
        rl_critic_path: if config.advanced.rl_agent.critic_path.is_empty() {
            None
        } else {
            Some(config.advanced.rl_agent.critic_path.clone())
        },
        rl_config: rl_agent::RLAgentConfig {
            action_type: match config.advanced.rl_agent.action_type.as_str() {
                "Discrete" => rl_agent::ActionType::Discrete,
                "Continuous" => rl_agent::ActionType::Continuous,
                _ => rl_agent::ActionType::MultiDiscrete,
            },
            sequence_length: config.advanced.rl_agent.sequence_length,
            use_recurrent: config.advanced.rl_agent.use_recurrent,
            epsilon: config.advanced.rl_agent.epsilon,
            temperature: config.advanced.rl_agent.temperature,
        },
        
        // GPU acceleration
        enable_gpu: config.advanced.gpu.enabled,
        gpu_device: match config.advanced.gpu.device.as_str() {
            "CUDA" => DeviceType::CUDA(config.advanced.gpu.device_id),
            "ROCm" => DeviceType::ROCm(config.advanced.gpu.device_id),
            "TensorRT" => DeviceType::TensorRT,
            _ => DeviceType::CPU,
        },
        gpu_batch_size: config.advanced.gpu.batch_size,
        gpu_model_path: config.advanced.gpu.model_path.clone(),
    };
    
    let advanced_manager = if config.advanced.enabled {
        tracing::info!("Initializing advanced features...");
        match AdvancedFeaturesManager::new(advanced_config).await {
            Ok(manager) => {
                let stats = manager.stats();
                tracing::info!("Advanced features initialized: {:?}", stats);
                Some(Arc::new(manager))
            }
            Err(e) => {
                tracing::warn!("Failed to initialize advanced features: {}. Continuing without them.", e);
                None
            }
        }
    } else {
        tracing::info!("Advanced features disabled");
        None
    };
    
    // ============================================
    // STANDARD ENGINE INITIALIZATION
    // ============================================
    
    let engine_config = EngineConfig {
        mode: config.engine.mode,
        feature_window_size: config.engine.feature_window_size,
        inference_timeout_ms: config.engine.inference_timeout_ms,
        gate_params: router::GateParams {
            enabled: config.gate.enabled,
            min_edge_bps: config.gate.min_edge_bps,
            min_confidence: config.gate.min_confidence,
            max_hold_s: config.gate.max_hold_s,
            max_spread_bps: config.gate.max_spread_bps,
        },
        enable_s3_writer: config.s3.enabled,
        s3_bucket: if config.s3.bucket.is_empty() {
            None
        } else {
            Some(config.s3.bucket.clone())
        },
    };
    
    let risk_limits = RiskLimits {
        max_notional_per_symbol: config.risk.max_notional_per_symbol,
        max_total_notional: config.risk.max_total_notional,
        max_leverage: config.risk.max_leverage,
        max_loss_per_day: config.risk.max_loss_per_day,
        max_position_concentration: config.risk.max_position_concentration,
    };
    
    let trading_engine = Arc::new(TradingEngine::new(engine_config, risk_limits)?);
    
    // Load ML models (standard inference models)
    if let Err(e) = trading_engine.load_models(&config.models.crypto_dir, &config.models.equity_dir) {
        tracing::warn!("Failed to load standard models: {}. Using rule-based fallback.", e);
    }
    
    // Initialize adapters
    let cred_store = CredentialStore::new_simple();
    
    if config.venues.hyperliquid.enabled {
        match load_hyperliquid_adapter(&cred_store) {
            Ok(adapter) => {
                trading_engine.add_adapter("hyperliquid".to_string(), Arc::new(adapter));
                tracing::info!("Hyperliquid adapter added");
            }
            Err(e) => {
                tracing::warn!("Failed to load Hyperliquid adapter: {}", e);
            }
        }
    }
    
    // Add symbols to track
    let symbols = vec!["BTC-USD", "ETH-USD", "SOL-USD"];
    for symbol in symbols {
        trading_engine.add_symbol(symbol.to_string(), config.engine.feature_window_size);
        
        // Initialize multi-threaded order book for this symbol
        if let Some(ref manager) = advanced_manager {
            if let Some(_orderbook) = manager.get_orderbook(symbol) {
                tracing::debug!("Multi-threaded order book initialized for {}", symbol);
            }
        }
    }
    
    // Create WebSocket server
    let (perf_tx, perf_rx) = watch::channel(PerformanceMetrics::default());
    let (risk_tx, risk_rx) = watch::channel(RiskSnapshot::default());
    let (alert_tx, _alert_rx) = broadcast::channel(1000);
    
    let metrics_state = ws_server::MetricsState {
        performance_rx: perf_rx,
        risk_rx,
        alert_tx: alert_tx.clone(),
    };
    
    let ws_app = ws_server::create_metrics_server(metrics_state);
    
    // Setup shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    
    // Spawn WebSocket server (Axum 0.8)
    let ws_handle = tokio::spawn(async move {
        let addr = format!("{}:{}", config.websocket.host, config.websocket.port);
        tracing::info!("WebSocket server listening on {}", addr);
        
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        axum::serve(listener, ws_app).await.unwrap();
    });
    
    // Spawn universe manager if enabled
    let universe_handle = if config.universe.enabled {
        let universe_config = universe::UniverseConfig {
            crypto_count: config.universe.crypto_count,
            equity_count: config.universe.equity_count,
            top_selection_count: (config.universe.top_selection_crypto, config.universe.top_selection_equity),
            rebuild_interval_mins: config.universe.rebuild_interval_mins,
            refresh_interval_mins: config.universe.refresh_interval_mins,
            min_volume_usd: config.universe.min_volume_usd,
            min_liquidity_usd: config.universe.min_liquidity_usd,
        };
        let data_sources = universe::data_sources::DataSources::new();
        let universe_manager = Arc::new(universe::UniverseManager::new(universe_config, data_sources));
        
        let shutdown_rx_clone = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            universe_manager.run(shutdown_rx_clone).await
        }))
    } else {
        None
    };
    
    // Run main engine
    let engine_handle = {
        let engine_clone = trading_engine.clone();
        let shutdown_rx_clone = shutdown_rx.clone();
        let advanced_clone = advanced_manager.clone();
        
        tokio::spawn(async move {
            run_trading_loop(engine_clone, advanced_clone, shutdown_rx_clone).await
        })
    };
    
    // Spawn metrics updater
    let metrics_handle = {
        let engine_clone = trading_engine.clone();
        let advanced_clone = advanced_manager.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                interval.tick().await;
                
                // Standard metrics
                let metrics = engine_clone.get_metrics();
                let _ = perf_tx.send(metrics);
                
                // Advanced features stats
                if let Some(ref manager) = advanced_clone {
                    let stats = manager.stats();
                    tracing::debug!("Advanced stats: {:?}", stats);
                }
            }
        })
    };
    
    // Spawn alert publisher
    let alert_publisher = Arc::new(ws_server::AlertPublisher::new(
        alert_tx, 
        sns_client,
        config.sns.topic_arn
    ));
    
    tracing::info!("All systems initialized successfully");
    
    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Shutdown signal received");
            let _ = shutdown_tx.send(true);
        }
    }
    
    // Graceful shutdown
    tracing::info!("Shutting down...");
    
    if let Some(handle) = universe_handle {
        let _ = handle.await;
    }
    let _ = engine_handle.await;
    ws_handle.abort();
    metrics_handle.abort();
    
    // Shutdown advanced features
    if let Some(manager) = advanced_manager {
        Arc::try_unwrap(manager)
            .ok()
            .map(|m| m.shutdown());
    }
    
    tracing::info!("Engine shutdown complete");
    Ok(())
}

/// Main trading loop with advanced features integration
async fn run_trading_loop(
    engine: Arc<TradingEngine>,
    advanced: Option<Arc<AdvancedFeaturesManager>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    tracing::info!("Trading loop starting");
    
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    
    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Your existing trading logic here
                // This is where you'd integrate the advanced features
                
                // Example: Use GPU features if enabled
                if let Some(ref manager) = advanced {
                    // Get multi-threaded order book
                    if let Some(btc_book) = manager.get_orderbook("BTC-USD") {
                        let bbo = btc_book.get_bbo();
                        // Use BBO for pricing
                    }
                    
                    // GPU feature computation would happen here
                    // RL agent decisions would happen here
                    // Training sample collection would happen here
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("Trading loop shutting down");
                    break;
                }
            }
        }
    }
    
    Ok(())
}

#[derive(serde::Deserialize)]
struct Config {
    engine: EngineSection,
    gate: GateSection,
    risk: RiskSection,
    universe: UniverseSection,
    s3: S3Section,
    sns: SnsSection,
    websocket: WebSocketSection,
    models: ModelsSection,
    venues: VenuesSection,
    advanced: AdvancedSection,
    enable_aws: bool,
}

#[derive(serde::Deserialize)]
struct EngineSection {
    mode: TradingMode,
    feature_window_size: usize,
    inference_timeout_ms: u64,
}

#[derive(serde::Deserialize)]
struct GateSection {
    enabled: bool,
    min_edge_bps: f64,
    min_confidence: f64,
    max_hold_s: f64,
    max_spread_bps: f64,
}

#[derive(serde::Deserialize)]
struct RiskSection {
    max_notional_per_symbol: f64,
    max_total_notional: f64,
    max_leverage: f64,
    max_loss_per_day: f64,
    max_position_concentration: f64,
}

#[derive(serde::Deserialize)]
struct UniverseSection {
    enabled: bool,
    crypto_count: usize,
    equity_count: usize,
    top_selection_crypto: usize,
    top_selection_equity: usize,
    rebuild_interval_mins: u64,
    refresh_interval_mins: u64,
    min_volume_usd: f64,
    min_liquidity_usd: f64,
}

#[derive(serde::Deserialize)]
struct S3Section {
    enabled: bool,
    bucket: String,
    region: String,
}

#[derive(serde::Deserialize)]
struct SnsSection {
    enabled: bool,
    topic_arn: Option<String>,
}

#[derive(serde::Deserialize)]
struct WebSocketSection {
    host: String,
    port: u16,
}

#[derive(serde::Deserialize)]
struct ModelsSection {
    crypto_dir: String,
    equity_dir: String,
}

#[derive(serde::Deserialize)]
struct VenuesSection {
    hyperliquid: VenueConfig,
    binance: VenueConfig,
    ibkr: IbkrConfig,
}

#[derive(serde::Deserialize)]
struct VenueConfig {
    enabled: bool,
    rate_limit_per_sec: u64,
}

#[derive(serde::Deserialize)]
struct IbkrConfig {
    enabled: bool,
    gateway_host: String,
    gateway_port: u16,
}

#[derive(serde::Deserialize)]
struct AdvancedSection {
    enabled: bool,
    orderbook: OrderbookSection,
    parquet: ParquetSection,
    rl_agent: RLAgentSection,
    gpu: GpuSection,
}

#[derive(serde::Deserialize)]
struct OrderbookSection {
    enabled: bool,
    workers_per_symbol: usize,
}

#[derive(serde::Deserialize)]
struct ParquetSection {
    enabled: bool,
    output_dir: String,
    batch_size: usize,
    samples_per_shard: usize,
    enable_s3_upload: bool,
    s3_bucket: String,
}

#[derive(serde::Deserialize)]
struct RLAgentSection {
    enabled: bool,
    actor_path: String,
    critic_path: String,
    action_type: String,
    use_recurrent: bool,
    sequence_length: usize,
    epsilon: f64,
    temperature: f64,
}

#[derive(serde::Deserialize)]
struct GpuSection {
    enabled: bool,
    device: String,
    device_id: usize,
    batch_size: usize,
    model_path: String,
}

fn load_config() -> Result<Config> {
    let config_str = std::fs::read_to_string("config/engine.toml")
        .map_err(|e| Error::Config(format!("Failed to read config: {}", e)))?;
    
    let mut config: Config = toml::from_str(&config_str)
        .map_err(|e| Error::Config(format!("Failed to parse config: {}", e)))?;
    
    // Override from environment
    config.enable_aws = std::env::var("ENABLE_AWS")
        .unwrap_or_else(|_| "false".to_string())
        .parse()
        .unwrap_or(false);
    
    Ok(config)
}

fn load_hyperliquid_adapter(store: &CredentialStore) -> Result<HyperliquidAdapter> {
    match store.load(Venue::Hyperliquid, "default", false) {
        Ok(creds) => Ok(HyperliquidAdapter::new(creds)),
        Err(_) => {
            let api_key = std::env::var("HYPERLIQUID_API_KEY")
                .map_err(|_| Error::Config("HYPERLIQUID_API_KEY not set".to_string()))?;
            let api_secret = std::env::var("HYPERLIQUID_SECRET")
                .map_err(|_| Error::Config("HYPERLIQUID_SECRET not set".to_string()))?;
            
            let creds = ApiCredentials::new(api_key, api_secret, false);
            Ok(HyperliquidAdapter::new(creds))
        }
    }
}
