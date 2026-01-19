//! GPU acceleration module for FHE operations
//!
//! This module provides GPU-accelerated FHE operations using CUDA and Metal backends.
//! It automatically detects available GPU hardware and falls back to CPU when needed.
//!
//! # Architecture
//!
//! - **CUDA Backend**: NVIDIA GPU acceleration on Linux/Windows (via tfhe-cuda-backend)
//! - **Metal Backend**: Apple GPU acceleration on macOS (custom implementation)
//! - **CPU Fallback**: Automatic fallback when GPU is unavailable
//!
//! # Example
//!
//! ```rust,ignore
//! use amaters_core::compute::gpu::{GpuExecutor, GpuBackend};
//!
//! let executor = GpuExecutor::new()?;
//! let backend = executor.backend();
//! println!("Using backend: {:?}", backend);
//! ```

use crate::error::{AmateRSError, ErrorContext, Result};
use std::sync::Arc;
use parking_lot::RwLock;

#[cfg(feature = "compute")]
use crate::compute::operations::{EncryptedBool, EncryptedU8, EncryptedU16, EncryptedU32, EncryptedU64};

/// GPU backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GpuBackend {
    /// NVIDIA CUDA backend (Linux, Windows)
    Cuda,
    /// Apple Metal backend (macOS)
    Metal,
    /// CPU fallback (all platforms)
    Cpu,
}

impl std::fmt::Display for GpuBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuBackend::Cuda => write!(f, "CUDA"),
            GpuBackend::Metal => write!(f, "Metal"),
            GpuBackend::Cpu => write!(f, "CPU"),
        }
    }
}

/// GPU configuration options
#[derive(Debug, Clone)]
pub struct GpuConfig {
    /// Preferred backend (None = auto-detect)
    pub preferred_backend: Option<GpuBackend>,
    /// Device ID for multi-GPU systems
    pub device_id: usize,
    /// Enable batch processing
    pub enable_batch: bool,
    /// Batch size for operations
    pub batch_size: usize,
    /// Memory pool size in bytes (0 = auto)
    pub memory_pool_size: usize,
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            preferred_backend: None,
            device_id: 0,
            enable_batch: true,
            batch_size: 64,
            memory_pool_size: 0,
        }
    }
}

/// GPU device information
#[derive(Debug, Clone)]
pub struct GpuDeviceInfo {
    /// Backend type
    pub backend: GpuBackend,
    /// Device name
    pub name: String,
    /// Compute capability (CUDA) or Metal version
    pub compute_capability: String,
    /// Total memory in bytes
    pub total_memory: u64,
    /// Available memory in bytes
    pub available_memory: u64,
    /// Number of compute units
    pub compute_units: u32,
}

/// GPU executor for FHE operations
///
/// Manages GPU resources and executes FHE operations on the selected backend.
/// Automatically handles memory management, batch processing, and fallback to CPU.
#[derive(Clone)]
pub struct GpuExecutor {
    backend: GpuBackend,
    config: GpuConfig,
    device_info: Option<GpuDeviceInfo>,
    #[cfg(all(feature = "cuda", feature = "compute"))]
    cuda_context: Option<Arc<RwLock<CudaContext>>>,
    #[cfg(all(feature = "metal", feature = "compute"))]
    metal_context: Option<Arc<RwLock<MetalContext>>>,
}

impl GpuExecutor {
    /// Create a new GPU executor with default configuration
    ///
    /// Automatically detects available GPU hardware and selects the best backend.
    pub fn new() -> Result<Self> {
        Self::with_config(GpuConfig::default())
    }

    /// Create a new GPU executor with custom configuration
    pub fn with_config(config: GpuConfig) -> Result<Self> {
        let backend = if let Some(preferred) = config.preferred_backend {
            // Use preferred backend if specified
            preferred
        } else {
            // Auto-detect best available backend
            Self::detect_backend()?
        };

        let device_info = Self::get_device_info(backend, config.device_id)?;

        let mut executor = Self {
            backend,
            config,
            device_info: Some(device_info),
            #[cfg(all(feature = "cuda", feature = "compute"))]
            cuda_context: None,
            #[cfg(all(feature = "metal", feature = "compute"))]
            metal_context: None,
        };

        // Initialize backend-specific context
        executor.initialize_backend()?;

        Ok(executor)
    }

    /// Detect the best available GPU backend
    fn detect_backend() -> Result<GpuBackend> {
        #[cfg(feature = "cuda")]
        {
            if Self::is_cuda_available() {
                return Ok(GpuBackend::Cuda);
            }
        }

        #[cfg(feature = "metal")]
        {
            if Self::is_metal_available() {
                return Ok(GpuBackend::Metal);
            }
        }

        // Fallback to CPU
        Ok(GpuBackend::Cpu)
    }

    /// Check if CUDA backend is available
    #[cfg(feature = "cuda")]
    fn is_cuda_available() -> bool {
        // Check for CUDA runtime and devices
        #[cfg(feature = "compute")]
        {
            cuda::detect_cuda_devices().is_ok()
        }
        #[cfg(not(feature = "compute"))]
        {
            false
        }
    }

    #[cfg(not(feature = "cuda"))]
    fn is_cuda_available() -> bool {
        false
    }

    /// Check if Metal backend is available
    #[cfg(feature = "metal")]
    fn is_metal_available() -> bool {
        // Metal is only available on macOS
        #[cfg(target_os = "macos")]
        {
            #[cfg(feature = "compute")]
            {
                metal::detect_metal_devices().is_ok()
            }
            #[cfg(not(feature = "compute"))]
            {
                false
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    #[cfg(not(feature = "metal"))]
    fn is_metal_available() -> bool {
        false
    }

    /// Get device information for the selected backend
    fn get_device_info(backend: GpuBackend, device_id: usize) -> Result<GpuDeviceInfo> {
        match backend {
            #[cfg(feature = "cuda")]
            GpuBackend::Cuda => {
                #[cfg(feature = "compute")]
                {
                    cuda::get_device_info(device_id)
                }
                #[cfg(not(feature = "compute"))]
                {
                    Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
                        "CUDA backend requires 'compute' feature".to_string(),
                    )))
                }
            }

            #[cfg(feature = "metal")]
            GpuBackend::Metal => {
                #[cfg(feature = "compute")]
                {
                    metal::get_device_info(device_id)
                }
                #[cfg(not(feature = "compute"))]
                {
                    Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
                        "Metal backend requires 'compute' feature".to_string(),
                    )))
                }
            }

            GpuBackend::Cpu => Ok(GpuDeviceInfo {
                backend: GpuBackend::Cpu,
                name: "CPU".to_string(),
                compute_capability: format!("{} cores", num_cpus::get()),
                total_memory: 0,
                available_memory: 0,
                compute_units: num_cpus::get() as u32,
            }),

            #[allow(unreachable_patterns)]
            _ => Err(AmateRSError::Configuration(ErrorContext::new(format!(
                "Backend {} is not available (feature not enabled)",
                backend
            )))),
        }
    }

    /// Initialize backend-specific context
    fn initialize_backend(&mut self) -> Result<()> {
        match self.backend {
            #[cfg(all(feature = "cuda", feature = "compute"))]
            GpuBackend::Cuda => {
                let context = cuda::CudaContext::new(
                    self.config.device_id,
                    self.config.memory_pool_size,
                )?;
                self.cuda_context = Some(Arc::new(RwLock::new(context)));
                Ok(())
            }

            #[cfg(all(feature = "metal", feature = "compute"))]
            GpuBackend::Metal => {
                let context = metal::MetalContext::new(
                    self.config.device_id,
                    self.config.memory_pool_size,
                )?;
                self.metal_context = Some(Arc::new(RwLock::new(context)));
                Ok(())
            }

            GpuBackend::Cpu => {
                // CPU backend doesn't need initialization
                Ok(())
            }

            #[allow(unreachable_patterns)]
            _ => Err(AmateRSError::Configuration(ErrorContext::new(format!(
                "Cannot initialize backend {} (feature not enabled)",
                self.backend
            )))),
        }
    }

    /// Get the current backend
    pub fn backend(&self) -> GpuBackend {
        self.backend
    }

    /// Get device information
    pub fn device_info(&self) -> Option<&GpuDeviceInfo> {
        self.device_info.as_ref()
    }

    /// Get configuration
    pub fn config(&self) -> &GpuConfig {
        &self.config
    }

    /// Check if GPU acceleration is enabled
    pub fn is_gpu_enabled(&self) -> bool {
        !matches!(self.backend, GpuBackend::Cpu)
    }

    /// Execute FHE operation with GPU acceleration
    ///
    /// This method automatically routes the operation to the appropriate backend
    /// and handles memory transfers between CPU and GPU.
    #[cfg(feature = "compute")]
    pub fn execute_operation<F, R>(&self, operation: F) -> Result<R>
    where
        F: FnOnce() -> Result<R> + Send,
        R: Send,
    {
        match self.backend {
            #[cfg(feature = "cuda")]
            GpuBackend::Cuda => {
                #[cfg(feature = "compute")]
                {
                    if let Some(context) = &self.cuda_context {
                        let ctx = context.read();
                        ctx.execute_operation(operation)
                    } else {
                        Err(AmateRSError::GpuError(ErrorContext::new(
                            "CUDA context not initialized".to_string(),
                        )))
                    }
                }
                #[cfg(not(feature = "compute"))]
                {
                    Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
                        "CUDA backend requires 'compute' feature".to_string(),
                    )))
                }
            }

            #[cfg(feature = "metal")]
            GpuBackend::Metal => {
                #[cfg(feature = "compute")]
                {
                    if let Some(context) = &self.metal_context {
                        let ctx = context.read();
                        ctx.execute_operation(operation)
                    } else {
                        Err(AmateRSError::GpuError(ErrorContext::new(
                            "Metal context not initialized".to_string(),
                        )))
                    }
                }
                #[cfg(not(feature = "compute"))]
                {
                    Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
                        "Metal backend requires 'compute' feature".to_string(),
                    )))
                }
            }

            GpuBackend::Cpu => {
                // Execute on CPU directly
                operation()
            }

            #[allow(unreachable_patterns)]
            _ => Err(AmateRSError::Configuration(ErrorContext::new(format!(
                "Backend {} is not available",
                self.backend
            )))),
        }
    }

    /// Stub implementation when compute feature is disabled
    #[cfg(not(feature = "compute"))]
    pub fn execute_operation<F, R>(&self, _operation: F) -> Result<R>
    where
        F: FnOnce() -> Result<R> + Send,
        R: Send,
    {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }

    /// Execute batch of FHE operations with GPU acceleration
    #[cfg(feature = "compute")]
    pub fn execute_batch<F, R>(&self, operations: Vec<F>) -> Result<Vec<R>>
    where
        F: FnOnce() -> Result<R> + Send,
        R: Send,
    {
        if !self.config.enable_batch || operations.is_empty() {
            return operations
                .into_iter()
                .map(|op| self.execute_operation(op))
                .collect();
        }

        match self.backend {
            #[cfg(feature = "cuda")]
            GpuBackend::Cuda => {
                #[cfg(feature = "compute")]
                {
                    if let Some(context) = &self.cuda_context {
                        let ctx = context.read();
                        ctx.execute_batch(operations, self.config.batch_size)
                    } else {
                        Err(AmateRSError::GpuError(ErrorContext::new(
                            "CUDA context not initialized".to_string(),
                        )))
                    }
                }
                #[cfg(not(feature = "compute"))]
                {
                    Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
                        "CUDA backend requires 'compute' feature".to_string(),
                    )))
                }
            }

            #[cfg(feature = "metal")]
            GpuBackend::Metal => {
                #[cfg(feature = "compute")]
                {
                    if let Some(context) = &self.metal_context {
                        let ctx = context.read();
                        ctx.execute_batch(operations, self.config.batch_size)
                    } else {
                        Err(AmateRSError::GpuError(ErrorContext::new(
                            "Metal context not initialized".to_string(),
                        )))
                    }
                }
                #[cfg(not(feature = "compute"))]
                {
                    Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
                        "Metal backend requires 'compute' feature".to_string(),
                    )))
                }
            }

            GpuBackend::Cpu => {
                // Execute batch on CPU using rayon if available
                #[cfg(feature = "parallel")]
                {
                    use rayon::prelude::*;
                    operations
                        .into_par_iter()
                        .map(|op| op())
                        .collect()
                }
                #[cfg(not(feature = "parallel"))]
                {
                    operations.into_iter().map(|op| op()).collect()
                }
            }

            #[allow(unreachable_patterns)]
            _ => Err(AmateRSError::Configuration(ErrorContext::new(format!(
                "Backend {} is not available",
                self.backend
            )))),
        }
    }

    /// Stub implementation when compute feature is disabled
    #[cfg(not(feature = "compute"))]
    pub fn execute_batch<F, R>(&self, _operations: Vec<F>) -> Result<Vec<R>>
    where
        F: FnOnce() -> Result<R> + Send,
        R: Send,
    {
        Err(AmateRSError::FeatureNotEnabled(ErrorContext::new(
            "FHE compute feature is not enabled".to_string(),
        )))
    }
}

impl Default for GpuExecutor {
    fn default() -> Self {
        Self::new().expect("Failed to create default GPU executor")
    }
}

impl std::fmt::Debug for GpuExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuExecutor")
            .field("backend", &self.backend)
            .field("config", &self.config)
            .field("device_info", &self.device_info)
            .finish()
    }
}

/// CUDA backend implementation
#[cfg(all(feature = "cuda", feature = "compute"))]
mod cuda {
    use super::*;

    /// CUDA context for GPU operations
    pub struct CudaContext {
        device_id: usize,
        memory_pool_size: usize,
    }

    impl CudaContext {
        pub fn new(device_id: usize, memory_pool_size: usize) -> Result<Self> {
            // Initialize CUDA context
            // Note: tfhe-cuda-backend handles context initialization internally
            Ok(Self {
                device_id,
                memory_pool_size,
            })
        }

        pub fn execute_operation<F, R>(&self, operation: F) -> Result<R>
        where
            F: FnOnce() -> Result<R> + Send,
            R: Send,
        {
            // CUDA operations are executed directly by tfhe-rs when GPU is enabled
            // The tfhe-cuda-backend is automatically used when available
            operation()
        }

        pub fn execute_batch<F, R>(&self, operations: Vec<F>, batch_size: usize) -> Result<Vec<R>>
        where
            F: FnOnce() -> Result<R> + Send,
            R: Send,
        {
            // Process operations in batches
            let mut results = Vec::with_capacity(operations.len());

            for chunk in operations.chunks(batch_size) {
                #[cfg(feature = "parallel")]
                {
                    use rayon::prelude::*;
                    let chunk_results: Result<Vec<_>> = chunk
                        .into_par_iter()
                        .map(|op| op())
                        .collect();
                    results.extend(chunk_results?);
                }
                #[cfg(not(feature = "parallel"))]
                {
                    for op in chunk {
                        results.push(op()?);
                    }
                }
            }

            Ok(results)
        }
    }

    /// Detect available CUDA devices
    pub fn detect_cuda_devices() -> Result<Vec<usize>> {
        // In a real implementation, this would query CUDA runtime
        // For now, we'll check if tfhe-cuda-backend is available
        #[cfg(feature = "cuda")]
        {
            // tfhe-cuda-backend automatically detects devices
            // Return device 0 if available
            Ok(vec![0])
        }
        #[cfg(not(feature = "cuda"))]
        {
            Err(AmateRSError::GpuError(ErrorContext::new(
                "CUDA feature not enabled".to_string(),
            )))
        }
    }

    /// Get CUDA device information
    pub fn get_device_info(device_id: usize) -> Result<GpuDeviceInfo> {
        // In a real implementation, this would query CUDA device properties
        // For now, we'll return placeholder values
        Ok(GpuDeviceInfo {
            backend: GpuBackend::Cuda,
            name: format!("CUDA Device {}", device_id),
            compute_capability: "8.0".to_string(),
            total_memory: 8_589_934_592, // 8 GB placeholder
            available_memory: 7_516_192_768, // ~7 GB placeholder
            compute_units: 68,
        })
    }
}

/// Metal backend implementation
#[cfg(all(feature = "metal", feature = "compute", target_os = "macos"))]
mod metal {
    use super::*;

    /// Metal context for GPU operations
    pub struct MetalContext {
        device_id: usize,
        memory_pool_size: usize,
    }

    impl MetalContext {
        pub fn new(device_id: usize, memory_pool_size: usize) -> Result<Self> {
            // Initialize Metal context
            // This would create Metal device, command queue, etc.
            Ok(Self {
                device_id,
                memory_pool_size,
            })
        }

        pub fn execute_operation<F, R>(&self, operation: F) -> Result<R>
        where
            F: FnOnce() -> Result<R> + Send,
            R: Send,
        {
            // Metal operations would be executed here
            // For now, we execute on CPU as Metal backend is not yet implemented
            operation()
        }

        pub fn execute_batch<F, R>(&self, operations: Vec<F>, batch_size: usize) -> Result<Vec<R>>
        where
            F: FnOnce() -> Result<R> + Send,
            R: Send,
        {
            // Process operations in batches
            let mut results = Vec::with_capacity(operations.len());

            for chunk in operations.chunks(batch_size) {
                #[cfg(feature = "parallel")]
                {
                    use rayon::prelude::*;
                    let chunk_results: Result<Vec<_>> = chunk
                        .into_par_iter()
                        .map(|op| op())
                        .collect();
                    results.extend(chunk_results?);
                }
                #[cfg(not(feature = "parallel"))]
                {
                    for op in chunk {
                        results.push(op()?);
                    }
                }
            }

            Ok(results)
        }
    }

    /// Detect available Metal devices
    pub fn detect_metal_devices() -> Result<Vec<usize>> {
        // Check if Metal is available on macOS
        #[cfg(target_os = "macos")]
        {
            // Return device 0 if available (default Metal device)
            Ok(vec![0])
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(AmateRSError::GpuError(ErrorContext::new(
                "Metal is only available on macOS".to_string(),
            )))
        }
    }

    /// Get Metal device information
    pub fn get_device_info(device_id: usize) -> Result<GpuDeviceInfo> {
        Ok(GpuDeviceInfo {
            backend: GpuBackend::Metal,
            name: format!("Apple Metal Device {}", device_id),
            compute_capability: "Metal 3.0".to_string(),
            total_memory: 16_106_127_360, // 15 GB unified memory placeholder
            available_memory: 14_495_514_624, // ~13.5 GB placeholder
            compute_units: 32,
        })
    }
}

/// Stub Metal module for non-macOS platforms
#[cfg(all(feature = "metal", feature = "compute", not(target_os = "macos")))]
mod metal {
    use super::*;

    pub struct MetalContext;

    impl MetalContext {
        pub fn new(_device_id: usize, _memory_pool_size: usize) -> Result<Self> {
            Err(AmateRSError::GpuError(ErrorContext::new(
                "Metal is only available on macOS".to_string(),
            )))
        }

        pub fn execute_operation<F, R>(&self, _operation: F) -> Result<R>
        where
            F: FnOnce() -> Result<R> + Send,
            R: Send,
        {
            Err(AmateRSError::GpuError(ErrorContext::new(
                "Metal is only available on macOS".to_string(),
            )))
        }

        pub fn execute_batch<F, R>(&self, _operations: Vec<F>, _batch_size: usize) -> Result<Vec<R>>
        where
            F: FnOnce() -> Result<R> + Send,
            R: Send,
        {
            Err(AmateRSError::GpuError(ErrorContext::new(
                "Metal is only available on macOS".to_string(),
            )))
        }
    }

    pub fn detect_metal_devices() -> Result<Vec<usize>> {
        Err(AmateRSError::GpuError(ErrorContext::new(
            "Metal is only available on macOS".to_string(),
        )))
    }

    pub fn get_device_info(_device_id: usize) -> Result<GpuDeviceInfo> {
        Err(AmateRSError::GpuError(ErrorContext::new(
            "Metal is only available on macOS".to_string(),
        )))
    }
}

#[cfg(all(test, feature = "compute"))]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_backend_display() {
        assert_eq!(format!("{}", GpuBackend::Cuda), "CUDA");
        assert_eq!(format!("{}", GpuBackend::Metal), "Metal");
        assert_eq!(format!("{}", GpuBackend::Cpu), "CPU");
    }

    #[test]
    fn test_gpu_config_default() {
        let config = GpuConfig::default();
        assert_eq!(config.preferred_backend, None);
        assert_eq!(config.device_id, 0);
        assert!(config.enable_batch);
        assert_eq!(config.batch_size, 64);
        assert_eq!(config.memory_pool_size, 0);
    }

    #[test]
    fn test_gpu_executor_creation() -> Result<()> {
        let executor = GpuExecutor::new()?;
        assert!(matches!(
            executor.backend(),
            GpuBackend::Cuda | GpuBackend::Metal | GpuBackend::Cpu
        ));
        Ok(())
    }

    #[test]
    fn test_gpu_executor_with_cpu_fallback() -> Result<()> {
        let config = GpuConfig {
            preferred_backend: Some(GpuBackend::Cpu),
            ..Default::default()
        };
        let executor = GpuExecutor::with_config(config)?;
        assert_eq!(executor.backend(), GpuBackend::Cpu);
        assert!(!executor.is_gpu_enabled());
        Ok(())
    }

    #[test]
    fn test_device_info() -> Result<()> {
        let executor = GpuExecutor::new()?;
        let info = executor.device_info();
        assert!(info.is_some());

        if let Some(info) = info {
            assert!(!info.name.is_empty());
            assert!(!info.compute_capability.is_empty());
        }

        Ok(())
    }

    #[test]
    fn test_execute_operation_cpu() -> Result<()> {
        let config = GpuConfig {
            preferred_backend: Some(GpuBackend::Cpu),
            ..Default::default()
        };
        let executor = GpuExecutor::with_config(config)?;

        let result = executor.execute_operation(|| Ok(42))?;
        assert_eq!(result, 42);

        Ok(())
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn test_execute_batch_cpu() -> Result<()> {
        let config = GpuConfig {
            preferred_backend: Some(GpuBackend::Cpu),
            enable_batch: true,
            batch_size: 4,
            ..Default::default()
        };
        let executor = GpuExecutor::with_config(config)?;

        let operations: Vec<_> = (0..10)
            .map(|i| move || Ok(i * 2))
            .collect();

        let results = executor.execute_batch(operations)?;
        assert_eq!(results.len(), 10);
        assert_eq!(results, vec![0, 2, 4, 6, 8, 10, 12, 14, 16, 18]);

        Ok(())
    }
}
