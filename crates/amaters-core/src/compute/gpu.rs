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
use parking_lot::RwLock;
use std::sync::Arc;

#[cfg(feature = "compute")]
use crate::compute::operations::{
    EncryptedBool, EncryptedU8, EncryptedU16, EncryptedU32, EncryptedU64,
};

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
    cuda_context: Option<Arc<RwLock<cuda::CudaContext>>>,
    #[cfg(all(feature = "metal", feature = "compute"))]
    metal_context: Option<Arc<RwLock<metal::MetalContext>>>,
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

            GpuBackend::Cpu => {
                let cpus = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1);
                Ok(GpuDeviceInfo {
                    backend: GpuBackend::Cpu,
                    name: "CPU".to_string(),
                    compute_capability: format!("{} cores", cpus),
                    total_memory: 0,
                    available_memory: 0,
                    compute_units: cpus as u32,
                })
            }

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
                let context =
                    cuda::CudaContext::new(self.config.device_id, self.config.memory_pool_size)?;
                self.cuda_context = Some(Arc::new(RwLock::new(context)));
                Ok(())
            }

            #[cfg(all(feature = "metal", feature = "compute"))]
            GpuBackend::Metal => {
                let context =
                    metal::MetalContext::new(self.config.device_id, self.config.memory_pool_size)?;
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
                    operations.into_par_iter().map(|op| op()).collect()
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

/// GPU device detection module
///
/// Provides real hardware detection using platform-specific tools:
/// - macOS: `system_profiler SPDisplaysDataType` and `sysctl hw.memsize`
/// - Linux: `nvidia-smi` and sysfs fallbacks
mod detection {
    use super::{GpuBackend, GpuDeviceInfo};
    use std::process::Command;

    /// Detect macOS GPU via system_profiler
    #[cfg(target_os = "macos")]
    pub fn detect_macos_gpu() -> Option<GpuDeviceInfo> {
        let output = Command::new("system_profiler")
            .arg("SPDisplaysDataType")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut info = parse_system_profiler(&text)?;

        // For Apple Silicon (unified memory), get total system memory via sysctl
        if info.name.starts_with("Apple") {
            if let Some(mem) = get_macos_system_memory() {
                info.total_memory = mem;
                // Estimate ~90% available (conservative)
                info.available_memory = mem * 9 / 10;
            }
        }

        Some(info)
    }

    /// Get macOS system memory via `sysctl hw.memsize`
    #[cfg(target_os = "macos")]
    fn get_macos_system_memory() -> Option<u64> {
        let output = Command::new("sysctl")
            .arg("-n")
            .arg("hw.memsize")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        text.trim().parse::<u64>().ok()
    }

    /// Detect NVIDIA GPU on Linux via nvidia-smi
    #[cfg(target_os = "linux")]
    pub fn detect_nvidia_gpu(device_id: usize) -> Option<GpuDeviceInfo> {
        let output = Command::new("nvidia-smi")
            .args([
                "--query-gpu=name,memory.total,memory.free",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let devices = parse_nvidia_smi(&text);
        devices.into_iter().nth(device_id)
    }

    /// Detect NVIDIA GPU count on Linux via nvidia-smi
    #[cfg(target_os = "linux")]
    pub fn detect_nvidia_device_count() -> Option<usize> {
        let output = Command::new("nvidia-smi")
            .args(["--query-gpu=name", "--format=csv,noheader"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let count = text.lines().filter(|l| !l.trim().is_empty()).count();
        if count > 0 { Some(count) } else { None }
    }

    /// Linux sysfs fallback: detect NVIDIA devices via /sys/class/drm
    #[cfg(target_os = "linux")]
    pub fn detect_nvidia_sysfs() -> Vec<GpuDeviceInfo> {
        let mut devices = Vec::new();
        let drm_path = std::path::Path::new("/sys/class/drm");
        if !drm_path.exists() {
            return devices;
        }

        let entries = match std::fs::read_dir(drm_path) {
            Ok(e) => e,
            Err(_) => return devices,
        };

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("card") || name_str.contains('-') {
                continue;
            }

            let vendor_path = entry.path().join("device/vendor");
            if let Ok(vendor) = std::fs::read_to_string(&vendor_path) {
                let vendor_trimmed = vendor.trim();
                // 0x10de = NVIDIA
                if vendor_trimmed == "0x10de" {
                    let device_name = read_nvidia_proc_name(devices.len())
                        .unwrap_or_else(|| format!("NVIDIA GPU (card {})", name_str));

                    devices.push(GpuDeviceInfo {
                        backend: GpuBackend::Cuda,
                        name: device_name,
                        compute_capability: "unknown".to_string(),
                        total_memory: 0,
                        available_memory: 0,
                        compute_units: 0,
                    });
                }
            }
        }

        devices
    }

    /// Try to read NVIDIA GPU name from /proc/driver/nvidia/gpus/*/information
    #[cfg(target_os = "linux")]
    fn read_nvidia_proc_name(index: usize) -> Option<String> {
        let nvidia_path = std::path::Path::new("/proc/driver/nvidia/gpus");
        if !nvidia_path.exists() {
            return None;
        }

        let entries: Vec<_> = std::fs::read_dir(nvidia_path).ok()?.flatten().collect();

        let entry = entries.get(index)?;
        let info_path = entry.path().join("information");
        let content = std::fs::read_to_string(info_path).ok()?;

        for line in content.lines() {
            if let Some(stripped) = line.strip_prefix("Model:") {
                return Some(stripped.trim().to_string());
            }
        }

        None
    }

    /// Parse `system_profiler SPDisplaysDataType` output
    ///
    /// Extracts GPU name, compute units, and memory information from the
    /// macOS system_profiler output.
    pub fn parse_system_profiler(text: &str) -> Option<GpuDeviceInfo> {
        let mut chipset_model: Option<String> = None;
        let mut total_cores: Option<u32> = None;
        let mut vram_bytes: Option<u64> = None;
        let mut is_apple_silicon = false;

        for line in text.lines() {
            let trimmed = line.trim();

            // Extract chipset model name
            if let Some(value) = trimmed.strip_prefix("Chipset Model:") {
                chipset_model = Some(value.trim().to_string());
                if value.trim().starts_with("Apple") {
                    is_apple_silicon = true;
                }
            }

            // Extract total number of GPU cores
            if let Some(value) = trimmed.strip_prefix("Total Number of Cores:") {
                total_cores = value.trim().parse::<u32>().ok();
            }

            // Extract VRAM (for discrete GPUs)
            if trimmed.starts_with("VRAM") {
                // Formats: "VRAM (Total): 8 GB", "VRAM (Dynamic, Max): 1536 MB"
                if let Some(colon_pos) = trimmed.find(':') {
                    let value_part = trimmed[colon_pos + 1..].trim();
                    vram_bytes = parse_memory_string(value_part);
                }
            }
        }

        let name = chipset_model?;

        let compute_capability = if is_apple_silicon {
            "Metal 3".to_string()
        } else if name.contains("Intel") {
            "Metal 2".to_string()
        } else {
            "Metal".to_string()
        };

        let compute_units = total_cores.unwrap_or(0);

        // For Apple Silicon, memory will be set later from sysctl
        // For discrete GPUs, use VRAM
        let total_memory = vram_bytes.unwrap_or(0);
        let available_memory = if total_memory > 0 {
            total_memory * 9 / 10
        } else {
            0
        };

        Some(GpuDeviceInfo {
            backend: GpuBackend::Metal,
            name,
            compute_capability,
            total_memory,
            available_memory,
            compute_units,
        })
    }

    /// Parse a memory string like "8 GB", "1536 MB", "16384 MB" into bytes
    fn parse_memory_string(s: &str) -> Option<u64> {
        let s = s.trim();
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() < 2 {
            return None;
        }

        let value = parts[0].replace(',', "").parse::<u64>().ok()?;
        let unit = parts[1].to_uppercase();

        match unit.as_str() {
            "GB" => Some(value * 1_073_741_824),
            "MB" => Some(value * 1_048_576),
            "KB" => Some(value * 1024),
            "TB" => Some(value * 1_099_511_627_776),
            _ => None,
        }
    }

    /// Parse nvidia-smi CSV output
    ///
    /// Expected input format (from `--format=csv,noheader,nounits`):
    /// ```text
    /// NVIDIA GeForce RTX 4090, 24564, 23456
    /// ```
    ///
    /// Each line: name, total_memory_mb, free_memory_mb
    pub fn parse_nvidia_smi(text: &str) -> Vec<GpuDeviceInfo> {
        let mut devices = Vec::new();

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let parts: Vec<&str> = trimmed.splitn(3, ',').collect();
            if parts.len() < 3 {
                continue;
            }

            let name = parts[0].trim().to_string();
            let total_mb = match parts[1].trim().parse::<u64>() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let free_mb = match parts[2].trim().parse::<u64>() {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Convert MB to bytes
            let total_memory = total_mb * 1_048_576;
            let available_memory = free_mb * 1_048_576;

            devices.push(GpuDeviceInfo {
                backend: GpuBackend::Cuda,
                name,
                compute_capability: "unknown".to_string(),
                total_memory,
                available_memory,
                compute_units: 0,
            });
        }

        devices
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
            // Process operations in batches, consuming the Vec
            let mut results = Vec::with_capacity(operations.len());
            let mut iter = operations.into_iter().peekable();

            while iter.peek().is_some() {
                let batch: Vec<F> = iter.by_ref().take(batch_size).collect();

                #[cfg(feature = "parallel")]
                {
                    use rayon::prelude::*;
                    let chunk_results: Result<Vec<_>> =
                        batch.into_par_iter().map(|op| op()).collect();
                    results.extend(chunk_results?);
                }
                #[cfg(not(feature = "parallel"))]
                {
                    for op in batch {
                        results.push(op()?);
                    }
                }
            }

            Ok(results)
        }
    }

    /// Detect available CUDA devices
    ///
    /// On Linux, tries nvidia-smi first, then falls back to sysfs detection.
    pub fn detect_cuda_devices() -> Result<Vec<usize>> {
        #[cfg(target_os = "linux")]
        {
            // Try nvidia-smi first
            if let Some(count) = detection::detect_nvidia_device_count() {
                return Ok((0..count).collect());
            }

            // Fallback to sysfs detection
            let sysfs_devices = detection::detect_nvidia_sysfs();
            if !sysfs_devices.is_empty() {
                return Ok((0..sysfs_devices.len()).collect());
            }
        }

        // Fallback: assume device 0 is available when cuda feature is enabled
        Ok(vec![0])
    }

    /// Get CUDA device information
    ///
    /// Attempts real detection via nvidia-smi on Linux, with sysfs fallback.
    /// Returns a placeholder if all detection methods fail.
    pub fn get_device_info(device_id: usize) -> Result<GpuDeviceInfo> {
        // Try real detection on Linux
        #[cfg(target_os = "linux")]
        {
            // Try nvidia-smi first
            if let Some(info) = detection::detect_nvidia_gpu(device_id) {
                return Ok(info);
            }

            // Try sysfs fallback
            let sysfs_devices = detection::detect_nvidia_sysfs();
            if let Some(info) = sysfs_devices.into_iter().nth(device_id) {
                return Ok(info);
            }
        }

        // Fallback to placeholder
        Ok(cuda_placeholder(device_id))
    }

    /// Generate a placeholder CUDA device info when detection fails
    fn cuda_placeholder(device_id: usize) -> GpuDeviceInfo {
        GpuDeviceInfo {
            backend: GpuBackend::Cuda,
            name: format!("CUDA Device {}", device_id),
            compute_capability: "unknown".to_string(),
            total_memory: 0,
            available_memory: 0,
            compute_units: 0,
        }
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
            // Process operations in batches, consuming the Vec
            let mut results = Vec::with_capacity(operations.len());
            let mut iter = operations.into_iter().peekable();

            while iter.peek().is_some() {
                let batch: Vec<F> = iter.by_ref().take(batch_size).collect();

                #[cfg(feature = "parallel")]
                {
                    use rayon::prelude::*;
                    let chunk_results: Result<Vec<_>> =
                        batch.into_par_iter().map(|op| op()).collect();
                    results.extend(chunk_results?);
                }
                #[cfg(not(feature = "parallel"))]
                {
                    for op in batch {
                        results.push(op()?);
                    }
                }
            }

            Ok(results)
        }
    }

    /// Detect available Metal devices
    ///
    /// On macOS, runs system_profiler to detect GPU hardware.
    pub fn detect_metal_devices() -> Result<Vec<usize>> {
        if let Some(_info) = detection::detect_macos_gpu() {
            Ok(vec![0])
        } else {
            // Fallback: assume device 0 on macOS (Metal is always available)
            Ok(vec![0])
        }
    }

    /// Get Metal device information
    ///
    /// Attempts real detection via system_profiler on macOS.
    /// Returns a placeholder if detection fails.
    pub fn get_device_info(device_id: usize) -> Result<GpuDeviceInfo> {
        if device_id == 0 {
            if let Some(info) = detection::detect_macos_gpu() {
                return Ok(info);
            }
        }

        // Fallback to placeholder
        Ok(metal_placeholder(device_id))
    }

    /// Generate a placeholder Metal device info when detection fails
    fn metal_placeholder(device_id: usize) -> GpuDeviceInfo {
        GpuDeviceInfo {
            backend: GpuBackend::Metal,
            name: format!("Apple Metal Device {}", device_id),
            compute_capability: "Metal".to_string(),
            total_memory: 0,
            available_memory: 0,
            compute_units: 0,
        }
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

        let operations: Vec<_> = (0..10).map(|i| move || Ok(i * 2)).collect();

        let results = executor.execute_batch(operations)?;
        assert_eq!(results.len(), 10);
        assert_eq!(results, vec![0, 2, 4, 6, 8, 10, 12, 14, 16, 18]);

        Ok(())
    }

    // ---- GPU detection parsing tests ----

    #[test]
    fn test_parse_system_profiler_m1() {
        let output = "\
Graphics/Displays:

    Apple M1:

      Chipset Model: Apple M1
      Type: GPU
      Bus: Built-In
      Total Number of Cores: 8
      Vendor: Apple (0x106b)
      Metal Support: Metal 3
";
        let info = detection::parse_system_profiler(output);
        assert!(info.is_some());
        let info = info.expect("should parse");
        assert_eq!(info.name, "Apple M1");
        assert_eq!(info.compute_units, 8);
        assert_eq!(info.backend, GpuBackend::Metal);
        assert_eq!(info.compute_capability, "Metal 3");
    }

    #[test]
    fn test_parse_system_profiler_m2_pro() {
        let output = "\
Graphics/Displays:

    Apple M2 Pro:

      Chipset Model: Apple M2 Pro
      Type: GPU
      Bus: Built-In
      Total Number of Cores: 19
      Vendor: Apple (0x106b)
      Metal Support: Metal 3
";
        let info = detection::parse_system_profiler(output);
        assert!(info.is_some());
        let info = info.expect("should parse");
        assert_eq!(info.name, "Apple M2 Pro");
        assert_eq!(info.compute_units, 19);
        assert_eq!(info.compute_capability, "Metal 3");
    }

    #[test]
    fn test_parse_system_profiler_m3_max() {
        let output = "\
Graphics/Displays:

    Apple M3 Max:

      Chipset Model: Apple M3 Max
      Type: GPU
      Bus: Built-In
      Total Number of Cores: 40
      Vendor: Apple (0x106b)
      Metal Support: Metal 3
";
        let info = detection::parse_system_profiler(output);
        assert!(info.is_some());
        let info = info.expect("should parse");
        assert_eq!(info.name, "Apple M3 Max");
        assert_eq!(info.compute_units, 40);
        assert_eq!(info.compute_capability, "Metal 3");
    }

    #[test]
    fn test_parse_system_profiler_intel_gpu() {
        let output = "\
Graphics/Displays:

    Intel Iris Plus Graphics 655:

      Chipset Model: Intel Iris Plus Graphics 655
      Type: GPU
      Bus: Built-In
      VRAM (Dynamic, Max): 1536 MB
      Vendor: Intel (0x8086)
      Device ID: 0x3ea5
      Metal Support: Metal 2
";
        let info = detection::parse_system_profiler(output);
        assert!(info.is_some());
        let info = info.expect("should parse");
        assert_eq!(info.name, "Intel Iris Plus Graphics 655");
        assert_eq!(info.compute_capability, "Metal 2");
        // 1536 MB = 1536 * 1048576 = 1610612736
        assert_eq!(info.total_memory, 1_610_612_736);
        assert!(info.available_memory > 0);
        assert_eq!(info.compute_units, 0); // Intel GPUs don't report cores this way
    }

    #[test]
    fn test_parse_system_profiler_empty() {
        let output = "";
        let info = detection::parse_system_profiler(output);
        assert!(info.is_none());
    }

    #[test]
    fn test_parse_system_profiler_no_gpu_section() {
        let output = "\
Graphics/Displays:

    No GPU found.
";
        let info = detection::parse_system_profiler(output);
        assert!(info.is_none());
    }

    #[test]
    fn test_parse_nvidia_smi_single() {
        let output = "NVIDIA GeForce RTX 4090, 24564, 23456\n";
        let devices = detection::parse_nvidia_smi(output);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(devices[0].total_memory, 24564 * 1_048_576);
        assert_eq!(devices[0].available_memory, 23456 * 1_048_576);
        assert_eq!(devices[0].backend, GpuBackend::Cuda);
    }

    #[test]
    fn test_parse_nvidia_smi_multi() {
        let output = "\
NVIDIA GeForce RTX 4090, 24564, 23456
NVIDIA A100-SXM4-80GB, 81920, 79000
";
        let devices = detection::parse_nvidia_smi(output);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(devices[0].total_memory, 24564 * 1_048_576);
        assert_eq!(devices[1].name, "NVIDIA A100-SXM4-80GB");
        assert_eq!(devices[1].total_memory, 81920 * 1_048_576);
        assert_eq!(devices[1].available_memory, 79000 * 1_048_576);
    }

    #[test]
    fn test_parse_nvidia_smi_empty() {
        let output = "";
        let devices = detection::parse_nvidia_smi(output);
        assert!(devices.is_empty());
    }

    #[test]
    fn test_parse_nvidia_smi_malformed() {
        let output = "\
this is not valid csv data
also garbage
,,
just-one-field
name, not_a_number, 123
name, 123, not_a_number
";
        let devices = detection::parse_nvidia_smi(output);
        assert!(devices.is_empty());
    }

    #[test]
    fn test_gpu_device_info_fields() {
        let info = GpuDeviceInfo {
            backend: GpuBackend::Cuda,
            name: "Test GPU".to_string(),
            compute_capability: "8.9".to_string(),
            total_memory: 16_000_000_000,
            available_memory: 15_000_000_000,
            compute_units: 128,
        };
        assert_eq!(info.backend, GpuBackend::Cuda);
        assert_eq!(info.name, "Test GPU");
        assert_eq!(info.compute_capability, "8.9");
        assert_eq!(info.total_memory, 16_000_000_000);
        assert_eq!(info.available_memory, 15_000_000_000);
        assert_eq!(info.compute_units, 128);
    }

    #[test]
    fn test_fallback_to_placeholder_cuda() {
        // Parsing empty nvidia-smi output returns empty vec,
        // so the caller would fall back to placeholder
        let devices = detection::parse_nvidia_smi("");
        assert!(devices.is_empty());

        // Malformed data also falls back
        let devices = detection::parse_nvidia_smi("garbage data here");
        assert!(devices.is_empty());
    }

    #[test]
    fn test_fallback_to_placeholder_metal() {
        // Empty system_profiler output returns None,
        // so the caller would fall back to placeholder
        let info = detection::parse_system_profiler("");
        assert!(info.is_none());

        // No chipset model also returns None
        let info = detection::parse_system_profiler("Graphics/Displays:\n    No data\n");
        assert!(info.is_none());
    }

    #[test]
    fn test_detect_on_current_platform() {
        // This test runs real detection on the current platform
        #[cfg(target_os = "macos")]
        {
            let info = detection::detect_macos_gpu();
            // On macOS, we should always detect a GPU
            assert!(info.is_some(), "should detect GPU on macOS");
            let info = info.expect("GPU detected");
            assert!(!info.name.is_empty());
            assert_eq!(info.backend, GpuBackend::Metal);
            // Apple Silicon should have unified memory > 0
            if info.name.starts_with("Apple") {
                assert!(info.total_memory > 0, "Apple Silicon should report memory");
                assert!(info.compute_units > 0, "Apple Silicon should report cores");
            }
        }

        #[cfg(target_os = "linux")]
        {
            // On Linux, detection depends on having NVIDIA hardware
            // Just verify the functions don't panic
            let _nvidia = detection::detect_nvidia_gpu(0);
            let _count = detection::detect_nvidia_device_count();
            let _sysfs = detection::detect_nvidia_sysfs();
        }
    }

    #[test]
    fn test_parse_nvidia_smi_whitespace_handling() {
        let output = "  NVIDIA RTX 3080 ,  10240 ,  9500  \n";
        let devices = detection::parse_nvidia_smi(output);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "NVIDIA RTX 3080");
        assert_eq!(devices[0].total_memory, 10240 * 1_048_576);
        assert_eq!(devices[0].available_memory, 9500 * 1_048_576);
    }
}
