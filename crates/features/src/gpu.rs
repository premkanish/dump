// crates/features/src/gpu.rs
use common::*;
use ndarray::Array1;
use std::sync::Arc;
use parking_lot::Mutex;

#[derive(Debug, Clone, Copy)]
pub enum DeviceType {
    CPU,
    CUDA(usize),
    ROCm(usize),
    TensorRT,
}

pub struct GpuFeatureComputer {
    device: DeviceType,
    batch_size: usize,
    #[cfg(feature = "cuda")]
    cuda: Option<CudaBackend>,
    #[cfg(feature = "wgpu")]
    wgpu: Option<WgpuBackend>,
}

#[cfg(feature = "cuda")]
struct CudaBackend {
    device: Arc<cudarc::driver::CudaDevice>,
    kernel: cudarc::driver::CudaFunction,
}

#[cfg(feature = "wgpu")]
struct WgpuBackend {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
}

impl GpuFeatureComputer {
    pub fn new(device: DeviceType, batch_size: usize) -> Result<Self> {
        match device {
            #[cfg(feature = "cuda")]
            DeviceType::CUDA(id) => Self::new_cuda(id, batch_size),
            
            #[cfg(feature = "wgpu")]
            DeviceType::ROCm(id) => Self::new_wgpu(batch_size),
            
            _ => Err(Error::Internal("GPU backend not compiled".to_string())),
        }
    }
    
    #[cfg(feature = "cuda")]
    fn new_cuda(device_id: usize, batch_size: usize) -> Result<Self> {
        use cudarc::driver::*;
        
        let device = CudaDevice::new(device_id)
            .map_err(|e| Error::Internal(format!("CUDA init: {:?}", e)))?;
        
        // Compile CUDA kernel
        let ptx = compile_cuda_kernel();
        let kernel = device.load_ptx(ptx, "features", &["compute_features"])
            .map_err(|e| Error::Internal(format!("Kernel load: {:?}", e)))?;
        
        tracing::info!("CUDA device {} ready: {}", device_id, device.name());
        
        Ok(Self {
            device: DeviceType::CUDA(device_id),
            batch_size,
            cuda: Some(CudaBackend {
                device: Arc::new(device),
                kernel,
            }),
            #[cfg(feature = "wgpu")]
            wgpu: None,
        })
    }
    
    #[cfg(feature = "wgpu")]
    fn new_wgpu(batch_size: usize) -> Result<Self> {
        // WebGPU initialization for AMD/Intel/Apple
        let instance = wgpu::Instance::default();
        
        let adapter = pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            }
        )).ok_or_else(|| Error::Internal("No GPU".to_string()))?;
        
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor::default(),
            None,
        )).map_err(|e| Error::Internal(format!("Device: {}", e)))?;
        
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("features"),
            source: wgpu::ShaderSource::Wgsl(WGSL_SHADER.into()),
        });
        
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("features"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        
        Ok(Self {
            device: DeviceType::ROCm(0),
            batch_size,
            #[cfg(feature = "cuda")]
            cuda: None,
            wgpu: Some(WgpuBackend { device, queue, pipeline }),
        })
    }
    
    /// Compute features for batch
    pub fn compute_batch(&self, snapshots: &[MarketSnapshot]) -> Result<Vec<crate::ComputedFeatures>> {
        #[cfg(feature = "cuda")]
        if let Some(cuda) = &self.cuda {
            return self.compute_cuda(cuda, snapshots);
        }
        
        #[cfg(feature = "wgpu")]
        if let Some(wgpu) = &self.wgpu {
            return self.compute_wgpu(wgpu, snapshots);
        }
        
        Err(Error::Internal("No GPU backend".to_string()))
    }
    
    #[cfg(feature = "cuda")]
    fn compute_cuda(&self, backend: &CudaBackend, snapshots: &[MarketSnapshot]) -> Result<Vec<crate::ComputedFeatures>> {
        use cudarc::driver::*;
        
        let n = snapshots.len();
        let features_per_symbol = 100;
        
        // Prepare input data
        let mut input = Vec::with_capacity(n * 1024);
        for snap in snapshots {
            serialize_snapshot(&mut input, snap);
        }
        
        // Allocate GPU memory
        let d_input = backend.device.htod_copy(&input)
            .map_err(|e| Error::Internal(format!("Upload: {:?}", e)))?;
        
        let d_output = backend.device.alloc_zeros::<f32>(n * features_per_symbol)
            .map_err(|e| Error::Internal(format!("Alloc: {:?}", e)))?;
        
        // Launch kernel
        let cfg = LaunchConfig {
            grid_dim: ((n + 255) / 256, 1, 1),
            block_dim: (256, 1, 1),
            shared_mem_bytes: 0,
        };
        
        unsafe {
            backend.kernel.clone().launch(
                cfg,
                (&d_input, &d_output, n as i32),
            ).map_err(|e| Error::Internal(format!("Launch: {:?}", e)))?;
        }
        
        // Download results
        let mut output = vec![0.0f32; n * features_per_symbol];
        backend.device.dtoh_sync_copy_into(&d_output, &mut output)
            .map_err(|e| Error::Internal(format!("Download: {:?}", e)))?;
        
        // Convert to features
        let results = snapshots.iter().enumerate().map(|(i, snap)| {
            let start = i * features_per_symbol;
            let end = start + features_per_symbol;
            let features = Array1::from_vec(output[start..end].to_vec());
            
            crate::ComputedFeatures {
                symbol: snap.symbol.clone(),
                timestamp_ns: snap.timestamp_ns,
                features,
                computed_on: crate::Device::CUDA,
            }
        }).collect();
        
        Ok(results)
    }
    
    #[cfg(feature = "wgpu")]
    fn compute_wgpu(&self, backend: &WgpuBackend, snapshots: &[MarketSnapshot]) -> Result<Vec<crate::ComputedFeatures>> {
        // WebGPU implementation
        todo!("WebGPU compute")
    }
}

fn serialize_snapshot(buffer: &mut Vec<f32>, snap: &MarketSnapshot) {
    let book = &snap.orderbook;
    
    // Mid, spread
    buffer.push(book.mid_price().unwrap_or(0.0) as f32);
    buffer.push(book.spread_bps().unwrap_or(0.0) as f32);
    
    // 10 bids
    for i in 0..10 {
        if let Some(level) = book.bids.get(i) {
            buffer.push(level.price.0 as f32);
            buffer.push(level.quantity as f32);
        } else {
            buffer.push(0.0);
            buffer.push(0.0);
        }
    }
    
    // 10 asks
    for i in 0..10 {
        if let Some(level) = book.asks.get(i) {
            buffer.push(level.price.0 as f32);
            buffer.push(level.quantity as f32);
        } else {
            buffer.push(0.0);
            buffer.push(0.0);
        }
    }
    
    // Recent trades
    for i in 0..100 {
        if let Some(trade) = snap.recent_trades.get(i) {
            buffer.push(trade.price as f32);
            buffer.push(trade.quantity as f32);
            buffer.push(if matches!(trade.side, Side::Buy) { 1.0 } else { -1.0 });
        } else {
            buffer.push(0.0);
            buffer.push(0.0);
            buffer.push(0.0);
        }
    }
    
    // Funding
    buffer.push(snap.funding_rate_bps.unwrap_or(0.0) as f32);
}

#[cfg(feature = "cuda")]
fn compile_cuda_kernel() -> cudarc::nvrtc::Ptx {
    // Inline CUDA kernel
    const KERNEL: &str = r#"
extern "C" __global__ void compute_features(
    const float* input,
    float* output,
    int num_symbols
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= num_symbols) return;
    
    // Input layout: mid, spread, 10 bids, 10 asks, 100 trades, funding
    const int input_stride = 2 + 20 + 20 + 300 + 1;
    const float* symbol_input = input + idx * input_stride;
    
    float* symbol_output = output + idx * 100;
    
    // Basic features
    float mid = symbol_input[0];
    float spread = symbol_input[1];
    float funding = symbol_input[input_stride - 1];
    
    symbol_output[0] = mid;
    symbol_output[1] = spread;
    symbol_output[2] = funding;
    
    // Order book imbalance
    float bid_vol = 0.0f;
    float ask_vol = 0.0f;
    
    for (int i = 0; i < 10; i++) {
        bid_vol += symbol_input[2 + i * 2 + 1];
        ask_vol += symbol_input[22 + i * 2 + 1];
    }
    
    float obi = (bid_vol - ask_vol) / (bid_vol + ask_vol + 1e-9f);
    symbol_output[3] = obi;
    
    // Trade flow features
    float buy_vol = 0.0f;
    float sell_vol = 0.0f;
    float vwap_sum = 0.0f;
    float vol_sum = 0.0f;
    
    for (int i = 0; i < 100; i++) {
        int trade_offset = 42 + i * 3;
        float price = symbol_input[trade_offset];
        float qty = symbol_input[trade_offset + 1];
        float side = symbol_input[trade_offset + 2];
        
        if (side > 0.0f) buy_vol += qty;
        else sell_vol += qty;
        
        vwap_sum += price * qty;
        vol_sum += qty;
    }
    
    float ofi = buy_vol - sell_vol;
    float vwap = vwap_sum / (vol_sum + 1e-9f);
    
    symbol_output[4] = ofi;
    symbol_output[5] = mid / vwap;
    
    // Pad remaining
    for (int i = 6; i < 100; i++) {
        symbol_output[i] = 0.0f;
    }
}
"#;
    
    cudarc::nvrtc::compile_ptx(KERNEL).unwrap()
}

const WGSL_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> num_symbols: u32;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= num_symbols) { return; }
    
    // Implement feature computation
    output[idx * 100] = input[idx * 343]; // Placeholder
}
"#;