// crates/engine/src/main.rs
use engine::*;
use common::*;
use adapters::{HyperliquidAdapter};
use common::security::{CredentialStore, ApiCredentials};
use std::sync::Arc;
use tokio::sync::{watch, broadcast};
use tracing_subscriber::EnvFilter;

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
    
    tracing::info!("HFT Trading Engine starting...");
    
    // Initialize metrics exporter
    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("Failed to install Prometheus recorder");
    
    // Load configuration
    let config = load_config()?;
    
    // Initialize trading engine
    let engine_config = EngineConfig {
        mode: TradingMode::Paper,
        feature_window_size: 1000,
        inference_timeout_ms: 3,
        gate_params: router::GateParams::default(),
        enable_s3_writer: false,
        s3_bucket: None,
    };
    
    let risk_limits = RiskLimits::default();
    
    let trading_engine = Arc::new(TradingEngine::new(engine_config, risk_limits)?);
    
    // Load ML models
    if let Err(e) = trading_engine.load_models("./models/crypto", "./models/equity") {
        tracing::warn!("Failed to load models: {}. Using rule-based fallback.", e);
    }
    
    // Initialize adapters
    let cred_store = CredentialStore::new_simple();
    
    // Load credentials and create adapters
    // Example: Hyperliquid
    match load_hyperliquid_adapter(&cred_store) {
        Ok(adapter) => {
            trading_engine.add_adapter("hyperliquid".to_string(), Arc::new(adapter));
            tracing::info!("Hyperliquid adapter added");
        }
        Err(e) => {
            tracing::warn!("Failed to load Hyperliquid adapter: {}", e);
        }
    }
    
    // Add symbols to track
    let symbols = vec!["BTC-USD", "ETH-USD", "SOL-USD"];
    for symbol in symbols {
        trading_engine.add_symbol(symbol.to_string(), 1000);
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
    
    // Spawn WebSocket server
    let ws_handle = tokio::spawn(async move {
        let addr = "0.0.0.0:8081";
        tracing::info!("WebSocket server listening on {}", addr);
        
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, ws_app).await.unwrap();
    });
    
    // Spawn universe manager if enabled
    let universe_handle = if config.enable_universe {
        let universe_config = universe::UniverseConfig::default();
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
        tokio::spawn(async move {
            engine_clone.run(shutdown_rx_clone).await
        })
    };
    
    // Spawn metrics updater
    let metrics_handle = {
        let engine_clone = trading_engine.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                interval.tick().await;
                let metrics = engine_clone.get_metrics();
                let _ = perf_tx.send(metrics);
            }
        })
    };
    
    // Spawn alert publisher
    let alert_publisher = Arc::new(ws_server::AlertPublisher::new(alert_tx, None, None));
    
    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Shutdown signal received");
            let _ = shutdown_tx.send(true);
        }
    }
    
    // Wait for tasks to complete
    if let Some(handle) = universe_handle {
        let _ = handle.await;
    }
    let _ = engine_handle.await;
    ws_handle.abort();
    metrics_handle.abort();
    
    tracing::info!("Engine shutdown complete");
    Ok(())
}

#[derive(serde::Deserialize)]
struct Config {
    enable_universe: bool,
}

fn load_config() -> Result<Config> {
    Ok(Config {
        enable_universe: std::env::var("ENABLE_UNIVERSE")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false),
    })
}

fn load_hyperliquid_adapter(store: &CredentialStore) -> Result<HyperliquidAdapter> {
    // Try to load from keychain
    match store.load(Venue::Hyperliquid, "default", false) {
        Ok(creds) => Ok(HyperliquidAdapter::new(creds)),
        Err(_) => {
            // Fallback to environment variables
            let api_key = std::env::var("HYPERLIQUID_API_KEY")
                .map_err(|_| Error::Config("HYPERLIQUID_API_KEY not set".to_string()))?;
            let api_secret = std::env::var("HYPERLIQUID_SECRET")
                .map_err(|_| Error::Config("HYPERLIQUID_SECRET not set".to_string()))?;
            
            let creds = ApiCredentials::new(api_key, api_secret, false);
            Ok(HyperliquidAdapter::new(creds))
        }
    }
}