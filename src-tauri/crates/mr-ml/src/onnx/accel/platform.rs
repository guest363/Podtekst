use super::AccelInfo;

#[cfg(all(
    target_os = "windows",
    any(feature = "ort-cuda", feature = "ort-directml")
))]
fn enumerate_dxgi_adapters() -> Vec<super::AdapterCandidate> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND,
    };

    // SAFETY: стандартный вызов COM DXGI на Windows для чтения списка адаптеров.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for idx in 0..32u32 {
        // SAFETY: `EnumAdapters1` возвращает новый COM-указатель либо `DXGI_ERROR_NOT_FOUND`.
        let adapter = match unsafe { factory.EnumAdapters1(idx) } {
            Ok(adapter) => adapter,
            Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(_) => break,
        };
        // SAFETY: валидный экземпляр `IDXGIAdapter1`, заполняется структура `DXGI_ADAPTER_DESC1`.
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        let end = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..end])
            .trim()
            .to_string();
        let is_software = (desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32)) != 0;
        out.push(super::AdapterCandidate {
            #[cfg(any(all(target_os = "windows", feature = "ort-directml"), test))]
            device_id: idx as i32,
            vendor_id: desc.VendorId,
            vram_bytes: desc.DedicatedVideoMemory as u64,
            is_software,
            name,
        });
    }
    out
}

#[cfg(all(target_os = "windows", feature = "ort-cuda"))]
fn probe_cuda_runtime_windows() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows::Win32::System::LibraryLoader::{
        LoadLibraryExW, LoadLibraryW, DONT_RESOLVE_DLL_REFERENCES,
    };

    const DLLS: &[&str] = &[
        "cudart64_13.dll",
        "cublasLt64_13.dll",
        "cublas64_13.dll",
        "cufft64_12.dll",
        "cudnn64_9.dll",
        "cudnn_ops64_9.dll",
        "cudnn_cnn64_9.dll",
        "onnxruntime_providers_shared.dll",
    ];
    let mut loaded: Vec<HMODULE> = Vec::with_capacity(DLLS.len());
    for name in DLLS {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: нуль-терминированная UTF-16 строка имени системной/CUDA DLL.
        match unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) } {
            Ok(handle) => loaded.push(handle),
            Err(_) => {
                for handle in loaded.into_iter().rev() {
                    // SAFETY: дескриптор только что получен от `LoadLibraryW` в этом же цикле.
                    unsafe {
                        let _ = FreeLibrary(handle);
                    }
                }
                return false;
            }
        }
    }
    // `onnxruntime_providers_cuda.dll` в `DllMain` вызывает `Provider_GetHost()`, поэтому
    // до инициализации ORT проверяем её наличие без исполнения `DllMain`.
    let cuda_provider: Vec<u16> = "onnxruntime_providers_cuda.dll"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: нуль-терминированная UTF-16 строка имени провайдера ORT.
    match unsafe {
        LoadLibraryExW(
            PCWSTR(cuda_provider.as_ptr()),
            None,
            DONT_RESOLVE_DLL_REFERENCES,
        )
    } {
        Ok(handle) => {
            // SAFETY: дескриптор только что получен от `LoadLibraryExW`.
            unsafe {
                let _ = FreeLibrary(handle);
            }
            true
        }
        Err(_) => {
            for handle in loaded.into_iter().rev() {
                // SAFETY: дескриптор только что получен от `LoadLibraryW` выше.
                unsafe {
                    let _ = FreeLibrary(handle);
                }
            }
            false
        }
    }
}

#[cfg(any(
    all(target_os = "linux", feature = "ort-cuda"),
    all(target_os = "android", feature = "ort-nnapi")
))]
struct DlHandle(*mut libc::c_void);

#[cfg(any(
    all(target_os = "linux", feature = "ort-cuda"),
    all(target_os = "android", feature = "ort-nnapi")
))]
impl DlHandle {
    fn open(name: &core::ffi::CStr) -> Option<Self> {
        // SAFETY: нуль-терминированное имя или путь к разделяемой библиотеке.
        let ptr = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    #[cfg(all(target_os = "linux", feature = "ort-cuda"))]
    fn pin(self) {
        let _ = core::mem::ManuallyDrop::new(self);
    }
}

#[cfg(any(
    all(target_os = "linux", feature = "ort-cuda"),
    all(target_os = "android", feature = "ort-nnapi")
))]
impl Drop for DlHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: дескриптор открыт через `libc::dlopen` в `DlHandle::open`.
            unsafe {
                libc::dlclose(self.0);
            }
        }
    }
}

#[cfg(any(
    all(target_os = "linux", feature = "ort-cuda"),
    all(target_os = "android", feature = "ort-nnapi")
))]
macro_rules! dlsym_fn {
    ($handle:expr, $sym:expr, $ty:ty) => {{
        // SAFETY: валидный дескриптор `dlopen` и нуль-терминированное имя экспортируемого символа.
        let ptr = unsafe { libc::dlsym($handle.0, $sym.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            // SAFETY: ненулевой указатель на функцию C ABI с сигнатурой `$ty`.
            Some(unsafe { std::mem::transmute::<*mut libc::c_void, $ty>(ptr) })
        }
    }};
}

#[cfg(all(target_os = "linux", feature = "ort-cuda"))]
fn probe_cuda_devices_linux() -> (Option<DlHandle>, Vec<super::CudaDeviceCandidate>) {
    let Some(lib) = DlHandle::open(c"libcuda.so.1") else {
        return (None, Vec::new());
    };
    let (Some(cu_init), Some(cu_count), Some(cu_get), Some(cu_mem), Some(cu_name)) = (
        dlsym_fn!(lib, c"cuInit", unsafe extern "C" fn(u32) -> libc::c_int),
        dlsym_fn!(
            lib,
            c"cuDeviceGetCount",
            unsafe extern "C" fn(*mut libc::c_int) -> libc::c_int
        ),
        dlsym_fn!(
            lib,
            c"cuDeviceGet",
            unsafe extern "C" fn(*mut libc::c_int, libc::c_int) -> libc::c_int
        ),
        dlsym_fn!(
            lib,
            c"cuDeviceTotalMem_v2",
            unsafe extern "C" fn(*mut usize, libc::c_int) -> libc::c_int
        ),
        dlsym_fn!(
            lib,
            c"cuDeviceGetName",
            unsafe extern "C" fn(*mut std::ffi::c_char, libc::c_int, libc::c_int) -> libc::c_int
        ),
    ) else {
        return (None, Vec::new());
    };
    // SAFETY: стандартная инициализация драйвера CUDA с флагами 0.
    if unsafe { cu_init(0) } != 0 {
        return (None, Vec::new());
    }
    let mut count: libc::c_int = 0;
    // SAFETY: `count` — валидный указатель на стек.
    if unsafe { cu_count(&mut count) } != 0 || count <= 0 {
        return (None, Vec::new());
    }
    let mut out = Vec::with_capacity(count.min(16) as usize);
    for ordinal in 0..count.min(16) {
        let mut dev: libc::c_int = 0;
        // SAFETY: `ordinal` находится в диапазоне `0..count`.
        if unsafe { cu_get(&mut dev, ordinal) } != 0 {
            continue;
        }
        let mut vram: usize = 0;
        // SAFETY: `dev` получен от `cuDeviceGet`.
        if unsafe { cu_mem(&mut vram, dev) } != 0 {
            continue;
        }
        let mut buf = [0 as std::ffi::c_char; 128];
        // SAFETY: буфер имени гарантированно не меньше 128 байт.
        let name = if unsafe { cu_name(buf.as_mut_ptr(), buf.len() as libc::c_int, dev) } == 0 {
            buf[buf.len() - 1] = 0;
            // SAFETY: последний байт обнулён, буфер валиден для чтения до нуля.
            unsafe { core::ffi::CStr::from_ptr(buf.as_ptr()) }
                .to_string_lossy()
                .trim()
                .to_string()
        } else {
            format!("CUDA GPU {dev}")
        };
        out.push(super::CudaDeviceCandidate {
            device_id: dev,
            vram_bytes: vram as u64,
            name,
        });
    }
    (Some(lib), out)
}

#[cfg(all(target_os = "linux", feature = "ort-cuda"))]
fn dlopen_linux_lib(exe_dir: Option<&std::path::Path>, name: &str) -> Option<DlHandle> {
    use std::os::unix::ffi::OsStrExt;
    if let Some(dir) = exe_dir {
        let candidate = dir.join(name);
        if candidate.is_file() {
            if let Ok(cpath) = std::ffi::CString::new(candidate.as_os_str().as_bytes()) {
                if let Some(handle) = DlHandle::open(&cpath) {
                    return Some(handle);
                }
            }
        }
    }
    let cname = std::ffi::CString::new(name).ok()?;
    DlHandle::open(&cname)
}

#[cfg(all(target_os = "linux", feature = "ort-cuda"))]
fn probe_cuda_runtime_linux() -> Option<Vec<DlHandle>> {
    // Провайдерные .so обычно лежат рядом с бинарником, а системный CUDA — в путях линковщика.
    const LIBS: &[&str] = &[
        "libcudart.so.13",
        "libcublasLt.so.13",
        "libcublas.so.13",
        "libcudnn.so.9",
        "libonnxruntime_providers_shared.so",
        "libonnxruntime_providers_cuda.so",
    ];
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    let mut loaded: Vec<DlHandle> = Vec::with_capacity(LIBS.len());
    for name in LIBS {
        let Some(handle) = dlopen_linux_lib(exe_dir.as_deref(), name) else {
            // Обратный порядок закрытия соблюдает граф зависимостей при ошибке загрузки.
            while loaded.pop().is_some() {}
            return None;
        };
        loaded.push(handle);
    }
    Some(loaded)
}

#[cfg(all(target_os = "android", feature = "ort-nnapi"))]
fn probe_nnapi_android() -> Option<String> {
    if !<ort::ep::NNAPI as ort::ep::ExecutionProvider>::is_available().unwrap_or(false) {
        return None;
    }
    let lib = DlHandle::open(c"libneuralnetworks.so")?;
    // Символы перечисления устройств NNAPI появились в Android API 29; на API 24–28 уходим в CPU.
    let (Some(get_count), Some(get_dev), Some(get_type), Some(get_name)) = (
        dlsym_fn!(
            lib,
            c"ANeuralNetworks_getDeviceCount",
            unsafe extern "C" fn(*mut u32) -> i32
        ),
        dlsym_fn!(
            lib,
            c"ANeuralNetworks_getDevice",
            unsafe extern "C" fn(u32, *mut *mut libc::c_void) -> i32
        ),
        dlsym_fn!(
            lib,
            c"ANeuralNetworksDevice_getType",
            unsafe extern "C" fn(*const libc::c_void, *mut i32) -> i32
        ),
        dlsym_fn!(
            lib,
            c"ANeuralNetworksDevice_getName",
            unsafe extern "C" fn(*const libc::c_void, *mut *const std::ffi::c_char) -> i32
        ),
    ) else {
        return None;
    };
    let mut count = 0u32;
    // SAFETY: `count` — валидный указатель на стек.
    if unsafe { get_count(&mut count) } != 0 || count == 0 {
        return None;
    }
    let mut candidates = Vec::with_capacity(count.min(32) as usize);
    for idx in 0..count.min(32) {
        let mut dev: *mut libc::c_void = core::ptr::null_mut();
        // SAFETY: `idx < count`, указатель заполняется библиотекой NNAPI.
        if unsafe { get_dev(idx, &mut dev) } != 0 || dev.is_null() {
            continue;
        }
        let mut dev_type = 0i32;
        // SAFETY: ненулевой указатель на `ANeuralNetworksDevice`.
        if unsafe { get_type(dev, &mut dev_type) } != 0 {
            continue;
        }
        let mut name_ptr: *const std::ffi::c_char = core::ptr::null();
        // SAFETY: `getName` записывает статическую нуль-терминированную C-строку процесса.
        let name = if unsafe { get_name(dev, &mut name_ptr) } == 0 && !name_ptr.is_null() {
            // SAFETY: статическая C-строка устройства NNAPI.
            unsafe { core::ffi::CStr::from_ptr(name_ptr) }
                .to_string_lossy()
                .trim()
                .to_string()
        } else {
            String::new()
        };
        candidates.push(super::NnapiDeviceCandidate {
            device_type: dev_type,
            name,
        });
    }
    super::select_nnapi_device(&candidates)
}

pub(super) fn detect_accel() -> AccelInfo {
    #[cfg(all(
        target_os = "windows",
        any(feature = "ort-cuda", feature = "ort-directml")
    ))]
    let adapters = enumerate_dxgi_adapters();

    #[cfg(all(target_os = "windows", feature = "ort-cuda"))]
    let (cuda_device_id, cuda_device_name, cuda_vram_bytes) =
        match super::select_best_nvidia_adapter(&adapters) {
            Some(nvidia)
                if <ort::ep::CUDA as ort::ep::ExecutionProvider>::is_available()
                    .unwrap_or(false)
                    && probe_cuda_runtime_windows() =>
            {
                (Some(nvidia.device_id), Some(nvidia.name), nvidia.vram_bytes)
            }
            _ => (None, None, 0),
        };
    #[cfg(all(target_os = "linux", feature = "ort-cuda"))]
    let (cuda_device_id, cuda_device_name, cuda_vram_bytes) = {
        let (driver_lib, devices) = probe_cuda_devices_linux();
        match (driver_lib, super::select_best_cuda_device(&devices)) {
            (Some(driver), Some(gpu))
                if <ort::ep::CUDA as ort::ep::ExecutionProvider>::is_available()
                    .unwrap_or(false) =>
            {
                if let Some(runtime_libs) = probe_cuda_runtime_linux() {
                    // Удерживаем открытые .so, чтобы повторная загрузка в ORT не сбросила драйвер CUDA.
                    driver.pin();
                    for lib in runtime_libs {
                        lib.pin();
                    }
                    (Some(gpu.device_id), Some(gpu.name), gpu.vram_bytes)
                } else {
                    (None, None, 0)
                }
            }
            _ => (None, None, 0),
        }
    };
    #[cfg(not(any(
        all(target_os = "windows", feature = "ort-cuda"),
        all(target_os = "linux", feature = "ort-cuda")
    )))]
    let (cuda_device_id, cuda_device_name, cuda_vram_bytes) = (None, None, 0);

    #[cfg(all(target_os = "windows", feature = "ort-directml"))]
    let dml_adapter = super::select_best_adapter(&adapters).filter(|_| {
        <ort::ep::DirectML as ort::ep::ExecutionProvider>::is_available().unwrap_or(false)
            && ort::ep::directml::api().is_some()
    });
    #[cfg(not(all(target_os = "windows", feature = "ort-directml")))]
    let dml_adapter = None;

    #[cfg(all(target_os = "macos", feature = "ort-coreml"))]
    let coreml_available =
        <ort::ep::CoreML as ort::ep::ExecutionProvider>::is_available().unwrap_or(false);
    #[cfg(not(all(target_os = "macos", feature = "ort-coreml")))]
    let coreml_available = false;

    #[cfg(all(target_os = "android", feature = "ort-nnapi"))]
    let nnapi_device_name = probe_nnapi_android();
    #[cfg(not(all(target_os = "android", feature = "ort-nnapi")))]
    let nnapi_device_name = None;

    AccelInfo {
        cuda_device_id,
        cuda_device_name,
        cuda_vram_bytes,
        dml_adapter,
        coreml_available,
        nnapi_device_name,
    }
}
