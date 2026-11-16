//! Text recognition for pages that have no text layer, with PP-OCRv6 small through oar-ocr
//! (ONNX Runtime). The models are downloaded on first use like the search model. See
//! docs/STATUS.md (Slice 9 baseline and tool bake-off) for why this engine.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};


use crate::error::{AppError, Result};
use crate::services::embedding::model::{self, ManifestFile, ModelState};
use tokio_util::sync::CancellationToken;

/// Pinned files. `remote` is a full URL, which `model::download` accepts.
pub const FILES: &[ManifestFile] = &[
    ManifestFile {
        name: "pp-ocrv6_small_det.onnx",
        remote: concat!("https://github.com/GreatV/oar-ocr/releases/download/v0.7.0", "/pp-ocrv6_small_det.onnx"),
        size: 9_880_512,
        sha256: "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e",
    },
    ManifestFile {
        name: "pp-ocrv6_small_rec.onnx",
        remote: concat!("https://github.com/GreatV/oar-ocr/releases/download/v0.7.0", "/pp-ocrv6_small_rec.onnx"),
        size: 21_159_378,
        sha256: "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
    },
    ManifestFile {
        name: "ppocrv6_dict.txt",
        remote: concat!("https://github.com/GreatV/oar-ocr/releases/download/v0.7.0", "/ppocrv6_dict.txt"),
        size: 74_947,
        sha256: "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d",
    },
];

/// Where the files go. The folder name carries the release, so a new set never mixes with an old one.
pub fn model_dir(app_data: &Path) -> PathBuf {
    app_data.join("models").join("ocr-ppocrv6-small-v0.7.0")
}

pub fn is_ready(dir: &Path) -> bool {
    model::is_complete(dir, FILES)
}

/// The download of the models, as the UI sees it.
#[derive(Clone)]
pub struct OcrModel {
    dir: PathBuf,
    files: &'static [ManifestFile],
    state: Arc<Mutex<ModelState>>,
}

impl OcrModel {
    pub fn new(app_data: &Path) -> Self {
        Self::with_files(model_dir(app_data), FILES)
    }

    /// Any pinned set of files in one folder (the layout model uses this too).
    pub fn with_files(dir: PathBuf, files: &'static [ManifestFile]) -> Self {
        let state = if model::is_complete(&dir, files) { ModelState::Ready } else { ModelState::Missing };
        OcrModel { dir, files, state: Arc::new(Mutex::new(state)) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn status(&self) -> ModelState {
        self.state.lock().map(|s| s.clone()).unwrap_or(ModelState::Missing)
    }

    fn set(&self, s: ModelState) {
        if let Ok(mut g) = self.state.lock() {
            *g = s;
        }
    }

    /// Starts the download unless it is running or done. Returns at once.
    pub fn ensure(&self) {
        {
            let Ok(mut s) = self.state.lock() else { return };
            if matches!(*s, ModelState::Downloading { .. }) {
                return;
            }
            if model::is_complete(&self.dir, self.files) {
                *s = ModelState::Ready;
                return;
            }
            *s = ModelState::Downloading { done: 0.0, total: model::total_bytes(self.files) as f64 };
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let progress = me.clone();
            let result = model::download(&model::http_client(), "", me.files, &me.dir, &CancellationToken::new(), &mut |done, total| {
                progress.set(ModelState::Downloading { done: done as f64, total: total as f64 });
            })
            .await;
            me.set(match result {
                Ok(()) => ModelState::Ready,
                Err(e) => ModelState::Failed { message: e.to_string() },
            });
        });
    }
}

/// Pixels per PDF point when rendering a page for recognition (250 dpi), and the longest side
/// allowed, so a poster-sized page doesn't use gigabytes.
pub const RENDER_SCALE: f32 = 250.0 / 72.0;
pub const MAX_SIDE_PX: f32 = 3600.0;
/// Lines the model is less sure of than this are left out.
const MIN_CONFIDENCE: f32 = 0.5;

/// One recognized line. The box is in image pixels, top-left origin.
#[derive(Debug, Clone)]
pub struct OcrLine {
    pub text: String,
    pub rect: [f32; 4],
}

pub struct Ocr {
    engine: OAROCR,
}

fn fail(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("Text recognition failed: {e}"))
}

impl Ocr {
    pub fn load(dir: &Path) -> Result<Self> {
        let engine = OAROCRBuilder::new(
            dir.join("pp-ocrv6_small_det.onnx"),
            dir.join("pp-ocrv6_small_rec.onnx"),
            dir.join("ppocrv6_dict.txt"),
        )
        .build()
        .map_err(fail)?;
        Ok(Ocr { engine })
    }

    pub fn recognize(&self, img: image::RgbImage) -> Result<Vec<OcrLine>> {
        let mut out = self.engine.predict(vec![img]).map_err(fail)?;
        let Some(page) = out.pop() else { return Ok(vec![]) };
        Ok(reading_order(page
            .text_regions
            .iter()
            .filter(|r| r.confidence.unwrap_or(1.0) >= MIN_CONFIDENCE)
            .filter_map(|r| {
                let text = r.text.as_ref()?.trim().to_string();
                let b = &r.bounding_box;
                (!text.is_empty()).then(|| OcrLine { text, rect: [b.x_min(), b.y_min(), b.x_max(), b.y_max()] })
            })
            .collect()))
    }
}

/// Puts lines in reading order. The detector returns them top to bottom, which mixes the two
/// columns of a paper. Full-width lines (titles, abstracts) split the page into bands, and a band
/// with a clear gutter is read left column first, then right.
pub fn reading_order(mut lines: Vec<OcrLine>) -> Vec<OcrLine> {
    if lines.len() < 4 {
        lines.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));
        return lines;
    }
    let left = lines.iter().map(|l| l.rect[0]).fold(f32::MAX, f32::min);
    let right = lines.iter().map(|l| l.rect[2]).fold(f32::MIN, f32::max);
    let width = right - left;
    let wide = |l: &OcrLine| l.rect[2] - l.rect[0] > 0.6 * width;
    lines.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));

    let mut out = Vec::with_capacity(lines.len());
    let mut band: Vec<OcrLine> = Vec::new();
    let flush = |band: &mut Vec<OcrLine>, out: &mut Vec<OcrLine>| {
        out.extend(read_band(std::mem::take(band)));
    };
    for l in lines {
        if wide(&l) {
            flush(&mut band, &mut out);
            out.push(l);
        } else {
            band.push(l);
        }
    }
    flush(&mut band, &mut out);
    out
}

/// One band of narrow lines, already sorted top to bottom.
fn read_band(band: Vec<OcrLine>) -> Vec<OcrLine> {
    if band.len() < 4 {
        return band;
    }
    let lo = band.iter().map(|l| l.rect[0]).fold(f32::MAX, f32::min);
    let hi = band.iter().map(|l| l.rect[2]).fold(f32::MIN, f32::max);
    let mid = (lo + hi) / 2.0;
    let crosses = |l: &OcrLine| l.rect[0] < mid && l.rect[2] > mid;
    if band.iter().any(crosses) {
        return band;
    }
    let (left, right): (Vec<_>, Vec<_>) = band.into_iter().partition(|l| l.rect[2] <= mid);
    left.into_iter().chain(right).collect()
}

/// Loads the models the first time a page needs them, and only if the files are in place.
#[derive(Default)]
pub struct OcrSlot {
    dir: Option<PathBuf>,
    loaded: Option<Ocr>,
}

impl OcrSlot {
    #[cfg(test)]
    pub fn new(dir: Option<PathBuf>) -> Self {
        OcrSlot { dir, loaded: None }
    }

    pub fn set_dir(&mut self, dir: Option<PathBuf>) {
        if self.dir != dir {
            self.dir = dir;
            self.loaded = None;
        }
    }

    pub fn get(&mut self) -> Option<&Ocr> {
        if self.loaded.is_none() {
            let dir = self.dir.as_ref().filter(|d| is_ready(d))?;
            match Ocr::load(dir) {
                Ok(o) => self.loaded = Some(o),
                Err(e) => {
                    log::warn!("text recognition models failed to load: {e}");
                    self.dir = None;
                    return None;
                }
            }
        }
        self.loaded.as_ref()
    }

    /// True when models are configured but not downloaded yet.
    pub fn missing(&self) -> bool {
        self.dir.as_ref().is_some_and(|d| !is_ready(d))
    }
}

/// How pdfium turned the page: its own `/Rotate`, clockwise, in degrees.
pub type Rotation = u16;

/// Maps a pixel box of a rendered page (top-left origin, pdfium applies `/Rotate` and the crop box
/// when rendering) to PDF user space, where the stored line boxes live: `[left, bottom, right, top]`.
/// `crop` is the page's visible box in user space, `scale` the pixels per point actually used.
pub fn to_user_space(px: [f32; 4], scale: (f32, f32), crop: [f32; 4], rotation: Rotation) -> [f32; 4] {
    let (cw, ch) = (crop[2] - crop[0], crop[3] - crop[1]);
    // Size of the rotated, visible page in points.
    let vis_h = if rotation % 180 == 0 { ch } else { cw };
    let corner = |x: f32, y: f32| -> (f32, f32) {
        let (vx, vy) = (x / scale.0, vis_h - y / scale.1);
        let (ux, uy) = match rotation {
            90 => (cw - vy, vx),
            180 => (cw - vx, ch - vy),
            270 => (vy, ch - vx),
            _ => (vx, vy),
        };
        (ux + crop[0], uy + crop[1])
    };
    let (ax, ay) = corner(px[0], px[1]);
    let (bx, by) = corner(px[2], px[3]);
    [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
}

/// The inverse of `to_user_space`: a user space box `[left, bottom, right, top]` as a pixel box of
/// the rendered page `[x0, y0, x1, y1]` (top-left origin).
pub fn to_pixels(user: [f32; 4], scale: (f32, f32), crop: [f32; 4], rotation: Rotation) -> [f32; 4] {
    let (cw, ch) = (crop[2] - crop[0], crop[3] - crop[1]);
    let vis_h = if rotation % 180 == 0 { ch } else { cw };
    let corner = |x: f32, y: f32| -> (f32, f32) {
        let (ux, uy) = (x - crop[0], y - crop[1]);
        let (vx, vy) = match rotation {
            90 => (uy, cw - ux),
            180 => (cw - ux, ch - uy),
            270 => (ch - uy, ux),
            _ => (ux, uy),
        };
        (vx * scale.0, (vis_h - vy) * scale.1)
    };
    let (ax, ay) = corner(user[0], user[1]);
    let (bx, by) = corner(user[2], user[3]);
    [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_space_boxes_round_trip_through_pixels() {
        let crop = [10.0, 20.0, 310.0, 420.0];
        for rotation in [0, 90, 180, 270] {
            let user = [50.0, 100.0, 120.0, 160.0];
            let px = to_pixels(user, (3.0, 3.0), crop, rotation);
            assert!(close(to_user_space(px, (3.0, 3.0), crop, rotation), user), "rotation {rotation}");
        }
    }

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
    }

    fn line(text: &str, x0: f32, y0: f32, x1: f32) -> OcrLine {
        OcrLine { text: text.into(), rect: [x0, y0, x1, y0 + 10.0] }
    }

    #[test]
    fn two_columns_are_read_one_after_the_other_below_a_full_width_title() {
        let lines = vec![
            line("title", 0.0, 0.0, 190.0),
            line("r1", 110.0, 20.0, 190.0),
            line("l1", 0.0, 21.0, 90.0),
            line("l2", 0.0, 35.0, 90.0),
            line("r2", 110.0, 36.0, 190.0),
            line("l3", 0.0, 50.0, 90.0),
            line("r3", 110.0, 51.0, 190.0),
        ];
        let order: Vec<_> = reading_order(lines).into_iter().map(|l| l.text).collect();
        assert_eq!(order, ["title", "l1", "l2", "l3", "r1", "r2", "r3"]);
    }

    #[test]
    fn one_column_stays_top_to_bottom() {
        let lines = (0..6).map(|i| line(&format!("a{i}"), 0.0, i as f32 * 14.0, 150.0 - i as f32)).collect::<Vec<_>>();
        let order: Vec<_> = reading_order(lines).into_iter().map(|l| l.text).collect();
        assert_eq!(order, ["a0", "a1", "a2", "a3", "a4", "a5"]);
    }

    #[test]
    fn manifest_is_pinned() {
        assert_eq!(model::total_bytes(FILES), 9_880_512 + 21_159_378 + 74_947);
        assert!(FILES.iter().all(|f| f.remote.starts_with("https://github.com/GreatV/oar-ocr/releases/download/v0.7.0/") && f.sha256.len() == 64));
    }

    /// A 200 x 100 pt page with its lower left corner at (10, 20), drawn at 2 px per point.
    #[test]
    fn upright_page() {
        // A box 40 pt from the left, 10 pt from the top, 60 x 20 pt.
        let px = [80.0, 20.0, 200.0, 60.0];
        let r = to_user_space(px, (2.0, 2.0), [10.0, 20.0, 210.0, 120.0], 0);
        assert!(close(r, [50.0, 90.0, 110.0, 110.0]), "{r:?}");
    }

    /// Turning the page clockwise by 90 degrees: user-space bottom left ends up top left.
    #[test]
    fn rotated_90() {
        // Unrotated page is 200 wide, 100 tall; shown rotated it is 100 wide, 200 tall.
        // The user-space point (30, 10) (near the bottom left) shows 10 pt from the left, 30 from the top.
        let px = [10.0, 30.0, 20.0, 50.0];
        let r = to_user_space(px, (1.0, 1.0), [0.0, 0.0, 200.0, 100.0], 90);
        assert!(close(r, [30.0, 10.0, 50.0, 20.0]), "{r:?}");
    }

    #[test]
    fn rotated_180() {
        let px = [10.0, 10.0, 30.0, 20.0];
        let r = to_user_space(px, (1.0, 1.0), [0.0, 0.0, 200.0, 100.0], 180);
        assert!(close(r, [170.0, 10.0, 190.0, 20.0]), "{r:?}");
    }

    #[test]
    fn rotated_270() {
        let px = [10.0, 30.0, 20.0, 50.0];
        let r = to_user_space(px, (1.0, 1.0), [0.0, 0.0, 200.0, 100.0], 270);
        assert!(close(r, [150.0, 80.0, 170.0, 90.0]), "{r:?}");
    }
}
