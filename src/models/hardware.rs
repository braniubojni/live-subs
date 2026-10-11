//! Machine facts the matcher reads. [`Hardware::detect`] is the only probe.

use serde::Deserialize;

/// Where a model is expected to keep up with real time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accel {
    Cpu,
    Metal,
    Cuda,
    Vulkan,
}

impl Accel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Metal => "metal",
            Self::Cuda => "cuda",
            Self::Vulkan => "vulkan",
        }
    }

    /// Speed class to require. A GPU machine asks for that GPU, not for CPU.
    pub fn for_gpu(gpu: GpuKind) -> Self {
        match gpu {
            GpuKind::Metal => Self::Metal,
            GpuKind::Cuda => Self::Cuda,
            GpuKind::Vulkan => Self::Vulkan,
            GpuKind::None => Self::Cpu,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuKind {
    Metal,
    Cuda,
    Vulkan,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hardware {
    pub ram: u64,
    pub free_disk: u64,
    pub cpu_cores: usize,
    pub gpu: GpuKind,
}

impl Hardware {
    pub fn detect() -> Self {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let free_disk = disks.iter().map(|d| d.available_space()).max().unwrap_or(0);
        Self {
            ram: sys.total_memory(),
            free_disk,
            cpu_cores: sysinfo::System::physical_core_count().unwrap_or(1),
            gpu: classify_gpu(cfg!(target_os = "macos"), cuda_present(), vulkan_present()),
        }
    }
}

/// macOS is Metal. Elsewhere a device node or an SDK env var is enough.
pub fn classify_gpu(macos: bool, cuda: bool, vulkan: bool) -> GpuKind {
    if macos {
        GpuKind::Metal
    } else if cuda {
        GpuKind::Cuda
    } else if vulkan {
        GpuKind::Vulkan
    } else {
        GpuKind::None
    }
}

fn cuda_present() -> bool {
    // ponytail: a device node or CUDA_PATH, not a driver query.
    std::env::var_os("CUDA_PATH").is_some()
        || std::path::Path::new("/dev/nvidia0").exists()
        || std::path::Path::new("C:\\Windows\\System32\\nvcuda.dll").exists()
}

fn vulkan_present() -> bool {
    std::env::var_os("VULKAN_SDK").is_some()
        || std::path::Path::new("/usr/lib/x86_64-linux-gnu/libvulkan.so.1").exists()
        || std::path::Path::new("/usr/lib/libvulkan.so.1").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_class_prefers_metal_then_cuda_then_vulkan() {
        assert_eq!(classify_gpu(true, true, true), GpuKind::Metal);
        assert_eq!(classify_gpu(false, true, true), GpuKind::Cuda);
        assert_eq!(classify_gpu(false, false, true), GpuKind::Vulkan);
        assert_eq!(classify_gpu(false, false, false), GpuKind::None);
    }
}
