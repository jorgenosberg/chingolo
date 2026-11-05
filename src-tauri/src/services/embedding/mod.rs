//! Local embeddings: EmbeddingGemma 300M 4-bit run through fastembed (ONNX Runtime, CPU).

use std::path::Path;

use fastembed::{
    AutoDevicePolicy, InitOptionsUserDefined, OutputKey, Pooling, QuantizationMode, TextEmbedding,
    TokenizerFiles, UserDefinedEmbeddingModel,
};

use crate::error::{AppError, Result};

pub const MODEL_NAME: &str = "embeddinggemma-300m-q4";
pub const DIMENSIONS: usize = 768;
/// EmbeddingGemma expects these prefixes (see the benchmark notes in docs/STATUS.md).
pub const QUERY_PREFIX: &str = "task: search result | query: ";
pub const DOC_PREFIX: &str = "title: none | text: ";

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
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastembed::{EmbeddingModel, InitOptions};

    fn model_dir() -> Option<std::path::PathBuf> {
        std::env::var_os("CHINGUI_EMBED_MODEL_DIR").map(Into::into)
    }

    fn normalized(mut v: Vec<f32>) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= n);
        v
    }

    /// Needs `CHINGUI_EMBED_MODEL_DIR` (the six model files in one folder) and network
    /// access for the built-in copy it is compared with.
    #[test]
    #[ignore]
    fn own_files_match_the_built_in_model() {
        let dir = model_dir().expect("set CHINGUI_EMBED_MODEL_DIR");
        let mut ours = load_model(&dir).expect("load from our files");
        let cache = tempfile::tempdir().unwrap();
        let mut builtin = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::EmbeddingGemma300MQ4)
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
