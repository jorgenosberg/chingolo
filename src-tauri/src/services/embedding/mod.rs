//! Embeddings for meaning search. Two backends, both 768 dimensions: Gemini's embedding API
//! (the default, needs the user's yes because passages leave the machine) and a local
//! EmbeddingGemma 300M 4-bit run through fastembed (ONNX Runtime, CPU).

pub mod cloud;
pub mod engine;
pub mod model;

use std::path::Path;

use fastembed::{
    AutoDevicePolicy, InitOptionsUserDefined, OutputKey, Pooling, QuantizationMode, TextEmbedding,
    TokenizerFiles, UserDefinedEmbeddingModel,
};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{AppError, Result};

pub const MODEL_NAME: &str = "embeddinggemma-300m-q4";
pub const DIMENSIONS: usize = 768;
/// Both models take these prefixes (EmbeddingGemma's, which `gemini-embedding-2` documents too).
pub const QUERY_PREFIX: &str = "task: search result | query: ";
pub const DOC_PREFIX: &str = "title: none | text: ";

/// Which model makes the vectors. Switching re-indexes, because vectors from different models
/// can't be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Backend {
    Gemini,
    Local,
}

impl Backend {
    /// The model name and revision stored with each vector.
    pub fn identity(self) -> (&'static str, &'static str) {
        match self {
            Backend::Gemini => (cloud::MODEL, cloud::REVISION),
            Backend::Local => (MODEL_NAME, model::REVISION),
        }
    }

    /// Passages per call. Local stays small so a search never waits long for the model.
    pub fn batch(self) -> u32 {
        match self {
            Backend::Gemini => 32,
            Backend::Local => 8,
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Backend::Gemini => "gemini",
            Backend::Local => "local",
        }
    }

    pub fn from_setting(s: &str) -> Option<Self> {
        match s {
            "gemini" => Some(Backend::Gemini),
            "local" => Some(Backend::Local),
            _ => None,
        }
    }
}

const ONNX_FILE: &str = "model_q4.onnx";
const ONNX_DATA_FILE: &str = "model_q4.onnx_data";

fn read(dir: &Path, name: &str) -> Result<Vec<u8>> {
    std::fs::read(dir.join(name))
        .map_err(|e| AppError::Internal(format!("Could not read model file {name}: {e}")))
}

/// Loads the model from a folder holding the six files listed in the download manifest.
/// The external weights file is handed to ONNX Runtime from memory under the name the
/// model refers to.
pub fn load_model(dir: &Path) -> Result<TextEmbedding> {
    let tokenizer_files = TokenizerFiles {
        tokenizer_file: read(dir, "tokenizer.json")?,
        config_file: read(dir, "config.json")?,
        special_tokens_map_file: read(dir, "special_tokens_map.json")?,
        tokenizer_config_file: read(dir, "tokenizer_config.json")?,
    };
    let mut model = UserDefinedEmbeddingModel::new(read(dir, ONNX_FILE)?, tokenizer_files)
        .with_pooling(Pooling::Mean)
        .with_quantization(QuantizationMode::None)
        .with_external_initializer(ONNX_DATA_FILE.to_string(), read(dir, ONNX_DATA_FILE)?);
    model.output_key = Some(OutputKey::ByName("sentence_embedding"));
    // CoreML was picked automatically on Apple Silicon and failed on one model in the benchmark.
    let options = InitOptionsUserDefined::default()
        .with_max_length(512)
        .with_auto_device(AutoDevicePolicy::PreferCPU);
    TextEmbedding::try_new_from_user_defined(model, options)
        .map_err(|e| AppError::Internal(format!("Could not start the search model: {e}")))
}

/// Dot product of two L2-normalized vectors.
#[cfg(test)]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastembed::{EmbeddingModel, TextInitOptions};

    fn model_dir() -> Option<std::path::PathBuf> {
        std::env::var_os("CHINGOLO_EMBED_MODEL_DIR").map(Into::into)
    }

    fn normalized(mut v: Vec<f32>) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= n);
        v
    }

    /// Needs `CHINGOLO_EMBED_MODEL_DIR` (the six model files in one folder) and network
    /// access for the built-in copy it is compared with.
    #[test]
    #[ignore]
    fn own_files_match_the_built_in_model() {
        let dir = model_dir().expect("set CHINGOLO_EMBED_MODEL_DIR");
        let mut ours = load_model(&dir).expect("load from our files");
        let cache = tempfile::tempdir().unwrap();
        let mut builtin = TextEmbedding::try_new(
            TextInitOptions::new(EmbeddingModel::EmbeddingGemma300MQ4)
                .with_cache_dir(cache.path().into())
                .with_auto_device(AutoDevicePolicy::PreferCPU),
        )
        .expect("built-in model");

        let texts = [
            format!("{DOC_PREFIX}Coral bleaching follows sustained heat stress."),
            format!("{DOC_PREFIX}Gill tissue was stained with alcian blue."),
            format!("{QUERY_PREFIX}why do reefs lose their colour"),
        ];
        let a = ours.embed(&texts, Some(8)).unwrap();
        let b = builtin.embed(&texts, Some(8)).unwrap();
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.len(), DIMENSIONS);
            let cos = dot(&normalized(x.clone()), &normalized(y.clone()));
            assert!(cos > 0.999, "cosine {cos}");
        }
        // The query is closer to the bleaching passage than to the staining one.
        let q = normalized(a[2].clone());
        assert!(dot(&q, &normalized(a[0].clone())) > dot(&q, &normalized(a[1].clone())));
    }
}
