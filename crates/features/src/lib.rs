// crates/features/src/lib.rs - GPU-First Feature Engineering
use common::*;
use ndarray::Array1;
use std::sync::Arc;
use parking_lot::RwLock;

pub mod gpu;
pub mod cpu;
pub mod indicators;

pub use gpu::{GpuFeatureComputer, DeviceType};
pub use cpu::CpuFeatureBuilder;

/// Unified feature computer with automatic GPU/CPU fallback
pub struct FeatureComputer {
    gpu: Option<Arc<GpuFeatureComputer>>,
    cpu: Arc<RwLock<CpuFeatureBuilder>>,
    mode: ComputeMode,
}

#[derive(Debug, Clone, Copy)]
pub enum ComputeMode {
    /// Try GPU first, fallback to CPU on error
    GPUFirst,
    /// CPU only (for debugging)
    CPUOnly,
    /// GPU only (fail if GPU unavailable)
    GPUOnly,
}

impl FeatureComputer {
    /// Create new feature computer with GPU support
    pub fn new(device: DeviceType, batch_size: usize) -> Result<Self> {
        let gpu = match GpuFeatureComputer::new(device, batch_size) {
            Ok(computer) => {
                tracing::info!("GPU feature computer initialized: {:?}", device);
                Some(Arc::new(computer))
            }
            Err(e) => {
                tracing::warn!("GPU initialization failed: {}. Using CPU fallback", e);
                None
            }
        };
        
        let mode = if gpu.is_some() {
            ComputeMode::GPUFirst
        } else {
            ComputeMode::CPUOnly
        };
        
        Ok(Self {
            gpu,
            cpu: Arc::new(RwLock::new(CpuFeatureBuilder::new())),
            mode,
        })
    }
    
    /// Create CPU-only computer
    pub fn cpu_only() -> Self {
        Self {
            gpu: None,
            cpu: Arc::new(RwLock::new(CpuFeatureBuilder::new())),
            mode: ComputeMode::CPUOnly,
        }
    }
    
    /// Compute features for a batch of market snapshots
    pub fn compute_batch(
        &self,
        snapshots: &[MarketSnapshot],
    ) -> Result<Vec<ComputedFeatures>> {
        let start = std::time::Instant::now();
        
        let result = match self.mode {
            ComputeMode::CPUOnly => self.compute_cpu(snapshots),
            
            ComputeMode::GPUOnly => {
                let gpu = self.gpu.as_ref()
                    .ok_or_else(|| Error::Internal("GPU not available".to_string()))?;
                gpu.compute_batch(snapshots)
            }
            
            ComputeMode::GPUFirst => {
                if let Some(gpu) = &self.gpu {
                    match gpu.compute_batch(snapshots) {
                        Ok(features) => Ok(features),
                        Err(e) => {
                            tracing::warn!("GPU compute failed: {}. Falling back to CPU", e);
                            metrics::increment_counter!("gpu_fallback_total");
                            self.compute_cpu(snapshots)
                        }
                    }
                } else {
                    self.compute_cpu(snapshots)
                }
            }
        };
        
        let elapsed = start.elapsed();
        metrics::histogram!("feature_compute_us", elapsed.as_micros() as f64,
            "mode" => format!("{:?}", self.mode)
        );
        
        result
    }
    
    fn compute_cpu(&self, snapshots: &[MarketSnapshot]) -> Result<Vec<ComputedFeatures>> {
        let builder = self.cpu.read();
        builder.compute_batch(snapshots)
    }
    
    /// Add symbol to track
    pub fn add_symbol(&self, symbol: String, window_size: usize) {
        self.cpu.write().add_symbol(symbol, window_size);
    }
    
    /// Update with new order book
    pub fn update_book(&self, orderbook: &OrderBook) {
        self.cpu.write().update_book(orderbook);
    }
}

/// Computed features with metadata
#[derive(Debug, Clone)]
pub struct ComputedFeatures {
    pub symbol: String,
    pub timestamp_ns: i64,
    pub features: Array1<f32>,
    pub computed_on: Device,
}

#[derive(Debug, Clone, Copy)]
pub enum Device {
    CPU,
    CUDA,
    ROCm,
    TensorRT,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_feature_computer_cpu() {
        let computer = FeatureComputer::cpu_only();
        // Test basic functionality
    }
}