// crates/engine/src/rl_agent.rs
use common::*;
use ndarray::{Array1, Array2};
use ort::{Session, Value};
use std::sync::Arc;
use parking_lot::RwLock;

/// RL Agent for trading decisions
pub struct RLAgent {
    actor: Arc<Session>,
    critic: Option<Arc<Session>>,
    config: RLAgentConfig,
    state_buffer: Arc<RwLock<StateBuffer>>,
}

#[derive(Debug, Clone)]
pub struct RLAgentConfig {
    pub action_type: ActionType,
    pub sequence_length: usize,
    pub use_recurrent: bool,
    pub epsilon: f64,  // Exploration (0.0 in production)
    pub temperature: f64,  // Softmax temperature
}

#[derive(Debug, Clone, Copy)]
pub enum ActionType {
    Discrete,      // [0=Hold, 1=Buy, 2=Sell]
    Continuous,    // [-1.0, 1.0] position size
    MultiDiscrete, // [style: 3, size: 5, duration: 4]
}

struct StateBuffer {
    states: Vec<Vec<f32>>,
    max_length: usize,
}

impl RLAgent {
    pub fn new(actor_path: &str, critic_path: Option<&str>, config: RLAgentConfig) -> Result<Self> {
        let actor = Session::builder()?
            .with_optimization_level(ort::GraphOptimizationLevel::Level3)?
            .with_intra_threads(2)?
            .commit_from_file(actor_path)?;
        
        let critic = if let Some(path) = critic_path {
            Some(Arc::new(
                Session::builder()?
                    .commit_from_file(path)?
            ))
        } else {
            None
        };
        
        tracing::info!("RL Agent loaded: {:?}", config.action_type);
        
        Ok(Self {
            actor: Arc::new(actor),
            critic,
            config,
            state_buffer: Arc::new(RwLock::new(StateBuffer {
                states: Vec::new(),
                max_length: config.sequence_length,
            })),
        })
    }
    
    /// Get action from current state
    pub fn get_action(
        &self,
        features: &Array1<f32>,
        market_state: &MarketState,
    ) -> Result<RLAction> {
        // Build state vector
        let state = self.build_state(features, market_state);
        
        // Update state buffer for recurrent models
        if self.config.use_recurrent {
            let mut buffer = self.state_buffer.write();
            buffer.states.push(state.clone());
            if buffer.states.len() > buffer.max_length {
                buffer.states.remove(0);
            }
        }
        
        // Prepare input for actor
        let input = if self.config.use_recurrent {
            self.prepare_sequence_input()?
        } else {
            self.prepare_single_input(&state)?
        };
        
        // Run inference
        let outputs = self.actor.run(vec![input])?;
        let action_logits = outputs[0].try_extract_raw_tensor::<f32>()?;
        
        // Sample action
        let action = match self.config.action_type {
            ActionType::Discrete => self.sample_discrete(action_logits),
            ActionType::Continuous => self.sample_continuous(action_logits),
            ActionType::MultiDiscrete => self.sample_multi_discrete(action_logits),
        }?;
        
        // Get value estimate if critic available
        let value = if let Some(critic) = &self.critic {
            let value_output = critic.run(vec![input])?;
            value_output[0].try_extract_raw_tensor::<f32>()?[0]
        } else {
            0.0
        };
        
        Ok(RLAction {
            action,
            value,
            confidence: self.compute_confidence(action_logits),
        })
    }
    
    fn build_state(&self, features: &Array1<f32>, market: &MarketState) -> Vec<f32> {
        let mut state = features.to_vec();
        
        // Add market state
        state.push(market.position_size as f32);
        state.push(market.unrealized_pnl as f32 / 10000.0); // Normalize
        state.push(market.holding_duration_s as f32 / 60.0);
        state.push(market.inventory_risk as f32);
        
        state
    }
    
    fn prepare_single_input(&self, state: &[f32]) -> Result<Value> {
        let array = Array2::from_shape_vec((1, state.len()), state.to_vec())?;
        Ok(Value::from_array(array)?)
    }
    
    fn prepare_sequence_input(&self) -> Result<Value> {
        let buffer = self.state_buffer.read();
        let seq_len = buffer.states.len();
        let state_dim = buffer.states[0].len();
        
        let mut flat = Vec::with_capacity(seq_len * state_dim);
        for state in &buffer.states {
            flat.extend_from_slice(state);
        }
        
        let array = Array2::from_shape_vec((1, seq_len * state_dim), flat)?;
        Ok(Value::from_array(array)?)
    }
    
    fn sample_discrete(&self, logits: &[f32]) -> Result<Action> {
        let probs = softmax(logits, self.config.temperature);
        
        let action_idx = if self.config.epsilon > 0.0 && rand::random::<f64>() < self.config.epsilon {
            rand::random::<usize>() % probs.len()
        } else {
            probs.iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
                .map(|(i, _)| i)
                .unwrap()
        };
        
        Ok(Action::Discrete(action_idx))
    }
    
    fn sample_continuous(&self, logits: &[f32]) -> Result<Action> {
        // Assume logits = [mean, log_std]
        let mean = logits[0];
        let std = logits[1].exp().clamp(0.01, 1.0);
        
        let value = if self.config.epsilon > 0.0 {
            use rand_distr::{Normal, Distribution};
            let normal = Normal::new(mean as f64, std as f64).unwrap();
            normal.sample(&mut rand::thread_rng()) as f32
        } else {
            mean
        };
        
        Ok(Action::Continuous(value.clamp(-1.0, 1.0)))
    }
    
    fn sample_multi_discrete(&self, logits: &[f32]) -> Result<Action> {
        // Assume logits split into: [style:3, size:5, duration:4]
        let style_logits = &logits[0..3];
        let size_logits = &logits[3..8];
        let duration_logits = &logits[8..12];
        
        let style = argmax(&softmax(style_logits, 1.0));
        let size = argmax(&softmax(size_logits, 1.0));
        let duration = argmax(&softmax(duration_logits, 1.0));
        
        Ok(Action::MultiDiscrete { style, size, duration })
    }
    
    fn compute_confidence(&self, logits: &[f32]) -> f64 {
        let probs = softmax(logits, 1.0);
        let max_prob = probs.iter().fold(0.0f32, |a, &b| a.max(b));
        max_prob as f64
    }
    
    /// Convert RL action to trading decision
    pub fn to_route_decision(
        &self,
        action: &RLAction,
        features: &FeatureVec,
    ) -> RouteDecision {
        match &action.action {
            Action::Discrete(idx) => {
                // 0=Hold, 1=Buy, 2=Sell
                let should_trade = *idx != 0;
                let side = if *idx == 1 { Side::Buy } else { Side::Sell };
                
                RouteDecision {
                    style: OrderStyle::MakerPassive,
                    size_fraction: if should_trade { 0.02 } else { 0.0 },
                    hold_duration_s: 30.0,
                    urgency: action.confidence,
                    should_trade,
                    reason: format!("RL action: {}", idx),
                }
            }
            
            Action::Continuous(size) => {
                let should_trade = size.abs() > 0.01;
                
                RouteDecision {
                    style: if size.abs() > 0.5 {
                        OrderStyle::TakerNow
                    } else {
                        OrderStyle::MakerPassive
                    },
                    size_fraction: size.abs() as f64 * 0.1,
                    hold_duration_s: 30.0,
                    urgency: action.confidence,
                    should_trade,
                    reason: format!("RL size: {:.3}", size),
                }
            }
            
            Action::MultiDiscrete { style, size, duration } => {
                let order_style = match style {
                    0 => OrderStyle::MakerPassive,
                    1 => OrderStyle::TakerNow,
                    _ => OrderStyle::Sniper,
                };
                
                let size_fraction = (*size as f64 + 1.0) * 0.01; // 1-5 -> 0.02-0.06
                let hold_duration = (*duration as f64 + 1.0) * 10.0; // 10-40s
                
                RouteDecision {
                    style: order_style,
                    size_fraction,
                    hold_duration_s: hold_duration,
                    urgency: action.confidence,
                    should_trade: *size > 0,
                    reason: format!("RL multi: s{} sz{} d{}", style, size, duration),
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RLAction {
    pub action: Action,
    pub value: f32,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub enum Action {
    Discrete(usize),
    Continuous(f32),
    MultiDiscrete { style: usize, size: usize, duration: usize },
}

#[derive(Debug, Clone)]
pub struct MarketState {
    pub position_size: f64,
    pub unrealized_pnl: f64,
    pub holding_duration_s: f64,
    pub inventory_risk: f64,
}

fn softmax(logits: &[f32], temperature: f64) -> Vec<f32> {
    let max = logits.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    let exp: Vec<f32> = logits.iter()
        .map(|&x| ((x - max) as f64 / temperature).exp() as f32)
        .collect();
    let sum: f32 = exp.iter().sum();
    exp.iter().map(|&x| x / sum).collect()
}

fn argmax(probs: &[f32]) -> usize {
    probs.iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .map(|(i, _)| i)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_softmax() {
        let logits = vec![1.0, 2.0, 3.0];
        let probs = softmax(&logits, 1.0);
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }
}