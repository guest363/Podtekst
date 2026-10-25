//! Общая обёртка над `ort::session::Session`: загрузка, проброс тензоров, чтение выходов.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use ort::environment::Environment;
use ort::value::{DynTensor, Tensor};

fn global_env() -> Arc<Environment> {
    static ENV: OnceLock<Arc<Environment>> = OnceLock::new();
    ENV.get_or_init(|| {
        Arc::new(
            ort::environment::init()
                .with_name("meeting-recorder")
                .build()
                .expect("ort environment"),
        )
    })
    .clone()
}

/// Открытая ONNX-сессия с именами входов/выходов, прочитанными из графа.
/// Формат тензоров: (форма i64, плоские данные f32).
pub type ShapeData = (Vec<i64>, Vec<f32>);
pub type ShapeDataI64 = (Vec<i64>, Vec<i64>);
pub type ShapeDataBool = (Vec<i64>, Vec<bool>);
pub type NamedTensors = HashMap<String, ShapeData>;

pub struct OrtSession {
    session: ort::session::Session,
    pub input_names: Vec<String>,
    pub output_names: Vec<String>,
}

impl OrtSession {
    /// Загружает `.onnx` с CPU-провайдером и `intra_threads` потоками (0 = авто).
    /// При включённых cargo-фичах `ort-cuda`/`ort-directml`/`ort-coreml` сначала
    /// пробует аппаратный ускоритель; недоступен драйвер или библиотека —
    /// прозрачно возвращается к CPU.
    pub fn load(path: &Path, intra_threads: usize) -> Result<Self, String> {
        let env = global_env();
        #[cfg(any(feature = "ort-cuda", feature = "ort-directml", feature = "ort-coreml"))]
        match Self::accelerated(&env, path, intra_threads) {
            Ok(session) => return Ok(session),
            Err(e) => eprintln!("GPU-провайдер недоступен, работаем на CPU: {e}"),
        }
        let mut builder = Self::base_builder(&env, intra_threads)?;
        let session = builder.commit_from_file(path).map_err(|e| e.to_string())?;
        Ok(Self::from_session(session))
    }

    /// Пробует сессию на аппаратном EP (DirectML/CUDA/CoreML — какие собраны).
    #[cfg(any(feature = "ort-cuda", feature = "ort-directml", feature = "ort-coreml"))]
    fn accelerated(
        env: &Arc<Environment>,
        path: &Path,
        intra_threads: usize,
    ) -> Result<Self, String> {
        use ort::ep::ExecutionProviderDispatch;
        let mut eps: Vec<ExecutionProviderDispatch> = Vec::new();
        #[cfg(feature = "ort-cuda")]
        eps.push(ort::ep::cuda::CUDA::default().into());
        #[cfg(feature = "ort-directml")]
        eps.push(ort::ep::directml::DirectML::default().into());
        #[cfg(feature = "ort-coreml")]
        eps.push(ort::ep::coreml::CoreML::default().into());
        let builder = Self::base_builder(env, intra_threads)?
            .with_execution_providers(eps)
            .map_err(|e| e.to_string())?;
        let session = builder.commit_from_file(path).map_err(|e| e.to_string())?;
        Ok(Self::from_session(session))
    }

    fn base_builder(
        env: &Arc<Environment>,
        intra_threads: usize,
    ) -> Result<ort::session::builder::SessionBuilder, String> {
        let mut builder = ort::session::Session::builder(env).map_err(|e| e.to_string())?;
        builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?;
        if intra_threads > 0 {
            builder = builder
                .with_intra_threads(intra_threads)
                .map_err(|e| e.to_string())?;
        }
        Ok(builder)
    }

    fn from_session(session: ort::session::Session) -> Self {
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
        }
    }

    pub fn has_input(&self, name: &str) -> bool {
        self.input_names.iter().any(|n| n == name)
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
        let mut names: Vec<String> = Vec::with_capacity(inputs.len());
        let mut tensors: Vec<DynTensor> = Vec::with_capacity(inputs.len());
        for (name, (shape, data)) in inputs {
            tensors.push(Self::tensor_f32(shape, data)?);
            names.push(name);
        }
        let map: HashMap<&str, DynTensor> = names.iter().map(|n| n.as_str()).zip(tensors).collect();
        let outputs = self.session.run(map).map_err(|e| e.to_string())?;
        Self::collect_outputs(&self.output_names, outputs)
    }

    /// Прогон со смешанными типами: f32 + i64 входы.
    pub fn run_mixed(
        &mut self,
        f32_inputs: NamedTensors,
        i64_inputs: HashMap<String, ShapeDataI64>,
    ) -> Result<NamedTensors, String> {
        let empty_bool: HashMap<String, ShapeDataBool> = HashMap::new();
        self.run_mixed_full(f32_inputs, i64_inputs, empty_bool)
    }

    /// Прогон со смешанными типами: f32 + i64 + bool входы.
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
        let outputs = self.session.run(map).map_err(|e| e.to_string())?;
        Self::collect_outputs(&self.output_names, outputs)
    }

    fn tensor_bool(shape: Vec<i64>, data: Vec<bool>) -> Result<DynTensor, String> {
        Tensor::from_array((shape, data))
            .map(|t| t.upcast())
            .map_err(|e| e.to_string())
    }
}
