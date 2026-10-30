//! Loading and running ONNX models with tract (pure Rust, no native
//! runtime to ship).

use std::path::Path;
use std::sync::Arc;

use tract_onnx::prelude::*;

use crate::error::MlError;

/// One model output: its shape and its values in row-major order.
pub(crate) type FlatOutput = (Vec<usize>, Vec<f32>);

/// An optimised, runnable ONNX graph with fixed input shapes.
#[derive(Clone)]
pub(crate) struct OnnxModel {
    name: String,
    outputs: Vec<String>,
    plan: Arc<TypedRunnableModel>,
}

impl core::fmt::Debug for OnnxModel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OnnxModel")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// The element type and shape of one model input.
#[derive(Clone, Debug)]
pub(crate) struct InputShape {
    pub datum: DatumType,
    pub shape: Vec<usize>,
}

impl InputShape {
    pub fn f32(shape: &[usize]) -> Self {
        Self {
            datum: f32::datum_type(),
            shape: shape.to_vec(),
        }
    }

    #[cfg(feature = "clap")]
    pub fn i64(shape: &[usize]) -> Self {
        Self {
            datum: i64::datum_type(),
            shape: shape.to_vec(),
        }
    }
}

impl OnnxModel {
    /// Loads `path`, pins every input to `inputs` and optimises the graph.
    pub fn load(name: &str, path: &Path, inputs: &[InputShape]) -> Result<Self, MlError> {
        Self::from_parsed(name, parse(name, path)?, inputs)
    }

    /// Pins the inputs of an already parsed graph and optimises it.
    pub fn from_parsed(
        name: &str,
        mut model: InferenceModel,
        inputs: &[InputShape],
    ) -> Result<Self, MlError> {
        let fail = |error: TractError| MlError::inference(name, error);
        for (index, input) in inputs.iter().enumerate() {
            let fact = InferenceFact::dt_shape(input.datum, input.shape.as_slice());
            model = model.with_input_fact(index, fact).map_err(fail)?;
        }
        let outputs = model
            .output_outlets()
            .map_err(fail)?
            .iter()
            .map(|outlet| {
                model
                    .outlet_label(*outlet)
                    .map(str::to_owned)
                    .unwrap_or_else(|| model.node(outlet.node).name.clone())
            })
            .collect();
        let plan = model
            .into_optimized()
            .and_then(|model| model.into_runnable())
            .map_err(fail)?;
        Ok(Self {
            name: name.to_owned(),
            outputs,
            plan,
        })
    }

    /// Position of the output called `name`.
    pub fn output_index(&self, name: &str) -> Result<usize, MlError> {
        self.outputs
            .iter()
            .position(|output| output == name)
            .ok_or_else(|| MlError::inference(&self.name, format!("no output named `{name}`")))
    }

    /// Runs the graph and returns every output as a flat `f32` vector with
    /// its shape.
    pub fn run(&self, inputs: Vec<Tensor>) -> Result<Vec<FlatOutput>, MlError> {
        let fail = |error: TractError| MlError::inference(&self.name, error);
        let inputs: TVec<TValue> = inputs.into_iter().map(TValue::from).collect();
        let outputs = self.plan.run(inputs).map_err(fail)?;
        outputs
            .iter()
            .map(|output| {
                let view = output.to_plain_array_view::<f32>().map_err(fail)?;
                Ok((view.shape().to_vec(), view.iter().copied().collect()))
            })
            .collect()
    }
}

/// Parses an ONNX file without fixing its input shapes.
pub(crate) fn parse(name: &str, path: &Path) -> Result<InferenceModel, MlError> {
    tract_onnx::onnx()
        .with_ignore_value_info(true)
        .model_for_path(path)
        .map_err(|error| MlError::inference(name, error))
}

/// A tensor of `shape` from row-major `data`.
pub(crate) fn tensor_f32(shape: &[usize], data: &[f32]) -> Result<Tensor, MlError> {
    Tensor::from_shape(shape, data).map_err(|error| MlError::InvalidInput(format!("{error:#}")))
}

/// An `i64` tensor of `shape` from row-major `data`.
#[cfg(feature = "clap")]
pub(crate) fn tensor_i64(shape: &[usize], data: &[i64]) -> Result<Tensor, MlError> {
    Tensor::from_shape(shape, data).map_err(|error| MlError::InvalidInput(format!("{error:#}")))
}
