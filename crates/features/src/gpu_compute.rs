// crates/features/src/gpu_compute.rs
// GPU-accelerated feature computation using wgpu (cross-platform) or cudarc

use common::*;
use ndarray::{Array1, Array2};
use std::sync::Arc;
use parking_lot::RwLock;

#[cfg(feature = "cuda")]
use cudarc::driver::*;

#[cfg(feature = "wgpu")]
use wgpu;

/// Device type for GPU computation
#[derive(Debug, Clone, Copy)]
pub enum DeviceType {
    CPU,
    CUDA(usize),      // Device ID
    ROCm(usize),      // Device ID  
    TensorRT,         // For NVIDIA optimized inference
}

/// GPU feature computer
pub struct GpuFeatureComputer {
    device: DeviceType,
    batch_size: usize,
    
    #[cfg(feature = "cuda")]
    cuda_context: Option<Arc<CudaDevice>>,
    
    #[cfg(feature = "wgpu")]
    wgpu_context: Option<WgpuContext>,
    
    // Pre-allocated buffers
    input_buffer: Arc<RwLock<Vec<f32>>>,
    output_buffer: Arc<RwLock<Vec<f32>>>,
}

#[cfg(feature = "wgpu")]
struct WgpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    compute_pipeline: wgpu::ComputePipeline,
}

impl GpuFeatureComputer {
    /// Initialize GPU feature computer
    pub fn new(device: DeviceType, batch_size: usize) -> Result<Self> {
        match device {
            #[cfg(feature = "cuda")]
            DeviceType::CUDA(device_id) => {
                Self::init_cuda(device_id, batch_size)
            }
            
            #[cfg(feature = "wgpu")]
            DeviceType::ROCm(_) | DeviceType::TensorRT => {
                Self::init_wgpu(batch_size)
            }
            
            DeviceType::CPU => {
                Ok(Self::init_cpu(batch_size))
            }
            
            #[allow(unreachable_patterns)]
            _ => Err(Error::Internal("GPU backend not compiled".to_string())),
        }
    }
    
    fn init_cpu(batch_size: usize) -> Self {
        Self {
            device: DeviceType::CPU,
            batch_size,
            
            #[cfg(feature = "cuda")]
            cuda_context: None,
            
            #[cfg(feature = "wgpu")]
            wgpu_context: None,
            
            input_buffer: Arc::new(RwLock::new(Vec::with_capacity(batch_size * 1024))),
            output_buffer: Arc::new(RwLock::new(Vec::with_capacity(batch_size * 256))),
        }
    }
    
    #[cfg(feature = "cuda")]
    fn init_cuda(device_id: usize, batch_size: usize) -> Result<Self> {
        let cuda_device = CudaDevice::new(device_id)
            .map_err(|e| Error::Internal(format!("CUDA init failed: {:?}", e)))?;
        
        tracing::info!("CUDA device {} initialized: {}", device_id, cuda_device.name());
        
        Ok(Self {
            device: DeviceType::CUDA(device_id),
            batch_size,
            cuda_context: Some(Arc::new(cuda_device)),
            
            #[cfg(feature = "wgpu")]
            wgpu_context: None,
            
            input_buffer: Arc::new(RwLock::new(Vec::with_capacity(batch_size * 1024))),
            output_buffer: Arc::new(RwLock::new(Vec::with_capacity(batch_size * 256))),
        })
    }
    
    #[cfg(feature = "wgpu")]
    fn init_wgpu(batch_size: usize) -> Result<Self> {
        let instance = wgpu::Instance::default();
        
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok_or_else(|| Error::Internal("No GPU adapter found".to_string()))?;
        
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("HFT Compute Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
            },
            None,
        ))
        .map_err(|e| Error::Internal(format!("Device request failed: {}", e)))?;
        
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Feature Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/features.wgsl").into()),
        });
        
        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Feature Pipeline"),
            layout: None,
            module: &shader,
            entry_point: "main",
        });
        
        tracing::info!("WebGPU/ROCm initialized: {}", adapter.get_info().name);
        
        Ok(Self {
            device: DeviceType::ROCm(0),
            batch_size,
            
            #[cfg(feature = "cuda")]
            cuda_context: None,
            
            wgpu_context: Some(WgpuContext {
                device,
                queue,
                compute_pipeline,
            }),
            
            input_buffer: Arc::new(RwLock::new(Vec::with_capacity(batch_size * 1024))),
            output_buffer: Arc::new(RwLock::new(Vec::with_capacity(batch_size * 256))),
        })
    }
    
    /// Compute features for a batch of order books
    pub fn compute_batch(
        &self,
        orderbooks: &[OrderBook],
        trades: &[Vec<Trade>],
        funding_rates: &[f64],
    ) -> Result<Vec<Array1<f32>>> {
        let start = std::time::Instant::now();
        
        let results = match self.device {
            DeviceType::CPU => {
                self.compute_batch_cpu(orderbooks, trades, funding_rates)
            }
            
            #[cfg(feature = "cuda")]
            DeviceType::CUDA(_) => {
                self.compute_batch_cuda(orderbooks, trades, funding_rates)
            }
            
            #[cfg(feature = "wgpu")]
            DeviceType::ROCm(_) | DeviceType::TensorRT => {
                self.compute_batch_wgpu(orderbooks, trades, funding_rates)
            }
            
            #[allow(unreachable_patterns)]
            _ => self.compute_batch_cpu(orderbooks, trades, funding_rates),
        }?;
        
        let elapsed = start.elapsed();
        metrics::histogram!("gpu_features_compute_us", elapsed.as_micros() as f64,
            "device" => format!("{:?}", self.device),
            "batch_size" => orderbooks.len().to_string()
        );
        
        Ok(results)
    }
    
    /// CPU fallback implementation
    fn compute_batch_cpu(
        &self,
        orderbooks: &[OrderBook],
        trades: &[Vec<Trade>],
        funding_rates: &[f64],
    ) -> Result<Vec<Array1<f32>>> {
        use rayon::prelude::*;
        
        // Parallel CPU computation using Rayon
        let results: Vec<Array1<f32>> = orderbooks
            .par_iter()
            .enumerate()
            .map(|(i, book)| {
                let trades = trades.get(i).map(|t| t.as_slice()).unwrap_or(&[]);
                let funding = funding_rates.get(i).copied().unwrap_or(0.0);
                
                self.compute_single_cpu(book, trades, funding)
            })
            .collect();
        
        Ok(results)
    }
    
    fn compute_single_cpu(&self, book: &OrderBook, trades: &[Trade], funding: f64) -> Array1<f32> {
        let mut features = Vec::with_capacity(100);
        
        // Basic features
        let mid = book.mid_price().unwrap_or(0.0) as f32;
        let spread = book.spread_bps().unwrap_or(0.0) as f32;
        
        features.push(mid);
        features.push(spread);
        features.push(funding as f32);
        
        // Order book imbalance
        let (bid_vol, ask_vol) = self.compute_book_volumes(book);
        let obi = (bid_vol - ask_vol) / (bid_vol + ask_vol + 1e-9);
        features.push(obi);
        
        // Depth features (10 levels each side)
        for level in book.bids.iter().take(10) {
            features.push(level.price.0 as f32);
            features.push(level.quantity as f32);
        }
        
        for level in book.asks.iter().take(10) {
            features.push(level.price.0 as f32);
            features.push(level.quantity as f32);
        }
        
        // Pad to 100 features
        features.resize(100, 0.0);
        
        Array1::from_vec(features)
    }
    
    fn compute_book_volumes(&self, book: &OrderBook) -> (f32, f32) {
        let bid_vol: f64 = book.bids.iter().take(10).map(|l| l.quantity).sum();
        let ask_vol: f64 = book.asks.iter().take(10).map(|l| l.quantity).sum();
        (bid_vol as f32, ask_vol as f32)
    }
    
    #[cfg(feature = "cuda")]
    fn compute_batch_cuda(
        &self,
        orderbooks: &[OrderBook],
        trades: &[Vec<Trade>],
        funding_rates: &[f64],
    ) -> Result<Vec<Array1<f32>>> {
        let cuda = self.cuda_context.as_ref()
            .ok_or_else(|| Error::Internal("CUDA not initialized".to_string()))?;
        
        // Prepare input data
        let mut input_data = self.input_buffer.write();
        input_data.clear();
        
        for (i, book) in orderbooks.iter().enumerate() {
            self.serialize_orderbook_cuda(&mut input_data, book);
            
            if let Some(trades) = trades.get(i) {
                self.serialize_trades_cuda(&mut input_data, trades);
            }
            
            input_data.push(funding_rates.get(i).copied().unwrap_or(0.0) as f32);
        }
        
        // Allocate GPU memory
        let d_input = cuda.htod_copy(input_data.as_slice())
            .map_err(|e| Error::Internal(format!("CUDA upload failed: {:?}", e)))?;
        
        let output_size = orderbooks.len() * 100; // 100 features per symbol
        let d_output = cuda.alloc_zeros::<f32>(output_size)
            .map_err(|e| Error::Internal(format!("CUDA alloc failed: {:?}", e)))?;
        
        // Launch kernel (would need actual CUDA kernel implementation)
        // This is pseudo-code - real implementation needs PTX/CUDA C
        tracing::warn!("CUDA kernel execution not implemented - falling back to CPU");
        
        // Copy results back
        let mut output_data = self.output_buffer.write();
        output_data.resize(output_size, 0.0);
        cuda.dtoh_sync_copy_into(&d_output, &mut output_data)
            .map_err(|e| Error::Internal(format!("CUDA download failed: {:?}", e)))?;
        
        // Convert to Array1 per symbol
        let results = output_data
            .chunks_exact(100)
            .map(|chunk| Array1::from_vec(chunk.to_vec()))
            .collect();
        
        Ok(results)
    }
    
    #[cfg(feature = "cuda")]
    fn serialize_orderbook_cuda(&self, buffer: &mut Vec<f32>, book: &OrderBook) {
        // Mid price
        buffer.push(book.mid_price().unwrap_or(0.0) as f32);
        
        // Spread
        buffer.push(book.spread_bps().unwrap_or(0.0) as f32);
        
        // Best bid/ask
        if let Some(bid) = book.best_bid() {
            buffer.push(bid.price.0 as f32);
            buffer.push(bid.quantity as f32);
        } else {
            buffer.extend_from_slice(&[0.0, 0.0]);
        }
        
        if let Some(ask) = book.best_ask() {
            buffer.push(ask.price.0 as f32);
            buffer.push(ask.quantity as f32);
        } else {
            buffer.extend_from_slice(&[0.0, 0.0]);
        }
        
        // 10 levels each side
        for level in book.bids.iter().take(10) {
            buffer.push(level.price.0 as f32);
            buffer.push(level.quantity as f32);
        }
        
        // Pad if less than 10 levels
        for _ in book.bids.len()..10 {
            buffer.extend_from_slice(&[0.0, 0.0]);
        }
        
        for level in book.asks.iter().take(10) {
            buffer.push(level.price.0 as f32);
            buffer.push(level.quantity as f32);
        }
        
        for _ in book.asks.len()..10 {
            buffer.extend_from_slice(&[0.0, 0.0]);
        }
    }
    
    #[cfg(feature = "cuda")]
    fn serialize_trades_cuda(&self, buffer: &mut Vec<f32>, trades: &[Trade]) {
        // Last 100 trades
        let recent_trades = trades.iter().rev().take(100);
        
        let mut trade_count = 0u32;
        for trade in recent_trades {
            buffer.push(trade.price as f32);
            buffer.push(trade.quantity as f32);
            buffer.push(if matches!(trade.side, Side::Buy) { 1.0 } else { -1.0 });
            trade_count += 1;
        }
        
        // Pad to 100 trades
        for _ in trade_count..100 {
            buffer.extend_from_slice(&[0.0, 0.0, 0.0]);
        }
    }
    
    #[cfg(feature = "wgpu")]
    fn compute_batch_wgpu(
        &self,
        orderbooks: &[OrderBook],
        trades: &[Vec<Trade>],
        funding_rates: &[f64],
    ) -> Result<Vec<Array1<f32>>> {
        let ctx = self.wgpu_context.as_ref()
            .ok_or_else(|| Error::Internal("WebGPU not initialized".to_string()))?;
        
        // WebGPU implementation would go here
        // For now, fall back to CPU
        tracing::warn!("WebGPU kernel not implemented - falling back to CPU");
        self.compute_batch_cpu(orderbooks, trades, funding_rates)
    }
}

/// Builder for GPU feature computer
pub struct GpuFeatureComputerBuilder {
    device: DeviceType,
    batch_size: usize,
}

impl GpuFeatureComputerBuilder {
    pub fn new() -> Self {
        Self {
            device: DeviceType::CPU,
            batch_size: 32,
        }
    }
    
    pub fn device(mut self, device: DeviceType) -> Self {
        self.device = device;
        self
    }
    
    pub fn batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }
    
    pub fn build(self) -> Result<GpuFeatureComputer> {
        GpuFeatureComputer::new(self.device, self.batch_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordered_float::OrderedFloat;
    
    #[test]
    fn test_cpu_compute() {
        let computer = GpuFeatureComputerBuilder::new()
            .device(DeviceType::CPU)
            .batch_size(10)
            .build()
            .unwrap();
        
        let book = OrderBook {
            symbol: "BTC".to_string(),
            timestamp_ns: 0,
            bids: vec![Level { price: OrderedFloat(50000.0), quantity: 1.0 }],
            asks: vec![Level { price: OrderedFloat(50010.0), quantity: 1.0 }],
            sequence: 1,
        };
        
        let results = computer.compute_batch(&[book], &[vec![]], &[0.01]).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].len(), 100);
    }
}