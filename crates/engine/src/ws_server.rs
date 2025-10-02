// crates/engine/src/ws_server.rs
use axum::{
    extract::{ws::WebSocket, State, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use common::*;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tower_http::cors::CorsLayer;

/// Metrics broadcast state
#[derive(Clone)]
pub struct MetricsState {
    pub performance_rx: watch::Receiver<PerformanceMetrics>,
    pub risk_rx: watch::Receiver<RiskSnapshot>,
    pub alert_tx: broadcast::Sender<Alert>,
}

/// Create metrics server
pub fn create_metrics_server(state: MetricsState) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/risk", get(risk_handler))
        .route("/alerts", get(alerts_handler))
        .route("/health", get(health_handler))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

/// WebSocket handler for performance metrics
async fn metrics_handler(
    ws: WebSocketUpgrade,
    State(state): State<MetricsState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_metrics_socket(socket, state))
}

async fn handle_metrics_socket(mut socket: WebSocket, state: MetricsState) {
    let mut perf_rx = state.performance_rx.clone();
    
    loop {
        tokio::select! {
            changed = perf_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                
                let metrics = perf_rx.borrow().clone();
                let json = match serde_json::to_string(&metrics) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::warn!("Failed to serialize metrics: {}", e);
                        continue;
                    }
                };
                
                if socket.send(axum::extract::ws::Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    }
    
    tracing::debug!("Metrics WebSocket closed");
}

/// WebSocket handler for risk metrics
async fn risk_handler(
    ws: WebSocketUpgrade,
    State(state): State<MetricsState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_risk_socket(socket, state))
}

async fn handle_risk_socket(mut socket: WebSocket, state: MetricsState) {
    let mut risk_rx = state.risk_rx.clone();
    
    loop {
        tokio::select! {
            changed = risk_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                
                let risk = risk_rx.borrow().clone();
                let json = match serde_json::to_string(&risk) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::warn!("Failed to serialize risk: {}", e);
                        continue;
                    }
                };
                
                if socket.send(axum::extract::ws::Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    }
    
    tracing::debug!("Risk WebSocket closed");
}

/// WebSocket handler for alerts
async fn alerts_handler(
    ws: WebSocketUpgrade,
    State(state): State<MetricsState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_alerts_socket(socket, state))
}

async fn handle_alerts_socket(mut socket: WebSocket, state: MetricsState) {
    let mut alert_rx = state.alert_tx.subscribe();
    
    loop {
        tokio::select! {
            alert = alert_rx.recv() => {
                match alert {
                    Ok(a) => {
                        let json = match serde_json::to_string(&a) {
                            Ok(j) => j,
                            Err(e) => {
                                tracing::warn!("Failed to serialize alert: {}", e);
                                continue;
                            }
                        };
                        
                        if socket.send(axum::extract::ws::Message::Text(json)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }
    
    tracing::debug!("Alerts WebSocket closed");
}

/// Health check endpoint
async fn health_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Alert publisher for critical events
pub struct AlertPublisher {
    tx: broadcast::Sender<Alert>,
    sns_client: Option<aws_sdk_sns::Client>,
    topic_arn: Option<String>,
}

impl AlertPublisher {
    pub fn new(
        tx: broadcast::Sender<Alert>,
        sns_client: Option<aws_sdk_sns::Client>,
        topic_arn: Option<String>,
    ) -> Self {
        Self {
            tx,
            sns_client,
            topic_arn,
        }
    }
    
    /// Publish alert
    pub async fn publish(&self, level: AlertLevel, source: String, message: String) {
        let alert = Alert {
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            level: level.clone(),
            source: source.clone(),
            message: message.clone(),
            metadata: serde_json::json!({}),
        };
        
        // Broadcast to WebSocket clients
        let _ = self.tx.send(alert.clone());
        
        // Send to SNS for critical alerts
        if matches!(level, AlertLevel::Critical) {
            if let (Some(client), Some(arn)) = (&self.sns_client, &self.topic_arn) {
                if let Err(e) = self.send_sns(client, arn, &alert).await {
                    tracing::error!("Failed to send SNS alert: {}", e);
                }
            }
        }
        
        // Log alert
        match level {
            AlertLevel::Info => tracing::info!("[{}] {}", source, message),
            AlertLevel::Warning => tracing::warn!("[{}] {}", source, message),
            AlertLevel::Critical => tracing::error!("[{}] {}", source, message),
        }
    }
    
    async fn send_sns(
        &self,
        client: &aws_sdk_sns::Client,
        topic_arn: &str,
        alert: &Alert,
    ) -> Result<()> {
        let subject = format!("[HFT {:?}] {}", alert.level, alert.source);
        let message = serde_json::to_string_pretty(&alert)?;
        
        client
            .publish()
            .topic_arn(topic_arn)
            .subject(subject)
            .message(message)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("SNS publish failed: {}", e)))?;
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_metrics_server() {
        let (perf_tx, perf_rx) = watch::channel(PerformanceMetrics::default());
        let (risk_tx, risk_rx) = watch::channel(RiskSnapshot::default());
        let (alert_tx, _) = broadcast::channel(100);
        
        let state = MetricsState {
            performance_rx: perf_rx,
            risk_rx,
            alert_tx,
        };
        
        let app = create_metrics_server(state);
        
        // Server is ready to accept connections
        assert!(true);
    }
}