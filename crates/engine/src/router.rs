// crates/engine/src/router.rs
use common::*;
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;

/// Gate parameters
#[derive(Debug, Clone)]
pub struct GateParams {
    pub min_edge_bps: f64,
    pub min_confidence: f64,
    pub max_hold_s: f64,
    pub max_spread_bps: f64,
    pub enabled: bool,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            min_edge_bps: 5.0,
            min_confidence: 0.5,
            max_hold_s: 30.0,
            max_spread_bps: 10.0,
            enabled: true,
        }
    }
}

/// Cost model for trading
#[derive(Debug, Clone)]
pub struct CostModel {
    pub taker_fee_bps: f64,
    pub maker_fee_bps: f64,
    pub maker_rebate_bps: f64,
    pub impact_bps: f64,
    pub slippage_buffer_bps: f64,
}

impl CostModel {
    pub fn total_cost_taker(&self) -> f64 {
        self.taker_fee_bps + self.impact_bps + self.slippage_buffer_bps
    }
    
    pub fn total_cost_maker(&self) -> f64 {
        self.maker_fee_bps + self.impact_bps + self.slippage_buffer_bps - self.maker_rebate_bps
    }
    
    pub fn net_edge_taker(&self, pred_edge_bps: f64) -> f64 {
        pred_edge_bps - self.total_cost_taker()
    }
    
    pub fn net_edge_maker(&self, pred_edge_bps: f64) -> f64 {
        pred_edge_bps - self.total_cost_maker()
    }
}

/// Trade gate - decides if signal is strong enough
pub struct TradeGate {
    params: Arc<RwLock<GateParams>>,
}

impl TradeGate {
    pub fn new(params: GateParams) -> Self {
        Self {
            params: Arc::new(RwLock::new(params)),
        }
    }
    
    pub fn update_params(&self, params: GateParams) {
        *self.params.write() = params;
    }
    
    /// Check if trade passes gate
    pub fn check(
        &self,
        prediction: &Prediction,
        features: &FeatureVec,
        costs: &CostModel,
        risk: &RiskState,
    ) -> GateResult {
        let params = self.params.read();
        
        if !params.enabled {
            return GateResult::Reject("Gate disabled".to_string());
        }
        
        // Check confidence
        if prediction.confidence < params.min_confidence {
            return GateResult::Reject(format!(
                "Low confidence: {:.3} < {:.3}",
                prediction.confidence, params.min_confidence
            ));
        }
        
        // Check spread
        if features.spread_bps > params.max_spread_bps {
            return GateResult::Reject(format!(
                "Wide spread: {:.2} > {:.2} bps",
                features.spread_bps, params.max_spread_bps
            ));
        }
        
        // Check net edge after costs
        let net_edge = costs.net_edge_taker(prediction.edge_bps);
        if net_edge < params.min_edge_bps {
            return GateResult::Reject(format!(
                "Insufficient edge: {:.2} < {:.2} bps",
                net_edge, params.min_edge_bps
            ));
        }
        
        // Check risk limits
        if risk.kill_switch_active {
            return GateResult::Reject("Kill switch active".to_string());
        }
        
        if risk.daily_loss_exceeded {
            return GateResult::Reject("Daily loss limit exceeded".to_string());
        }
        
        GateResult::Pass {
            net_edge_bps: net_edge,
            urgency: self.compute_urgency(prediction, features),
        }
    }
    
    fn compute_urgency(&self, prediction: &Prediction, features: &FeatureVec) -> f64 {
        // Higher urgency for:
        // - Higher confidence
        // - Tighter spread
        // - Stronger signal
        
        let confidence_factor = prediction.confidence;
        let spread_factor = (10.0 - features.spread_bps).max(0.0) / 10.0;
        let signal_factor = (prediction.edge_bps.abs() / 20.0).min(1.0);
        
        (confidence_factor * 0.4 + spread_factor * 0.3 + signal_factor * 0.3).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone)]
pub enum GateResult {
    Pass { net_edge_bps: f64, urgency: f64 },
    Reject(String),
}

/// Risk state
#[derive(Debug, Clone)]
pub struct RiskState {
    pub current_notional: f64,
    pub max_notional: f64,
    pub daily_pnl: f64,
    pub daily_loss_limit: f64,
    pub kill_switch_active: bool,
    pub daily_loss_exceeded: bool,
}

impl RiskState {
    pub fn can_trade(&self, additional_notional: f64) -> bool {
        !self.kill_switch_active
            && !self.daily_loss_exceeded
            && (self.current_notional + additional_notional) <= self.max_notional
    }
}

/// Order router
pub struct OrderRouter {
    gate: TradeGate,
    risk_manager: Arc<RwLock<RiskManager>>,
}

impl OrderRouter {
    pub fn new(gate_params: GateParams, risk_limits: RiskLimits) -> Self {
        Self {
            gate: TradeGate::new(gate_params),
            risk_manager: Arc::new(RwLock::new(RiskManager::new(risk_limits))),
        }
    }
    
    /// Make routing decision
    pub fn decide(
        &self,
        prediction: &Prediction,
        features: &FeatureVec,
        costs: &CostModel,
    ) -> RouteDecision {
        let risk_state = self.risk_manager.read().get_state();
        
        // Check gate
        let gate_result = self.gate.check(prediction, features, costs, &risk_state);
        
        let (should_trade, reason, urgency) = match gate_result {
            GateResult::Pass { net_edge_bps, urgency } => {
                (true, format!("Edge: {:.2} bps", net_edge_bps), urgency)
            }
            GateResult::Reject(reason) => (false, reason, 0.0),
        };
        
        if !should_trade {
            return RouteDecision {
                style: OrderStyle::MakerPassive,
                size_fraction: 0.0,
                hold_duration_s: 0.0,
                urgency: 0.0,
                should_trade: false,
                reason,
            };
        }
        
        // Determine order style based on urgency and spread
        let style = self.select_style(urgency, features.spread_bps);
        
        // Size based on conviction and risk
        let size_fraction = self.compute_size(prediction.confidence, urgency);
        
        // Hold time based on prediction horizon and market conditions
        let hold_duration_s = self.compute_hold_time(
            prediction.horizon_ms,
            features.spread_bps,
            urgency,
        );
        
        RouteDecision {
            style,
            size_fraction,
            hold_duration_s,
            urgency,
            should_trade: true,
            reason,
        }
    }
    
    fn select_style(&self, urgency: f64, spread_bps: f64) -> OrderStyle {
        if urgency > 0.8 {
            OrderStyle::TakerNow
        } else if urgency > 0.5 && spread_bps < 3.0 {
            OrderStyle::Sniper // Join best bid/ask
        } else {
            OrderStyle::MakerPassive
        }
    }
    
    fn compute_size(&self, confidence: f64, urgency: f64) -> f64 {
        // Kelly-inspired sizing with conservative fraction
        let base_size = 0.02; // 2% base
        let confidence_multiplier = confidence.powf(2.0);
        let urgency_multiplier = 1.0 + urgency * 0.5;
        
        (base_size * confidence_multiplier * urgency_multiplier).min(0.10)
    }
    
    fn compute_hold_time(&self, horizon_ms: u64, spread_bps: f64, urgency: f64) -> f64 {
        let base_hold = (horizon_ms as f64 / 1000.0) * 0.5;
        
        // Reduce hold time for wide spreads (harder to exit)
        let spread_factor = if spread_bps > 5.0 {
            0.7
        } else {
            1.0
        };
        
        // Reduce hold time for urgent trades
        let urgency_factor = 1.0 - urgency * 0.3;
        
        (base_hold * spread_factor * urgency_factor).clamp(2.0, 60.0)
    }
    
    pub fn get_risk_manager(&self) -> Arc<RwLock<RiskManager>> {
        self.risk_manager.clone()
    }
}

/// Risk manager
pub struct RiskManager {
    limits: RiskLimits,
    positions: HashMap<String, Position>,
    daily_pnl: f64,
    daily_start: i64,
    kill_switch: bool,
}

impl RiskManager {
    pub fn new(limits: RiskLimits) -> Self {
        Self {
            limits,
            positions: HashMap::new(),
            daily_pnl: 0.0,
            daily_start: chrono::Utc::now().timestamp(),
            kill_switch: false,
        }
    }
    
    pub fn get_state(&self) -> RiskState {
        let current_notional: f64 = self.positions.values()
            .map(|p| p.size.abs() * p.mark_price)
            .sum();
        
        let daily_loss_exceeded = self.daily_pnl < -self.limits.max_loss_per_day;
        
        RiskState {
            current_notional,
            max_notional: self.limits.max_total_notional,
            daily_pnl: self.daily_pnl,
            daily_loss_limit: self.limits.max_loss_per_day,
            kill_switch_active: self.kill_switch,
            daily_loss_exceeded,
        }
    }
    
    pub fn update_position(&mut self, position: Position) {
        self.positions.insert(position.symbol.clone(), position);
    }
    
    pub fn update_pnl(&mut self, pnl_delta: f64) {
        self.daily_pnl += pnl_delta;
        
        // Reset daily PnL at midnight UTC
        let now = chrono::Utc::now().timestamp();
        if now - self.daily_start > 86400 {
            self.daily_pnl = 0.0;
            self.daily_start = now;
        }
    }
    
    pub fn activate_kill_switch(&mut self) {
        self.kill_switch = true;
        tracing::warn!("Kill switch activated!");
    }
    
    pub fn deactivate_kill_switch(&mut self) {
        self.kill_switch = false;
        tracing::info!("Kill switch deactivated");
    }
    
    pub fn check_limits(&self, symbol: &str, additional_notional: f64) -> Result<()> {
        let state = self.get_state();
        
        if state.kill_switch_active {
            return Err(Error::RiskCheck("Kill switch active".to_string()));
        }
        
        if state.daily_loss_exceeded {
            return Err(Error::RiskCheck("Daily loss limit exceeded".to_string()));
        }
        
        if state.current_notional + additional_notional > state.max_notional {
            return Err(Error::RiskCheck(format!(
                "Would exceed max notional: {:.0} + {:.0} > {:.0}",
                state.current_notional, additional_notional, state.max_notional
            )));
        }
        
        // Check per-symbol limit
        if let Some(pos) = self.positions.get(symbol) {
            let pos_notional = pos.size.abs() * pos.mark_price;
            if pos_notional + additional_notional > self.limits.max_notional_per_symbol {
                return Err(Error::RiskCheck(format!(
                    "Would exceed per-symbol limit for {}: {:.0} + {:.0} > {:.0}",
                    symbol, pos_notional, additional_notional, self.limits.max_notional_per_symbol
                )));
            }
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_gate_pass() {
        let gate = TradeGate::new(GateParams::default());
        
        let prediction = Prediction {
            timestamp_ns: 0,
            symbol: "BTC".to_string(),
            edge_bps: 15.0,
            confidence: 0.8,
            horizon_ms: 5000,
            model_version: "test".to_string(),
        };
        
        let features = FeatureVec {
            timestamp_ns: 0,
            symbol: "BTC".to_string(),
            mid_price: 50000.0,
            spread_bps: 3.0,
            ofi_1s: 0.5,
            obi_1s: 0.3,
            depth_imbalance: 0.2,
            depth_a: 0.001,
            depth_beta: 0.5,
            realized_vol_5s: 0.02,
            atr_30s: 10.0,
            funding_bps_8h: 1.0,
            impact_bps_1pct: 0.5,
            microprice: 50001.0,
            vwap_ratio: 1.001,
        };
        
        let costs = CostModel {
            taker_fee_bps: 5.0,
            maker_fee_bps: 2.0,
            maker_rebate_bps: 1.0,
            impact_bps: 2.0,
            slippage_buffer_bps: 1.0,
        };
        
        let risk = RiskState {
            current_notional: 0.0,
            max_notional: 100000.0,
            daily_pnl: 0.0,
            daily_loss_limit: 10000.0,
            kill_switch_active: false,
            daily_loss_exceeded: false,
        };
        
        let result = gate.check(&prediction, &features, &costs, &risk);
        assert!(matches!(result, GateResult::Pass { .. }));
    }
    
    #[test]
    fn test_router_decision() {
        let router = OrderRouter::new(GateParams::default(), RiskLimits::default());
        
        let prediction = Prediction {
            timestamp_ns: 0,
            symbol: "BTC".to_string(),
            edge_bps: 15.0,
            confidence: 0.8,
            horizon_ms: 5000,
            model_version: "test".to_string(),
        };
        
        let features = FeatureVec {
            timestamp_ns: 0,
            symbol: "BTC".to_string(),
            mid_price: 50000.0,
            spread_bps: 3.0,
            ofi_1s: 0.5,
            obi_1s: 0.3,
            depth_imbalance: 0.2,
            depth_a: 0.001,
            depth_beta: 0.5,
            realized_vol_5s: 0.02,
            atr_30s: 10.0,
            funding_bps_8h: 1.0,
            impact_bps_1pct: 0.5,
            microprice: 50001.0,
            vwap_ratio: 1.001,
        };
        
        let costs = CostModel {
            taker_fee_bps: 5.0,
            maker_fee_bps: 2.0,
            maker_rebate_bps: 1.0,
            impact_bps: 2.0,
            slippage_buffer_bps: 1.0,
        };
        
        let decision = router.decide(&prediction, &features, &costs);
        assert!(decision.should_trade);
        assert!(decision.size_fraction > 0.0);
    }
    
    #[test]
    fn test_risk_manager() {
        let limits = RiskLimits {
            max_notional_per_symbol: 50000.0,
            max_total_notional: 100000.0,
            max_leverage: 3.0,
            max_loss_per_day: 5000.0,
            max_position_concentration: 0.5,
        };
        
        let mut manager = RiskManager::new(limits);
        
        // Should pass
        assert!(manager.check_limits("BTC", 30000.0).is_ok());
        
        // Add position
        let position = Position {
            symbol: "BTC".to_string(),
            size: 1.0,
            entry_price: 50000.0,
            mark_price: 50000.0,
            unrealized_pnl: 0.0,
            realized_pnl: 0.0,
            leverage: 1.0,
            margin_used: 50000.0,
            liquidation_price: None,
        };
        
        manager.update_position(position);
        
        // Should reject (exceeds per-symbol limit)
        assert!(manager.check_limits("BTC", 10000.0).is_err());
        
        // Test kill switch
        manager.activate_kill_switch();
        assert!(manager.check_limits("ETH", 10000.0).is_err());
    }
}
