// crates/adapters/src/hyperliquid.rs
use crate::*;
use common::*;
use common::security::ApiCredentials;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const WS_URL: &str = "wss://api.hyperliquid.xyz/ws";
const REST_URL: &str = "https://api.hyperliquid.xyz/info";

pub struct HyperliquidAdapter {
    credentials: ApiCredentials,
    rate_limiter: RateLimiter,
    snapshot_tx: mpsc::UnboundedSender<MarketSnapshot>,
    snapshot_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<MarketSnapshot>>>>,
    books: Arc<RwLock<HashMap<String, OrderBookMaintainer>>>,
    client: reqwest::Client,
    connected: Arc<RwLock<bool>>,
}

impl HyperliquidAdapter {
    pub fn new(credentials: ApiCredentials) -> Self {
        let (snapshot_tx, snapshot_rx) = mpsc::unbounded_channel();
        
        Self {
            credentials,
            rate_limiter: RateLimiter::new(100, 10.0), // 10 req/sec
            snapshot_tx,
            snapshot_rx: Arc::new(RwLock::new(Some(snapshot_rx))),
            books: Arc::new(RwLock::new(HashMap::new())),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            connected: Arc::new(RwLock::new(false)),
        }
    }
    
    async fn ws_loop(
        books: Arc<RwLock<HashMap<String, OrderBookMaintainer>>>,
        snapshot_tx: mpsc::UnboundedSender<MarketSnapshot>,
    ) {
        loop {
            match connect_async(WS_URL).await {
                Ok((ws_stream, _)) => {
                    tracing::info!("Hyperliquid WS connected");
                    let (mut write, mut read) = ws_stream.split();
                    
                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                if let Err(e) = Self::handle_ws_message(&text, &books, &snapshot_tx).await {
                                    tracing::warn!("Failed to handle WS message: {}", e);
                                }
                            }
                            Ok(Message::Close(_)) => {
                                tracing::warn!("Hyperliquid WS closed");
                                break;
                            }
                            Err(e) => {
                                tracing::error!("Hyperliquid WS error: {}", e);
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to connect to Hyperliquid WS: {}", e);
                }
            }
            
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
    
    async fn handle_ws_message(
        text: &str,
        books: &Arc<RwLock<HashMap<String, OrderBookMaintainer>>>,
        snapshot_tx: &mpsc::UnboundedSender<MarketSnapshot>,
    ) -> Result<()> {
        #[derive(Deserialize)]
        struct WsMessage {
            channel: String,
            data: serde_json::Value,
        }
        
        let msg: WsMessage = serde_json::from_str(text)?;
        
        match msg.channel.as_str() {
            "l2Book" => {
                #[derive(Deserialize)]
                struct L2Book {
                    coin: String,
                    levels: Vec<Vec<serde_json::Value>>,
                    time: i64,
                }
                
                let book: L2Book = serde_json::from_value(msg.data)?;
                let mut books_guard = books.write().await;
                
                let maintainer = books_guard
                    .entry(book.coin.clone())
                    .or_insert_with(|| OrderBookMaintainer::new(book.coin.clone()));
                
                // Process levels
                for level in book.levels {
                    if level.len() >= 3 {
                        let side = if level[0].as_str() == Some("bid") { Side::Buy } else { Side::Sell };
                        let price = level[1].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                        let qty = level[2].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                        
                        maintainer.apply_delta(BookDelta::Update { side, price, quantity: qty });
                    }
                }
                
                let orderbook = maintainer.to_orderbook(book.time * 1_000_000, 20);
                
                let snapshot = MarketSnapshot {
                    timestamp_ns: book.time * 1_000_000,
                    symbol: book.coin,
                    orderbook,
                    recent_trades: vec![],
                    funding_rate_bps: None,
                    open_interest: None,
                    volume_24h: 0.0,
                };
                
                let _ = snapshot_tx.send(snapshot);
            }
            "trades" => {
                // Handle trades
            }
            _ => {}
        }
        
        Ok(())
    }
    
    async fn post_request<T: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        endpoint: &str,
        payload: &T,
    ) -> Result<R> {
        let _guard = self.rate_limiter.acquire().await;
        
        let response = self.client
            .post(format!("{}/{}", REST_URL, endpoint))
            .json(payload)
            .send()
            .await?;
        
        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(Error::Venue(format!("Hyperliquid API error: {}", error_text)));
        }
        
        Ok(response.json().await?)
    }
}

#[async_trait]
impl MarketDataStream for HyperliquidAdapter {
    async fn subscribe_orderbook(&mut self, symbols: &[String]) -> Result<()> {
        let books = self.books.clone();
        let snapshot_tx = self.snapshot_tx.clone();
        
        tokio::spawn(async move {
            Self::ws_loop(books, snapshot_tx).await;
        });
        
        Ok(())
    }
    
    async fn subscribe_trades(&mut self, _symbols: &[String]) -> Result<()> {
        Ok(())
    }
    
    fn snapshot_receiver(&self) -> mpsc::UnboundedReceiver<MarketSnapshot> {
        self.snapshot_rx.write().await.take().expect("Receiver already taken")
    }
}

#[async_trait]
impl AccountData for HyperliquidAdapter {
    async fn balances(&self) -> Result<HashMap<String, Balance>> {
        #[derive(Serialize)]
        struct Request {
            #[serde(rename = "type")]
            req_type: String,
            user: String,
        }
        
        #[derive(Deserialize)]
        struct Response {
            balances: Vec<BalanceItem>,
        }
        
        #[derive(Deserialize)]
        struct BalanceItem {
            coin: String,
            total: String,
        }
        
        let req = Request {
            req_type: "clearinghouseState".to_string(),
            user: self.credentials.api_key.clone(),
        };
        
        let resp: Response = self.post_request("info", &req).await?;
        
        let mut balances = HashMap::new();
        for item in resp.balances {
            let total = item.total.parse::<f64>().unwrap_or(0.0);
            balances.insert(
                item.coin.clone(),
                Balance {
                    asset: item.coin,
                    free: total,
                    locked: 0.0,
                    total,
                },
            );
        }
        
        Ok(balances)
    }
    
    async fn positions(&self) -> Result<Vec<Position>> {
        #[derive(Serialize)]
        struct Request {
            #[serde(rename = "type")]
            req_type: String,
            user: String,
        }
        
        #[derive(Deserialize)]
        struct Response {
            #[serde(rename = "assetPositions")]
            asset_positions: Vec<PositionItem>,
        }
        
        #[derive(Deserialize)]
        struct PositionItem {
            position: PositionData,
        }
        
        #[derive(Deserialize)]
        struct PositionData {
            coin: String,
            szi: String,
            #[serde(rename = "entryPx")]
            entry_px: String,
            #[serde(rename = "positionValue")]
            position_value: String,
            #[serde(rename = "unrealizedPnl")]
            unrealized_pnl: String,
        }
        
        let req = Request {
            req_type: "clearinghouseState".to_string(),
            user: self.credentials.api_key.clone(),
        };
        
        let resp: Response = self.post_request("info", &req).await?;
        
        let positions = resp.asset_positions
            .into_iter()
            .map(|item| {
                let pos = item.position;
                Position {
                    symbol: pos.coin,
                    size: pos.szi.parse().unwrap_or(0.0),
                    entry_price: pos.entry_px.parse().unwrap_or(0.0),
                    mark_price: 0.0,
                    unrealized_pnl: pos.unrealized_pnl.parse().unwrap_or(0.0),
                    realized_pnl: 0.0,
                    leverage: 1.0,
                    margin_used: pos.position_value.parse().unwrap_or(0.0),
                    liquidation_price: None,
                }
            })
            .collect();
        
        Ok(positions)
    }
    
    async fn fee_tier(&self) -> Result<FeeTier> {
        Ok(FeeTier {
            maker_fee_bps: 2.0,
            taker_fee_bps: 5.0,
            volume_30d: 0.0,
        })
    }
    
    async fn leverage(&self) -> Result<f64> {
        Ok(1.0)
    }
}

#[async_trait]
impl OrderRouter for HyperliquidAdapter {
    async fn send_order(&self, order: OrderRequest) -> Result<OrderAck> {
        #[derive(Serialize)]
        struct OrderPayload {
            coin: String,
            is_buy: bool,
            sz: f64,
            limit_px: f64,
            order_type: OrderTypePayload,
            reduce_only: bool,
        }
        
        #[derive(Serialize)]
        struct OrderTypePayload {
            limit: LimitOrder,
        }
        
        #[derive(Serialize)]
        struct LimitOrder {
            tif: String,
        }
        
        let payload = OrderPayload {
            coin: order.symbol.clone(),
            is_buy: matches!(order.side, Side::Buy),
            sz: order.quantity,
            limit_px: order.price.unwrap_or(0.0),
            order_type: OrderTypePayload {
                limit: LimitOrder {
                    tif: "Gtc".to_string(),
                },
            },
            reduce_only: order.reduce_only,
        };
        
        #[derive(Deserialize)]
        struct Response {
            status: String,
            response: ResponseData,
        }
        
        #[derive(Deserialize)]
        struct ResponseData {
            #[serde(rename = "type")]
            response_type: String,
            data: Option<OrderData>,
        }
        
        #[derive(Deserialize)]
        struct OrderData {
            statuses: Vec<OrderStatusData>,
        }
        
        #[derive(Deserialize)]
        struct OrderStatusData {
            filled: bool,
        }
        
        let resp: Response = self.post_request("exchange", &payload).await?;
        
        Ok(OrderAck {
            venue_order_id: order.client_id.clone(),
            client_id: order.client_id,
            status: OrderStatus::Accepted,
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        })
    }
    
    async fn cancel_order(&self, _order_id: &str) -> Result<()> {
        Ok(())
    }
    
    async fn cancel_all(&self, _symbol: &str) -> Result<()> {
        Ok(())
    }
    
    async fn get_order(&self, order_id: &str) -> Result<OrderAck> {
        Ok(OrderAck {
            venue_order_id: order_id.to_string(),
            client_id: order_id.to_string(),
            status: OrderStatus::Filled,
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        })
    }
}

#[async_trait]
impl MarketInfo for HyperliquidAdapter {
    async fn list_symbols(&self) -> Result<Vec<String>> {
        #[derive(Serialize)]
        struct Request {
            #[serde(rename = "type")]
            req_type: String,
        }
        
        #[derive(Deserialize)]
        struct Response {
            universe: Vec<UniverseItem>,
        }
        
        #[derive(Deserialize)]
        struct UniverseItem {
            name: String,
        }
        
        let req = Request {
            req_type: "meta".to_string(),
        };
        
        let resp: Response = self.post_request("info", &req).await?;
        
        Ok(resp.universe.into_iter().map(|item| item.name).collect())
    }
    
    async fn search_symbols(&self, prefix: &str) -> Result<Vec<String>> {
        let all_symbols = self.list_symbols().await?;
        Ok(all_symbols
            .into_iter()
            .filter(|s| s.to_lowercase().starts_with(&prefix.to_lowercase()))
            .collect())
    }
    
    async fn funding_rate(&self, _symbol: &str) -> Result<f64> {
        Ok(0.0)
    }
    
    async fn open_interest(&self, _symbol: &str) -> Result<f64> {
        Ok(0.0)
    }
    
    async fn volume_24h(&self, _symbol: &str) -> Result<f64> {
        Ok(0.0)
    }
}

#[async_trait]
impl ExchangeAdapter for HyperliquidAdapter {
    fn venue(&self) -> Venue {
        Venue::Hyperliquid
    }
    
    fn is_connected(&self) -> bool {
        *self.connected.blocking_read()
    }
    
    async fn connect(&mut self) -> Result<()> {
        *self.connected.write().await = true;
        Ok(())
    }
    
    async fn disconnect(&mut self) -> Result<()> {
        *self.connected.write().await = false;
        Ok(())
    }
}