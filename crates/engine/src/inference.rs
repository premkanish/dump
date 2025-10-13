// crates/engine/src/inference.rs - MANDATORY models (no fallbacks)
use common::*;
use ndarray::{Array1, Array2};
use ort::{Environment, ExecutionProvider, Session, SessionBuilder, Value};
use std::path::Path;
use std::sync::Arc;
use parking_lot::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    IDEC,
    Transformer,
    GBDT,
    Edge,
}

pub struct ModelSet {
    pub idec: Arc<Session>,
    pub transformer: Arc<Session>,
    pub gbdt: Arc<Session>,
    pub edge: Arc<Session>,
}

impl ModelSet {
    pub fn load(env: &Arc<Environment>, models_dir: &Path) -> Result<Self> {
        tracing::info!("Loading models from {:?} (MANDATORY)", models_dir);
        
        let load_model = |name: &str| -> Result<Arc<Session>> {
            let path = models_dir.join(format!("{}.onnx", name));
            
            if !path.exists() {
                return Err(Error::Model(format!(
                    "Model NOT FOUND: {:?}. This is REQUIRED for operation.",
                    path
                )));
            }
            
            tracing::info!("Loading model: {:?}", path);
            
            let session = SessionBuilder::new(env)?
                .with_execution_providers([ExecutionProvider::CPU])?
                .with_optimization_level(ort::GraphOptimizationLevel::Level3)?
                .with_intra_threads(2)?
                .with_model_from_file(&path)
                .map_err(|e| Error::Model(format!(
                    "Failed to load {:?}: {}. Model file may be corrupted.",
                    path, e
                )))?;
            
            tracing::info!("✅ Loaded: {:?}", path);
            Ok(Arc::new(session))
        };
        
        // Load all models - ALL MANDATORY
        let idec = load_model("idec")?;
        let transformer = load_model("transformer")?;
        let gbdt = load_model("gbdt")?;
        let edge = load_model("edge")?;
        
        Ok(Self {
            idec,
            transformer,
            gbdt,
            edge,
        })
    }
}

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
                .map_err(|e| Error::Model(format!("ONNX environment init failed: {}", e)))?
        );
        
        Ok(Self {
            env,
            crypto: Arc::new(RwLock::new(None)),
            equity: Arc::new(RwLock::new(None)),
            timeout_ms,
        })
    }
    
    /// Load crypto models - FAILS if models missing
    pub fn load_crypto(&self, models_dir: &Path) -> Result<()> {
        let models = ModelSet::load(&self.env, models_dir)?;
        *self.crypto.write() = Some(models);
        tracing::info!("✅ Crypto models loaded and verified");
        Ok(())
    }
    
    /// Load equity models - FAILS if models missing
    pub fn load_equity(&self, models_dir: &Path) -> Result<()> {
        let models = ModelSet::load(&self.env, models_dir)?;
        *self.equity.write() = Some(models);
        tracing::info!("✅ Equity models loaded and verified");
        Ok(())
    }
    
    /// Check if crypto models are loaded
    pub fn has_crypto_models(&self) -> bool {
        self.crypto.read().is_some()
    }
    
    /// Check if equity models are loaded
    pub fn has_equity_models(&self) -> bool {
        self.equity.read().is_some()
    }
    
    /// Run inference - FAILS if models not loaded (no fallback)
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
        
        let model_set = models.as_ref().ok_or_else(|| {
            Error::Model(format!(
                "Models NOT loaded for {:?}. REQUIRED: Load models before trading.",
                category
            ))
        })?;
        
        let session = match model_type {
            ModelType::IDEC => &model_set.idec,
            ModelType::Transformer => &model_set.transformer,
            ModelType::GBDT => &model_set.gbdt,
            ModelType::Edge => &model_set.edge,
        };
        
        // Run inference with timeout - FAILS if timeout
        let prediction = tokio::time::timeout(
            std::time::Duration::from_millis(self.timeout_ms),
            self.run_inference(session.clone(), features)
        ).await.map_err(|_| {
            Error::Timeout(format!(
                "Inference timeout after {}ms. Model: {:?}. This is CRITICAL.",
                self.timeout_ms, model_type
            ))
        })??;
        
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
            return Err(Error::Timeout(format!(
                "Inference took {}ms > {}ms timeout",
                elapsed.as_millis(), self.timeout_ms
            )));
        }
        
        Ok(prediction)
    }
    
    async fn run_inference(
        &self,
        session: Arc<Session>,
        features: &Array1<f32>,
    ) -> Result<Prediction> {
        let features_owned = features.clone();
        
        let result = tokio::task::spawn_blocking(move || {
            let input_shape = vec![1, features_owned.len()];
            let input_array = Array2::from_shape_vec(
                (input_shape[0], input_shape[1]),
                features_owned.to_vec()
            ).map_err(|e| Error::Model(format!("Failed to reshape input: {}", e)))?;
            
            let input_value = Value::from_array(session.allocator(), &input_array)
                .map_err(|e| Error::Model(format!("Failed to create ONNX value: {}", e)))?;
            
            let outputs = session.run(vec![input_value])
                .map_err(|e| Error::Model(format!("Inference execution failed: {}", e)))?;
            
            let output = &outputs[0];
            let output_array: Array2<f32> = output.try_extract()
                .map_err(|e| Error::Model(format!("Failed to extract output: {}", e)))?
                .view()
                .to_owned();
            
            let edge_bps = output_array[[0, 0]] as f64;
            let confidence = output_array[[0, 1]] as f64;
            
            Ok::<_, Error>((edge_bps, confidence))
        }).await
        .map_err(|e| Error::Model(format!("Inference task failed: {}", e)))??;
        
        Ok(Prediction {
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            symbol: String::new(),
            edge_bps: result.0,
            confidence: result.1,
            horizon_ms: 5000,
            model_version: "v1.0".to_string(),
        })
    }
    
    /// Ensemble prediction - MANDATORY (no single model fallback)
    pub async fn predict_ensemble(
        &self,
        category: AssetCategory,
        features: &Array1<f32>,
    ) -> Result<Prediction> {
        let models = vec![ModelType::IDEC, ModelType::Transformer, ModelType::GBDT];
        
        let mut predictions = Vec::new();
        let mut errors = Vec::new();
        
        for model_type in models {
            match self.predict(category, features, model_type).await {
                Ok(pred) => predictions.push(pred),
                Err(e) => {
                    tracing::error!("❌ Model {:?} failed: {}", model_type, e);
                    errors.push(format!("{:?}: {}", model_type, e));
                }
            }
        }
        
        if predictions.is_empty() {
            return Err(Error::Model(format!(
                "ALL ensemble models failed. Errors: {:?}. Cannot continue.",
                errors
            )));
        }
        
        // Weighted average
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

// ❌ REMOVED: RuleBasedPredictor (no fallback)
// All predictions MUST come from ML models

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_inference_pool_creation() {
        let pool = InferencePool::new(100).unwrap();
        assert!(!pool.has_crypto_models());
        assert!(!pool.has_equity_models());
    }
    
    #[test]
    fn test_missing_models_fail() {
        let pool = InferencePool::new(100).unwrap();
        
        // Should fail when models not loaded
        let result = pollster::block_on(pool.predict(
            AssetCategory::CryptoFutures,
            &Array1::zeros(100),
            ModelType::Edge,
        ));
        
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("NOT loaded"));
    }
}