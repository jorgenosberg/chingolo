//! Page layout with PP-DocLayoutV3 through oar-ocr (ONNX Runtime): which parts of a page are body
//! text, headers, footers, tables, figures, and in what order they read. Regions only say where
//! things are. The characters in them still come from the PDF's own text layer (or OCR on scans).
//! See docs/STATUS.md ("Next batch") for the rules.

use std::path::{Path, PathBuf};

use oar_ocr::core::traits::adapter::{AdapterBuilder, ModelAdapter};
use oar_ocr::core::traits::task::ImageTaskInput;
use oar_ocr::domain::adapters::{LayoutDetectionAdapter, LayoutDetectionAdapterBuilder, LayoutModelConfig};
use oar_ocr::domain::tasks::LayoutDetectionConfig;

use super::ocr::OcrModel;
use crate::error::{AppError, Result};
use crate::services::embedding::model::ManifestFile;

pub const FILES: &[ManifestFile] = &[ManifestFile {
    name: "pp-doclayoutv3.onnx",
    remote: "https://github.com/GreatV/oar-ocr/releases/download/v0.6.0/pp-doclayoutv3.onnx",
    size: 129_955_811,
    sha256: "1a7ec3812d239ad14debb87e38273012558fea27ac10b44b61260e3a88358e39",
}];

pub fn model_dir(app_data: &Path) -> PathBuf {
    app_data.join("models").join("layout-ppdoclayoutv3-v0.6.0")
}

/// The download of the layout model; same behaviour as the OCR models.
pub fn model(app_data: &Path) -> OcrModel {
    OcrModel::with_files(model_dir(app_data), FILES)
}

/// One region. The box is in image pixels, top-left origin.
#[derive(Debug, Clone)]
pub struct Region {
    /// The model's own label, e.g. `text`, `table`, `header`, `footnote`.
    pub label: String,
    pub rect: [f32; 4],
    pub score: f32,
    /// Position in the model's reading order, 0-based.
    pub order: u32,
}

impl Region {
    /// Running heads, page numbers and the like: not part of what the document says.
    pub fn is_furniture(&self) -> bool {
        matches!(self.label.as_str(), "header" | "header_image" | "footer" | "footer_image" | "number")
    }
}

pub struct Layout {
    adapter: LayoutDetectionAdapter,
}

fn fail(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("Page layout failed: {e}"))
}

impl Layout {
    pub fn load(dir: &Path) -> Result<Self> {
        let adapter = LayoutDetectionAdapterBuilder::new()
            .model_config(LayoutModelConfig::pp_doclayoutv3())
            .with_config(LayoutDetectionConfig::with_pp_doclayoutv3_defaults())
            .build(&dir.join("pp-doclayoutv3.onnx"))
            .map_err(fail)?;
        Ok(Layout { adapter })
    }

    /// Regions in reading order, from the layout model alone (no text recognition, no table models,
    /// which fail a whole page when the wired table model is missing).
    pub fn detect(&self, img: image::RgbImage) -> Result<Vec<Region>> {
        let out = self.adapter.execute(ImageTaskInput::new(vec![img]), None).map_err(fail)?;
        let elements = out.elements.into_iter().next().unwrap_or_default();
        Ok(elements
            .into_iter()
            .enumerate()
            .map(|(i, e)| Region {
                label: e.element_type.to_lowercase(),
                rect: [e.bbox.x_min(), e.bbox.y_min(), e.bbox.x_max(), e.bbox.y_max()],
                score: e.score,
                order: i as u32,
            })
            .collect())
    }
}
