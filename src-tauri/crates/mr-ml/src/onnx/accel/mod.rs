use std::sync::OnceLock;

mod platform;

pub const MIN_VRAM_BYTES: u64 = 1536 * 1024 * 1024;
#[cfg(any(all(target_os = "windows", feature = "ort-cuda"), test))]
const NVIDIA_VENDOR_ID: u32 = 0x10DE;
#[cfg(any(feature = "ort-cuda", test))]
const CUDA_VRAM_HEADROOM_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(any(feature = "ort-cuda", test))]
const MIN_CUDA_MEM_LIMIT_BYTES: u64 = 1024 * 1024 * 1024;

#[cfg(any(all(target_os = "android", feature = "ort-nnapi"), test))]
pub(super) const NNAPI_DEVICE_GPU: i32 = 3;
#[cfg(any(all(target_os = "android", feature = "ort-nnapi"), test))]
pub(super) const NNAPI_DEVICE_ACCELERATOR: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionProviderKind {
    Cuda,
    DirectMl,
    CoreMl,
    Nnapi,
    Cpu,
}

impl ExecutionProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cuda => "cuda",
            Self::DirectMl => "directml",
            Self::CoreMl => "coreml",
            Self::Nnapi => "nnapi",
            Self::Cpu => "cpu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProfile {
    CpuOnly,
    GpuPreferred,
    CudaPreferred,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterInfo {
    pub device_id: i32,
    pub vendor_id: u32,
    pub vram_bytes: u64,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct AccelInfo {
    pub cuda_device_id: Option<i32>,
    pub cuda_device_name: Option<String>,
    pub cuda_vram_bytes: u64,
    pub dml_adapter: Option<AdapterInfo>,
    pub coreml_available: bool,
    pub nnapi_device_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ProviderAvailability {
    cuda: bool,
    directml: bool,
    coreml: bool,
    nnapi: bool,
}

#[cfg(any(
    all(
        target_os = "windows",
        any(feature = "ort-cuda", feature = "ort-directml")
    ),
    test
))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AdapterCandidate {
    #[cfg(any(all(target_os = "windows", feature = "ort-directml"), test))]
    pub(super) device_id: i32,
    pub(super) vendor_id: u32,
    pub(super) vram_bytes: u64,
    pub(super) is_software: bool,
    pub(super) name: String,
}

#[cfg(any(all(target_os = "linux", feature = "ort-cuda"), test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CudaDeviceCandidate {
    pub(super) device_id: i32,
    pub(super) vram_bytes: u64,
    pub(super) name: String,
}

#[cfg(any(all(target_os = "android", feature = "ort-nnapi"), test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NnapiDeviceCandidate {
    pub(super) device_type: i32,
    pub(super) name: String,
}

impl AccelInfo {
    fn availability(&self) -> ProviderAvailability {
        ProviderAvailability {
            cuda: self.cuda_device_id.is_some(),
            directml: self.dml_adapter.is_some(),
            coreml: self.coreml_available,
            nnapi: self.nnapi_device_name.is_some(),
        }
    }

    pub(crate) fn candidates(&self, profile: SessionProfile) -> Vec<ExecutionProviderKind> {
        candidate_providers(profile, self.availability())
    }
}

fn candidate_providers(
    profile: SessionProfile,
    avail: ProviderAvailability,
) -> Vec<ExecutionProviderKind> {
    let mut out = Vec::with_capacity(5);
    match profile {
        SessionProfile::CpuOnly => {}
        SessionProfile::GpuPreferred => {
            if avail.cuda {
                out.push(ExecutionProviderKind::Cuda);
            }
            if avail.directml {
                out.push(ExecutionProviderKind::DirectMl);
            }
            if avail.coreml {
                out.push(ExecutionProviderKind::CoreMl);
            }
            if avail.nnapi {
                out.push(ExecutionProviderKind::Nnapi);
            }
        }
        SessionProfile::CudaPreferred => {
            if avail.cuda {
                out.push(ExecutionProviderKind::Cuda);
            }
        }
    }
    out.push(ExecutionProviderKind::Cpu);
    out
}

#[cfg(any(feature = "ort-cuda", test))]
pub(crate) fn cuda_memory_limit(vram_bytes: u64) -> Option<usize> {
    if vram_bytes == 0 {
        return None;
    }
    let pct75 = (vram_bytes / 4).saturating_mul(3);
    let with_headroom = vram_bytes.saturating_sub(CUDA_VRAM_HEADROOM_BYTES);
    let limit = pct75
        .min(with_headroom)
        .max(MIN_CUDA_MEM_LIMIT_BYTES)
        .min(vram_bytes);
    usize::try_from(limit).ok()
}

#[cfg(any(all(target_os = "windows", feature = "ort-directml"), test))]
pub(super) fn select_best_adapter(adapters: &[AdapterCandidate]) -> Option<AdapterInfo> {
    adapters
        .iter()
        .filter(|a| !a.is_software && a.vram_bytes >= MIN_VRAM_BYTES)
        .max_by_key(|a| a.vram_bytes)
        .map(|a| AdapterInfo {
            device_id: a.device_id,
            vendor_id: a.vendor_id,
            vram_bytes: a.vram_bytes,
            name: a.name.clone(),
        })
}

#[cfg(any(all(target_os = "windows", feature = "ort-cuda"), test))]
pub(super) fn select_best_nvidia_adapter(adapters: &[AdapterCandidate]) -> Option<AdapterInfo> {
    adapters
        .iter()
        .filter(|a| !a.is_software && a.vendor_id == NVIDIA_VENDOR_ID)
        .enumerate()
        // Индекс CUDA считается по всем GPU NVIDIA до отсечения слабых по VRAM.
        .filter(|(_, a)| a.vram_bytes >= MIN_VRAM_BYTES)
        .max_by_key(|(_, a)| a.vram_bytes)
        .map(|(nvidia_idx, a)| AdapterInfo {
            // В CUDA нумерация идёт только по GPU NVIDIA, отдельно от индекса DXGI.
            device_id: nvidia_idx as i32,
            vendor_id: a.vendor_id,
            vram_bytes: a.vram_bytes,
            name: a.name.clone(),
        })
}

#[cfg(any(all(target_os = "linux", feature = "ort-cuda"), test))]
pub(super) fn select_best_cuda_device(
    devices: &[CudaDeviceCandidate],
) -> Option<CudaDeviceCandidate> {
    devices
        .iter()
        .filter(|d| d.vram_bytes >= MIN_VRAM_BYTES)
        .max_by_key(|d| d.vram_bytes)
        .cloned()
}

#[cfg(any(all(target_os = "android", feature = "ort-nnapi"), test))]
pub(super) fn select_nnapi_device(devices: &[NnapiDeviceCandidate]) -> Option<String> {
    devices
        .iter()
        .filter_map(|d| {
            // Эталонная CPU-реализация NNAPI медленнее штатных ядер ORT на CPU.
            if d.name.eq_ignore_ascii_case("nnapi-reference") {
                return None;
            }
            let priority = match d.device_type {
                NNAPI_DEVICE_ACCELERATOR => 2u8,
                NNAPI_DEVICE_GPU => 1u8,
                _ => return None,
            };
            Some((priority, d.name.trim()))
        })
        .rev()
        .max_by_key(|(priority, _)| *priority)
        .map(|(_, name)| {
            if name.is_empty() {
                "NNAPI".to_string()
            } else {
                format!("NNAPI ({name})")
            }
        })
}

pub fn detected_accel() -> &'static AccelInfo {
    static INFO: OnceLock<AccelInfo> = OnceLock::new();
    INFO.get_or_init(|| {
        let info = platform::detect_accel();
        let active_name = info
            .cuda_device_name
            .as_deref()
            .or_else(|| info.dml_adapter.as_ref().map(|a| a.name.as_str()))
            .or_else(|| info.coreml_available.then_some("Apple CoreML"))
            .or(info.nnapi_device_name.as_deref())
            .unwrap_or("none");
        let active_id = info
            .cuda_device_id
            .or_else(|| info.dml_adapter.as_ref().map(|a| a.device_id))
            .unwrap_or(-1);
        let vram_bytes = if info.cuda_vram_bytes > 0 {
            info.cuda_vram_bytes
        } else {
            info.dml_adapter.as_ref().map(|a| a.vram_bytes).unwrap_or(0)
        };
        log::info!(
            target: "model",
            cuda = info.cuda_device_id.is_some(),
            directml = info.dml_adapter.is_some(),
            coreml = info.coreml_available,
            nnapi = info.nnapi_device_name.is_some(),
            adapter = active_name,
            device_id = active_id,
            vram_mb = vram_bytes / (1024 * 1024);
            "ev=model.accel"
        );
        info
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(device_id: i32, vendor_id: u32, vram_mib: u64, is_software: bool) -> AdapterCandidate {
        AdapterCandidate {
            device_id,
            vendor_id,
            vram_bytes: vram_mib * 1024 * 1024,
            is_software,
            name: format!("GPU-{device_id}"),
        }
    }

    #[test]
    fn adapter_selection_filters_software_and_low_vram() {
        let list = [
            mk(0, 0x1414, 4096, true),
            mk(1, 0x8086, 1024, false),
            mk(2, 0x10DE, 10240, false),
            mk(3, 0x1002, 8192, false),
        ];
        let best = select_best_adapter(&list).expect("дискретный GPU");
        assert_eq!(best.device_id, 2);
        assert_eq!(best.vendor_id, NVIDIA_VENDOR_ID);

        let only_weak = [mk(0, 0x1414, 8192, true), mk(1, 0x8086, 512, false)];
        assert!(select_best_adapter(&only_weak).is_none());
    }

    #[test]
    fn nvidia_selection_ignores_other_vendors_and_reindexes_cuda() {
        let list = [
            mk(0, 0x8086, 2048, false),
            mk(1, 0x1002, 16384, false),
            mk(2, NVIDIA_VENDOR_ID, 1024, false),
            mk(3, NVIDIA_VENDOR_ID, 10240, false),
        ];
        let nv = select_best_nvidia_adapter(&list).expect("NVIDIA GPU");
        assert_eq!(nv.device_id, 1);
        assert_eq!(nv.vram_bytes, 10240 * 1024 * 1024);

        let amd_only = [mk(0, 0x1002, 16384, false)];
        assert!(select_best_nvidia_adapter(&amd_only).is_none());
    }

    #[test]
    fn cuda_memory_limit_respects_headroom_and_floor() {
        assert_eq!(cuda_memory_limit(0), None);
        let mib = 1024 * 1024usize;
        assert_eq!(cuda_memory_limit(1536 * mib as u64), Some(1024 * mib));
        assert_eq!(cuda_memory_limit(4096 * mib as u64), Some(3072 * mib));
        assert_eq!(cuda_memory_limit(10240 * mib as u64), Some(7680 * mib));
        assert_eq!(cuda_memory_limit(24576 * mib as u64), Some(18432 * mib));
    }

    #[test]
    fn linux_cuda_selection_picks_largest_above_vram_floor() {
        let mib = 1024 * 1024u64;
        let devices = [
            CudaDeviceCandidate {
                device_id: 0,
                vram_bytes: 1024 * mib,
                name: "GPU-small".to_string(),
            },
            CudaDeviceCandidate {
                device_id: 1,
                vram_bytes: 8192 * mib,
                name: "GPU-mid".to_string(),
            },
            CudaDeviceCandidate {
                device_id: 2,
                vram_bytes: 24576 * mib,
                name: "GPU-big".to_string(),
            },
        ];
        let chosen = select_best_cuda_device(&devices).expect("CUDA GPU");
        assert_eq!(chosen.device_id, 2);
        assert_eq!(chosen.name, "GPU-big");
        assert_eq!(chosen.vram_bytes, 24576 * mib);

        assert!(select_best_cuda_device(&devices[..1]).is_none());
    }

    #[test]
    fn nnapi_selection_prefers_accelerator_and_ignores_cpu() {
        let only_cpu = [
            NnapiDeviceCandidate {
                device_type: 2,
                name: "nnapi-reference".to_string(),
            },
            NnapiDeviceCandidate {
                device_type: 1,
                name: "other-cpu".to_string(),
            },
        ];
        assert!(select_nnapi_device(&only_cpu).is_none());

        let mixed = [
            NnapiDeviceCandidate {
                device_type: 2,
                name: "nnapi-reference".to_string(),
            },
            NnapiDeviceCandidate {
                device_type: NNAPI_DEVICE_GPU,
                name: "qti-gpu".to_string(),
            },
            NnapiDeviceCandidate {
                device_type: NNAPI_DEVICE_ACCELERATOR,
                name: "qti-htp".to_string(),
            },
        ];
        assert_eq!(
            select_nnapi_device(&mixed).as_deref(),
            Some("NNAPI (qti-htp)")
        );

        let gpu_only = [NnapiDeviceCandidate {
            device_type: NNAPI_DEVICE_GPU,
            name: "mali-gpu".to_string(),
        }];
        assert_eq!(
            select_nnapi_device(&gpu_only).as_deref(),
            Some("NNAPI (mali-gpu)")
        );
    }

    #[test]
    fn candidate_providers_follow_profile_and_priority() {
        let all = ProviderAvailability {
            cuda: true,
            directml: true,
            coreml: true,
            nnapi: true,
        };
        assert_eq!(
            candidate_providers(SessionProfile::CpuOnly, all),
            vec![ExecutionProviderKind::Cpu]
        );
        assert_eq!(
            candidate_providers(SessionProfile::GpuPreferred, all),
            vec![
                ExecutionProviderKind::Cuda,
                ExecutionProviderKind::DirectMl,
                ExecutionProviderKind::CoreMl,
                ExecutionProviderKind::Nnapi,
                ExecutionProviderKind::Cpu,
            ]
        );
        assert_eq!(
            candidate_providers(SessionProfile::CudaPreferred, all),
            vec![ExecutionProviderKind::Cuda, ExecutionProviderKind::Cpu]
        );
        let nnapi_only = ProviderAvailability {
            cuda: false,
            directml: false,
            coreml: false,
            nnapi: true,
        };
        assert_eq!(
            candidate_providers(SessionProfile::GpuPreferred, nnapi_only),
            vec![ExecutionProviderKind::Nnapi, ExecutionProviderKind::Cpu]
        );
        assert_eq!(
            candidate_providers(SessionProfile::CudaPreferred, nnapi_only),
            vec![ExecutionProviderKind::Cpu]
        );
    }
}
