// crates/engine/src/inference.rs
use common::*;
use ndarray::{Array1, Array2};
use ort::{Environment, ExecutionProvider, Session, SessionBuilder, Value};
use std::path::Path;
use std::sync::Arc;
use parking_lot::RwLock;

/// Model types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    IDEC,        // Interpretable Deep Extreme Classifier
    Transformer, // Transformer-based price predictor
    GBDT,        // Gradient Boosted Decision Trees
    Edge,        // Edge model (simple, fast)
}

/// Model set for a category
pub struct ModelSet {
    pub idec: Arc<Session>,
    pub transformer: Arc<Session>,
    pub gbdt: Arc<Session>,
    pub edge: Arc<Session>,
}

impl ModelSet {
    pub fn load(env: &Arc<Environment>, models_dir: &Path) -> Result<Self> {
        tracing::info!("Loading models from {:?}", models_dir);
        
        let load_model = |name: &str| -> Result<Arc<Session>> {
            let path = models_dir.join(format!("{}.onnx", name));
            
            if !path.exists() {
                return Err(Error::Model(format!("Model not found: {:?}", path)));
            }
            
            let session = SessionBuilder::new(env)?
                .with_execution_providers([ExecutionProvider::CPU])?
                .with_optimization_level(ort::GraphOptimizationLevel::Level3)?
                .with_intra_threads(2)?
                .with_model_from_file(&path)
                .map_err(|e| Error::Model(format!("Failed to load model: {}", e)))?;
            
            Ok(Arc::new(session))
        };
        
        Ok(Self {
            idec: load_model("idec")?,
            transformer: load_model("transformer")?,
            gbdt: load_model("gbdt")?,
            edge: load_model("edge")?,
        })
    }
}

/// Inference pool managing crypto and equity models
pub struct InferencePool {
    env: Arc<Environment>,
    pub crypto: Arc<RwLock<Option<ModelSet>>>,
    pub equity: Arc<RwLock<Option<ModelSet>>>,
    timeout_ms: u64,
}

impl InferencePool {
    pub fn new(timeout_ms: u64) -> Result<Self> {
        let env = Arc::new(
            Environment::builder()
                .with_name("hft_inference")
                .build()
                .map_err(|e| Error::Model(format!("Failed to create ONNX environment: {}", e)))?
        );
        
        Ok(Self {
            env,
            crypto: Arc::new(RwLock::new(None)),
            equity: Arc::new(RwLock::new(None)),
            timeout_ms,
        })
    }
    
    /// Load crypto models
    pub fn load_crypto(&self, models_dir: &Path) -> Result<()> {
        let models = ModelSet::load(&self.env, models_dir)?;
        *self.crypto.write() = Some(models);
        tracing::info!("Crypto models loaded");
        Ok(())
    }
    
    /// Load equity models
    pub fn load_equity(&self, models_dir: &Path) -> Result<()> {
        let models = ModelSet::load(&self.env, models_dir)?;
        *self.equity.write() = Some(models);
        tracing::info!("Equity models loaded");
        Ok(())
    }
    
    /// Run inference with timeout
    pub async fn predict(
        &self,
        category: AssetCategory,
        features: &Array1<f32>,
        model_type: ModelType,
    ) -> Result<Prediction> {
        let start = std::time::Instant::now();
        
        let models = match category {
            AssetCategory::CryptoFutures => self.crypto.read(),
            AssetCategory::Equity => self.equity.read(),
        };
        
        let model_set = models.as_ref()
            .ok_or_else(|| Error::Model(format!("Models not loaded for {:?}", category)))?;
        
        let session = match model_type {
            ModelType::IDEC => &model_set.idec,
            ModelType::Transformer => &model_set.transformer,
            ModelType::GBDT => &model_set.gbdt,
            ModelType::Edge => &model_set.edge,
        };
        
        // Run inference with timeout
        let prediction = tokio::time::timeout(
            std::time::Duration::from_millis(self.timeout_ms),
            self.run_inference(session.clone(), features)
        ).await
        .map_err(|_| Error::Timeout(format!("Inference timeout after {}ms", self.timeout_ms)))??;
        
        let elapsed = start.elapsed();
        metrics::histogram!("inference_duration_us", elapsed.as_micros() as f64, 
            "category" => format!("{:?}", category),
            "model" => format!("{:?}", model_type)
        );
        
        if elapsed.as_millis() > self.timeout_ms as u128 {
            metrics::increment_counter!("inference_timeout",
                "category" => format!("{:?}", category),
                "model" => format!("{:?}", model_type)
            );
        }
        
        Ok(prediction)
    }
    
    async fn run_inference(
        &self,
        session: Arc<Session>,
        features: &Array1<f32>,
    ) -> Result<Prediction> {
        // Spawn blocking task for ONNX inference
        let features_owned = features.clone();
        
        let result = tokio::task::spawn_blocking(move || {
            // Reshape to (1, N) for batch inference
            let input_shape = vec![1, features_owned.len()];
            let input_array = Array2::from_shape_vec(
                (input_shape[0], input_shape[1]),
                features_owned.to_vec()
            ).map_err(|e| Error::Model(format!("Failed to reshape input: {}", e)))?;
            
            // Create ONNX value
            let input_value = Value::from_array(session.allocator(), &input_array)
                .map_err(|e| Error::Model(format!("Failed to create ONNX value: {}", e)))?;
            
            // Run inference
            let outputs = session.run(vec![input_value])
                .map_err(|e| Error::Model(format!("Inference failed: {}", e)))?;
            
            // Extract prediction
            let output = &outputs[0];
            let output_array: Array2<f32> = output.try_extract()
                .map_err(|e| Error::Model(format!("Failed to extract output: {}", e)))?
                .view()
                .to_owned();
            
            // Assuming output shape is (1, 2) -> [edge_bps, confidence]
            let edge_bps = output_array[[0, 0]] as f64;
            let confidence = output_array[[0, 1]] as f64;
            
            Ok::<_, Error>((edge_bps, confidence))
        }).await
        .map_err(|e| Error::Model(format!("Inference task failed: {}", e)))??;
        
        Ok(Prediction {
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            symbol: String::new(), // Set by caller
            edge_bps: result.0,
            confidence: result.1,
            horizon_ms: 5000, // 5 second horizon
            model_version: "v1.0".to_string(),
        })
    }
    
    /// Ensemble prediction from multiple models
    pub async fn predict_ensemble(
        &self,
        category: AssetCategory,
        features: &Array1<f32>,
    ) -> Result<Prediction> {
        let models = vec![ModelType::IDEC, ModelType::Transformer, ModelType::GBDT];
        
        let mut predictions = Vec::new();
        for model_type in models {
            match self.predict(category, features, model_type).await {
                Ok(pred) => predictions.push(pred),
                Err(e) => {
                    tracing::warn!("Model {:?} failed: {}", model_type, e);
                }
            }
        }
        
        if predictions.is_empty() {
            return Err(Error::Model("All models failed".to_string()));
        }
        
        // Weighted average by confidence
        let total_confidence: f64 = predictions.iter().map(|p| p.confidence).sum();
        let weighted_edge: f64 = predictions.iter()
            .map(|p| p.edge_bps * p.confidence / total_confidence)
            .sum();
        
        Ok(Prediction {
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            symbol: String::new(),
            edge_bps: weighted_edge,
            confidence: total_confidence / predictions.len() as f64,
            horizon_ms: 5000,
            model_version: "ensemble-v1.0".to_string(),
        })
    }
}

/// Fallback rule-based predictor when models unavailable
pub struct RuleBasedPredictor;

impl RuleBasedPredictor {
    pub fn predict(features: &FeatureVec) -> Prediction {
        // Simple momentum + mean reversion hybrid
        let momentum_signal = features.vwap_ratio - 1.0;
        let spread_penalty = -features.spread_bps / 10.0;
        let ofi_signal = features.ofi_1s * 2.0;
        
        let edge_bps = momentum_signal * 5.0 + ofi_signal + spread_penalty;
        
        Prediction {
            timestamp_ns: features.timestamp_ns,
            symbol: features.symbol.clone(),
            edge_bps: edge_bps.clamp(-20.0, 20.0),
            confidence: 0.3, // Low confidence for rule-based
            horizon_ms: 5000,
            model_version: "rule-based-v1.0".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_inference_pool_creation() {
        let pool = InferencePool::new(100).unwrap();
        assert!(pool.crypto.read().is_none());
        assert!(pool.equity.read().is_none());
    }
    
    #[test]
    fn test_rule_based_predictor() {
        let features = FeatureVec {
            timestamp_ns: 0,
            symbol: "BTC".to_string(),
            mid_price: 50000.0,
            spread_bps: 2.0,
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
        
        let pred = RuleBasedPredictor::predict(&features);
        assert!(pred.edge_bps.abs() <= 20.0);
        assert!(pred.confidence > 0.0 && pred.confidence < 1.0);
    }
}