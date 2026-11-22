use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use ort::environment::Environment;
use ort::value::{DynTensor, Tensor, TensorRef};

use super::accel::{detected_accel, AccelInfo, ExecutionProviderKind, SessionProfile};

fn global_env() -> Result<Arc<Environment>, String> {
    static ENV: OnceLock<Result<Arc<Environment>, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        ort::environment::init()
            .with_name("podtekst")
            .build()
            .map(Arc::new)
            .map_err(|e| format!("ort environment: {e}"))
    })
    .clone()
}

pub type ShapeData = (Vec<i64>, Vec<f32>);
pub type ShapeDataI64 = (Vec<i64>, Vec<i64>);
pub type ShapeDataBool = (Vec<i64>, Vec<bool>);
pub type NamedTensors = HashMap<String, ShapeData>;

pub struct OrtSession {
    session: ort::session::Session,
    pub input_names: Vec<String>,
    pub output_names: Vec<String>,
    label: String,
    provider: ExecutionProviderKind,
    device_name: Option<String>,
}

pub(crate) fn outputs_are_finite(outputs: &NamedTensors) -> bool {
    !outputs.is_empty()
        && outputs
            .values()
            .all(|(_, data)| !data.is_empty() && data.iter().all(|x| x.is_finite()))
}

impl OrtSession {
    pub fn load(path: &Path, intra_threads: usize) -> Result<Self, String> {
        Self::load_with_profile(path, intra_threads, SessionProfile::CpuOnly)
    }

    pub fn load_with_profile(
        path: &Path,
        intra_threads: usize,
        profile: SessionProfile,
    ) -> Result<Self, String> {
        Self::load_with_probe(path, intra_threads, profile, |_| Ok(()))
    }

    pub fn load_with_probe<F>(
        path: &Path,
        intra_threads: usize,
        profile: SessionProfile,
        probe: F,
    ) -> Result<Self, String>
    where
        F: Fn(&mut Self) -> Result<(), String>,
    {
        let started = std::time::Instant::now();
        let env = global_env()?;
        let accel = detected_accel();
        let candidates = accel.candidates(profile);
        let file_label = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut loaded: Option<Self> = None;
        for provider in candidates {
            match Self::try_provider(&env, path, intra_threads, provider, accel) {
                Ok(mut session) => {
                    if provider != ExecutionProviderKind::Cpu {
                        if let Err(err) = probe(&mut session) {
                            log::warn!(
                                target: "model",
                                file = file_label.as_str(),
                                provider = provider.as_str(),
                                stage = "probe",
                                error = err.as_str();
                                "ev=model.gpu_fallback"
                            );
                            continue;
                        }
                    }
                    loaded = Some(session);
                    break;
                }
                Err((stage, err)) => {
                    if provider == ExecutionProviderKind::Cpu {
                        return Err(err);
                    }
                    log::warn!(
                        target: "model",
                        file = file_label.as_str(),
                        provider = provider.as_str(),
                        stage = stage,
                        error = err.as_str();
                        "ev=model.gpu_fallback"
                    );
                }
            }
        }

        let loaded = loaded.ok_or_else(|| format!("не удалось загрузить {file_label}"))?;
        log::info!(
            target: "model",
            file = loaded.label.clone(),
            provider = loaded.provider.as_str(),
            device = loaded.device_name.as_deref().unwrap_or_else(|| loaded.provider.as_str()),
            bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            elapsed_ms = started.elapsed().as_millis() as u64,
            inputs = format!("{:?}", loaded.input_names),
            outputs = format!("{:?}", loaded.output_names),
            threads = intra_threads;
            "ev=model.load"
        );
        Ok(loaded)
    }

    pub fn provider(&self) -> ExecutionProviderKind {
        self.provider
    }

    pub fn device_name(&self) -> Option<&str> {
        self.device_name.as_deref()
    }

    fn try_provider(
        env: &Arc<Environment>,
        path: &Path,
        intra_threads: usize,
        provider: ExecutionProviderKind,
        accel: &AccelInfo,
    ) -> Result<Self, (&'static str, String)> {
        // В DirectML memory pattern на динамических формах провоцирует утечки VRAM и сбои.
        let memory_pattern = provider != ExecutionProviderKind::DirectMl;
        let builder =
            Self::base_builder(env, intra_threads, memory_pattern).map_err(|e| ("builder", e))?;
        let (mut builder, device_name) =
            Self::attach_provider(builder, provider, accel).map_err(|e| ("register", e))?;
        let session = builder
            .commit_from_file(path)
            .map_err(|e| ("commit", e.to_string()))?;
        Ok(Self::from_session(session, path, provider, device_name))
    }

    fn attach_provider(
        builder: ort::session::builder::SessionBuilder,
        provider: ExecutionProviderKind,
        accel: &AccelInfo,
    ) -> Result<(ort::session::builder::SessionBuilder, Option<String>), String> {
        match provider {
            ExecutionProviderKind::Cpu => Ok((builder, None)),
            ExecutionProviderKind::Cuda => {
                #[cfg(feature = "ort-cuda")]
                {
                    let mut cuda = ort::ep::cuda::CUDA::default()
                        .with_device_id(accel.cuda_device_id.unwrap_or(0))
                        .with_conv_algorithm_search(ort::ep::cuda::ConvAlgorithmSearch::Heuristic)
                        .with_arena_extend_strategy(ort::ep::ArenaExtendStrategy::SameAsRequested);
                    if let Some(limit) = super::accel::cuda_memory_limit(accel.cuda_vram_bytes) {
                        cuda = cuda.with_memory_limit(limit);
                    }
                    let builder = builder
                        .with_execution_providers([cuda.build().error_on_failure()])
                        .map_err(|e| e.to_string())?;
                    Ok((builder, accel.cuda_device_name.clone()))
                }
                #[cfg(not(feature = "ort-cuda"))]
                {
                    let _ = (builder, accel);
                    Err("CUDA EP не включён в сборку".to_string())
                }
            }
            ExecutionProviderKind::DirectMl => {
                #[cfg(feature = "ort-directml")]
                {
                    let adapter = accel
                        .dml_adapter
                        .as_ref()
                        .ok_or_else(|| "DirectML-адаптер не обнаружен".to_string())?;
                    let ep = ort::ep::directml::DirectML::default()
                        .with_device_id(adapter.device_id)
                        .build()
                        .error_on_failure();
                    let builder = builder
                        .with_execution_providers([ep])
                        .map_err(|e| e.to_string())?;
                    Ok((builder, Some(adapter.name.clone())))
                }
                #[cfg(not(feature = "ort-directml"))]
                {
                    let _ = (builder, accel);
                    Err("DirectML EP не включён в сборку".to_string())
                }
            }
            ExecutionProviderKind::CoreMl => {
                #[cfg(feature = "ort-coreml")]
                {
                    use ort::ep::coreml::{
                        ComputeUnits, CoreML, ModelFormat, SpecializationStrategy,
                    };
                    let _ = accel;
                    let ep = CoreML::default()
                        .with_model_format(ModelFormat::MLProgram)
                        .with_compute_units(ComputeUnits::All)
                        .with_specialization_strategy(SpecializationStrategy::FastPrediction)
                        .build()
                        .error_on_failure();
                    let builder = builder
                        .with_execution_providers([ep])
                        .map_err(|e| e.to_string())?;
                    Ok((builder, Some("Apple CoreML".to_string())))
                }
                #[cfg(not(feature = "ort-coreml"))]
                {
                    let _ = (builder, accel);
                    Err("CoreML EP не включён в сборку".to_string())
                }
            }
            ExecutionProviderKind::Nnapi => {
                #[cfg(feature = "ort-nnapi")]
                {
                    let ep = ort::ep::nnapi::NNAPI::default()
                        .with_disable_cpu(true)
                        .build()
                        .error_on_failure();
                    let builder = builder
                        .with_execution_providers([ep])
                        .map_err(|e| e.to_string())?;
                    Ok((builder, accel.nnapi_device_name.clone()))
                }
                #[cfg(not(feature = "ort-nnapi"))]
                {
                    let _ = (builder, accel);
                    Err("NNAPI EP не включён в сборку".to_string())
                }
            }
        }
    }

    fn base_builder(
        env: &Arc<Environment>,
        intra_threads: usize,
        memory_pattern: bool,
    ) -> Result<ort::session::builder::SessionBuilder, String> {
        let mut builder = ort::session::Session::builder(env).map_err(|e| e.to_string())?;
        // Ограничение ORT: при 0 захватывает все ядра и на гибридных CPU
        // простаивает на энергоэффективных (E) ядрах.
        let threads = if intra_threads > 0 {
            intra_threads
        } else {
            (std::thread::available_parallelism()
                .map(|n| n.get() / 2)
                .unwrap_or(4))
            .clamp(2, 6)
        };
        builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?
            .with_intra_threads(threads)
            .map_err(|e| e.to_string())?
            .with_inter_threads(1)
            .map_err(|e| e.to_string())?
            .with_parallel_execution(false)
            .map_err(|e| e.to_string())?
            .with_memory_pattern(memory_pattern)
            .map_err(|e| e.to_string())?;
        Ok(builder)
    }

    fn from_session(
        session: ort::session::Session,
        path: &Path,
        provider: ExecutionProviderKind,
        device_name: Option<String>,
    ) -> Self {
        let input_names = session
            .inputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect();
        let output_names = session
            .outputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect();
        Self {
            session,
            input_names,
            output_names,
            label: path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            provider,
            device_name,
        }
    }

    pub fn has_input(&self, name: &str) -> bool {
        self.input_names.iter().any(|n| n == name)
    }

    // Контракт ONNX: -1 в первой оси означает динамический размер батча.
    pub fn input_shape(&self, name: &str) -> Option<Vec<i64>> {
        let input = self.session.inputs().iter().find(|i| i.name() == name)?;
        match input.dtype() {
            ort::value::ValueType::Tensor { shape, .. } => Some(shape.to_vec()),
            _ => None,
        }
    }

    fn tensor_f32(shape: Vec<i64>, data: Vec<f32>) -> Result<DynTensor, String> {
        Tensor::from_array((shape, data))
            .map(|t| t.upcast())
            .map_err(|e| e.to_string())
    }

    fn tensor_i64(shape: Vec<i64>, data: Vec<i64>) -> Result<DynTensor, String> {
        Tensor::from_array((shape, data))
            .map(|t| t.upcast())
            .map_err(|e| e.to_string())
    }

    fn collect_outputs(
        output_names: &[String],
        outputs: ort::session::SessionOutputs,
    ) -> Result<NamedTensors, String> {
        let mut out = HashMap::new();
        for name in output_names.iter().cloned() {
            let value = &outputs[name.as_str()];
            // f32 напрямую; длины/маски (i64/i32) приводим к f32 — вызывающий игнорирует лишнее
            if let Ok((shape, data)) = value.try_extract_tensor::<f32>() {
                out.insert(name, (shape.to_vec(), data.to_vec()));
                continue;
            }
            if let Ok((shape, data)) = value.try_extract_tensor::<i64>() {
                out.insert(
                    name,
                    (shape.to_vec(), data.iter().map(|&x| x as f32).collect()),
                );
                continue;
            }
            if let Ok((shape, data)) = value.try_extract_tensor::<i32>() {
                out.insert(
                    name,
                    (shape.to_vec(), data.iter().map(|&x| x as f32).collect()),
                );
                continue;
            }
            return Err(format!("неподдерживаемый тип выхода {name}"));
        }
        Ok(out)
    }

    /// Прогон: f32-тензоры `{имя: (shape, flat)}` → `{имя_выхода: (shape, flat)}`.
    /// Вход передаётся по значению: данные уходят в рантайм без копирования
    /// (на шаге декодера это мегабайты состояний энкодера).
    pub fn run_f32(&mut self, inputs: NamedTensors) -> Result<NamedTensors, String> {
        self.run_mixed_full(inputs, HashMap::new(), HashMap::new())
    }

    pub fn run_mixed(
        &mut self,
        f32_inputs: NamedTensors,
        i64_inputs: HashMap<String, ShapeDataI64>,
    ) -> Result<NamedTensors, String> {
        let empty_bool: HashMap<String, ShapeDataBool> = HashMap::new();
        self.run_mixed_full(f32_inputs, i64_inputs, empty_bool)
    }

    pub fn run_mixed_full(
        &mut self,
        f32_inputs: NamedTensors,
        i64_inputs: HashMap<String, ShapeDataI64>,
        bool_inputs: HashMap<String, ShapeDataBool>,
    ) -> Result<NamedTensors, String> {
        let mut names: Vec<String> = Vec::new();
        let mut tensors: Vec<DynTensor> = Vec::new();
        for (name, (shape, data)) in f32_inputs {
            tensors.push(Self::tensor_f32(shape, data)?);
            names.push(name);
        }
        for (name, (shape, data)) in i64_inputs {
            tensors.push(Self::tensor_i64(shape, data)?);
            names.push(name);
        }
        for (name, (shape, data)) in bool_inputs {
            tensors.push(Self::tensor_bool(shape, data)?);
            names.push(name);
        }
        let map: HashMap<&str, DynTensor> = names.iter().map(|n| n.as_str()).zip(tensors).collect();
        let outputs = self.session.run(map).map_err(|error| {
            log::error!(
                target: "model",
                model = self.label,
                inputs = format!("{names:?}"),
                error = error.to_string();
                "ev=model.infer_error"
            );
            error.to_string()
        })?;
        Self::collect_outputs(&self.output_names, outputs)
    }

    /// Запуск с заимствованными входными буферами: значения не копируются в
    /// `Vec` на каждом шаге. Нужен для Whisper-декодера, где KV-кэш — десятки
    /// мегабайт на токен.
    pub fn run_mixed_views<'a>(
        &mut self,
        f32_inputs: Vec<(&'a str, Vec<i64>, &'a [f32])>,
        i64_inputs: Vec<(&'a str, Vec<i64>, &'a [i64])>,
        bool_inputs: Vec<(&'a str, Vec<i64>, &'a [bool])>,
    ) -> Result<NamedTensors, String> {
        use ort::session::SessionInputValue;
        let mut map: HashMap<&str, SessionInputValue<'a>> = HashMap::new();
        for (name, shape, data) in f32_inputs {
            let value = TensorRef::from_array_view((shape, data))
                .map_err(|e| e.to_string())?
                .into_dyn();
            map.insert(name, value.into());
        }
        for (name, shape, data) in i64_inputs {
            let value = TensorRef::from_array_view((shape, data))
                .map_err(|e| e.to_string())?
                .into_dyn();
            map.insert(name, value.into());
        }
        for (name, shape, data) in bool_inputs {
            let value = TensorRef::from_array_view((shape, data))
                .map_err(|e| e.to_string())?
                .into_dyn();
            map.insert(name, value.into());
        }
        let outputs = self.session.run(map).map_err(|error| {
            log::error!(
                target: "model",
                model = self.label,
                error = error.to_string();
                "ev=model.infer_error"
            );
            error.to_string()
        })?;
        Self::collect_outputs(&self.output_names, outputs)
    }

    fn tensor_bool(shape: Vec<i64>, data: Vec<bool>) -> Result<DynTensor, String> {
        Tensor::from_array((shape, data))
            .map(|t| t.upcast())
            .map_err(|e| e.to_string())
    }
}
