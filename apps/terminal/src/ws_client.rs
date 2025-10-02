// apps/terminal/src/ws_client.rs
use common::*;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Metrics client for terminal UI
pub struct MetricsClient {
    performance: Arc<RwLock<PerformanceMetrics>>,
    risk: Arc<RwLock<RiskSnapshot>>,
    alerts: Arc<RwLock<Vec<Alert>>>,
}

impl MetricsClient {
    pub async fn connect(url: &str) -> Result<Self> {
        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| Error::WebSocket(format!("Connection failed: {}", e)))?;
        
        let (mut write, mut read) = ws_stream.split();
        
        let performance = Arc::new(RwLock::new(PerformanceMetrics::default()));
        let risk = Arc::new(RwLock::new(RiskSnapshot::default()));
        let alerts = Arc::new(RwLock::new(Vec::new()));
        
        let perf_clone = performance.clone();
        let risk_clone = risk.clone();
        let alerts_clone = alerts.clone();
        
        // Spawn receive loop
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        // Try to parse as different message types
                        if let Ok(perf) = serde_json::from_str::<PerformanceMetrics>(&text) {
                            *perf_clone.write().await = perf;
                        } else if let Ok(r) = serde_json::from_str::<RiskSnapshot>(&text) {
                            *risk_clone.write().await = r;
                        } else if let Ok(alert) = serde_json::from_str::<Alert>(&text) {
                            let mut alerts = alerts_clone.write().await;
                            alerts.push(alert);
                            if alerts.len() > 100 {
                                alerts.remove(0);
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        tracing::warn!("WebSocket closed");
                        break;
                    }
                    Err(e) => {
                        tracing::error!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        });
        
        Ok(Self {
            performance,
            risk,
            alerts,
        })
    }
    
    pub async fn get_performance(&self) -> PerformanceMetrics {
        self.performance.read().await.clone()
    }
    
    pub async fn get_risk(&self) -> RiskSnapshot {
        self.risk.read().await.clone()
    }
    
    pub async fn get_alerts(&self) -> Vec<Alert> {
        self.alerts.read().await.clone()
    }
}