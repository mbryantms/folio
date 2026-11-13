//! `GET /admin/ocr/models` — read-only OCR model surface
//! (text-detection-1.0 plan, M5).
//!
//! Reports which of the OCR-pipeline models the operator has on disk
//! plus bytes-on-disk per model. Lives under the `/admin/server` family
//! so it picks up [`RequireAdmin`].
//!
//! The list, in order:
//!
//! - the current engines: the balloon detector, the line finder and the
//!   line recogniser ([`crate::ocr::models`] pins, fetched on first use);
//! - `manga-ocr` (encoder + decoder + vocab), used with either engine set;
//! - the legacy engines, loaded only with `COMIC_OCR_LEGACY_ENGINES=true`:
//!   `comic-text-detector` and the English Tesseract data
//!   (`${TESSDATA_PREFIX}/eng.traineddata`, falling back to
//!   `${HOME}/.tesseract-rs/tessdata/eng.traineddata`, the resolution
//!   [`crate::ocr::recognizer::western`] uses). Without the switch the
//!   detector is never fetched and stays "missing" on a new install.
//!
//! ONNX models sit in the hub cache,
//! `<root>/hub/models--<org>--<repo>/`. The loader's root is
//! `dirs::home_dir() + .cache/huggingface` (`hf_hub::Cache::default()`,
//! driven by `HOME`, **not** `HF_HOME`); `hf_home_dir()` below reads
//! `HF_HOME` first and falls back to `${HOME}/.cache/huggingface`, and
//! the Folio Dockerfile sets `HOME=/data` and `HF_HOME` so both
//! resolutions land on the same persistent path. See `docs/dev/ocr.md`.
//!
//! Missing directories return `present = false` + `bytes_on_disk = 0`
//! rather than an error — the operator dashboard uses this to drive
//! a "model not yet downloaded" hint.

use std::path::{Path, PathBuf};

use axum::{Json, response::IntoResponse};
use serde::Serialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::RequireAdmin;
use crate::ocr::models::{self, PinnedFile};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OcrModelView {
    /// Stable identifier; never localized.
    pub id: &'static str,
    /// One-line operator-facing description.
    pub purpose: &'static str,
    /// `"onnx"` (HF-cached models) or `"tessdata"` (Tesseract LSTM).
    pub kind: &'static str,
    /// Absolute path the loader actually reads from. May not exist
    /// yet — pair with `present`.
    pub cache_dir: String,
    /// `true` if at least one model file is on disk under
    /// `cache_dir`. For HF entries this is "the snapshot folder
    /// holds files"; for tessdata, "`eng.traineddata` exists".
    pub present: bool,
    /// Recursive sum of bytes under `cache_dir`. `0` when
    /// `present = false`.
    pub bytes_on_disk: u64,
    /// Size once fully downloaded (the sum of the pinned files for the
    /// ONNX models) — used by the UI to tell "downloading" from "ready".
    pub expected_bytes_approx: u64,
    /// Where the binary fetches this model from on first init.
    /// `"huggingface.co/<repo>"` or `"tessdata_best (build script)"`.
    pub source: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OcrModelsView {
    /// Resolved `HF_HOME` — useful for operators wanting to
    /// pre-stage models on disk in air-gapped deploys.
    pub hf_home: String,
    /// Resolved tessdata directory; see module doc for resolution
    /// order.
    pub tessdata_dir: String,
    /// Per-model state.
    pub models: Vec<OcrModelView>,
    /// Sum of `bytes_on_disk` across every entry.
    pub total_bytes_on_disk: u64,
}

#[utoipa::path(
    operation_id = "admin_ocr_list",    get,
    path = "/admin/ocr/models",
    responses(
        (status = 200, body = OcrModelsView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn list(_admin: RequireAdmin) -> impl IntoResponse {
    let hf_home = hf_home_dir();
    let tessdata_dir = tessdata_dir();

    // HF cache layout is `${HF_HOME}/hub/models--{org}--{repo}/…`.
    let tessdata_eng = tessdata_dir.join("eng.traineddata");
    let tessdata_bytes = file_size(&tessdata_eng).unwrap_or(0);

    let mut models: Vec<OcrModelView> = ONNX_MODELS
        .iter()
        .map(|m| onnx_model_view(&hf_home, m))
        .collect();
    models.push(OcrModelView {
        id: "tesseract-eng",
        purpose: "Legacy Western OCR: Tesseract LSTM, English `tessdata_best` \
                  (loaded only with COMIC_OCR_LEGACY_ENGINES=true)",
        kind: "tessdata",
        cache_dir: tessdata_dir.to_string_lossy().into_owned(),
        present: tessdata_bytes > 0,
        bytes_on_disk: tessdata_bytes,
        // `tessdata_best/eng.traineddata` on the v4.1 LSTM branch.
        expected_bytes_approx: 15 * 1024 * 1024,
        source: "tessdata_best (build script)".to_owned(),
    });

    let total_bytes_on_disk = models.iter().map(|m| m.bytes_on_disk).sum();

    Json(OcrModelsView {
        hf_home: hf_home.to_string_lossy().into_owned(),
        tessdata_dir: tessdata_dir.to_string_lossy().into_owned(),
        models,
        total_bytes_on_disk,
    })
}

// ───────── model list ─────────

/// One ONNX model of the card: the pinned files of one hub repository.
struct OnnxModel {
    id: &'static str,
    purpose: &'static str,
    /// All of one hub repository.
    files: &'static [PinnedFile],
}

/// The ONNX models, in display order: the current engines, manga-ocr,
/// then the legacy detector. Sizes and cache folders come from the pins,
/// so the card cannot drift from what the loaders fetch.
const ONNX_MODELS: &[OnnxModel] = &[
    OnnxModel {
        id: "comic-text-and-bubble-detector",
        purpose: "Balloon and free-text detection over a whole page",
        files: models::BUBBLE_DETECTOR_FILES,
    },
    OnnxModel {
        id: "pp-ocrv5-mobile-det",
        purpose: "Line finder: splits a balloon into text lines",
        files: models::LINE_FINDER_FILES,
    },
    OnnxModel {
        id: models::WESTERN_REC.id,
        purpose: "Western OCR: line recogniser (model + character list)",
        files: models::WESTERN_REC.files,
    },
    OnnxModel {
        id: "manga-ocr",
        purpose: "Vertical Japanese OCR (encoder + decoder + vocab)",
        files: models::MANGA_FILES,
    },
    OnnxModel {
        id: "comic-text-detector",
        purpose: "Legacy page detector (loaded only with COMIC_OCR_LEGACY_ENGINES=true)",
        files: models::DETECTOR_FILES,
    },
];

fn onnx_model_view(hf_home: &Path, model: &OnnxModel) -> OcrModelView {
    let repo = model.files.first().map_or("", |f| f.repo);
    let dir = hf_repo_dir(hf_home, repo);
    let bytes = dir_size(&dir).unwrap_or(0);
    OcrModelView {
        id: model.id,
        purpose: model.purpose,
        kind: "onnx",
        cache_dir: dir.to_string_lossy().into_owned(),
        present: bytes > 0,
        bytes_on_disk: bytes,
        expected_bytes_approx: model.files.iter().map(|f| f.size).sum(),
        source: format!("huggingface.co/{repo}"),
    }
}

// ───────── path resolution ─────────

/// Resolve `HF_HOME`. The `hf-hub` crate honors this env var first,
/// then falls back to `${HOME}/.cache/huggingface`. We mirror that
/// exactly so this surface reports what the loader sees. If neither
/// is set (HOME unset on a stripped-down container, say) we report
/// `/.cache/huggingface` — wrong but at least obviously placeholder.
fn hf_home_dir() -> PathBuf {
    if let Ok(p) = std::env::var("HF_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cache").join("huggingface")
}

/// HF cache directory for `{org}/{repo}` — uses HF's underscore
/// quoting convention so this stays consistent with the layout
/// `hf-hub` writes to.
fn hf_repo_dir(hf_home: &Path, repo: &str) -> PathBuf {
    let mangled = format!("models--{}", repo.replace('/', "--"));
    hf_home.join("hub").join(mangled)
}

/// Mirror of [`crate::ocr::recognizer::western::tessdata_dir`] — see
/// that resolver for the canonical rationale.
fn tessdata_dir() -> PathBuf {
    if let Ok(p) = std::env::var("TESSDATA_PREFIX") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".tesseract-rs").join("tessdata")
}

// ───────── disk walking ─────────

/// Recursive byte sum. Symlinks are followed (HF stores blobs as
/// links from `snapshots/` into `blobs/` — to report "real" bytes
/// we want them counted once, but `WalkDir::follow_links(false)`
/// already does that since blobs live under the same root).
///
/// Missing directories return `Ok(0)` so the endpoint doesn't have
/// to surface a "directory not found" error path — pre-download
/// state is the common case.
fn dir_size(path: &Path) -> std::io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total: u64 = 0;
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        // An unfinished model download is not a model on disk.
        if entry.file_type().is_file()
            && !crate::ocr::models::is_partial_download(&entry.file_name().to_string_lossy())
            && let Ok(meta) = entry.metadata()
        {
            total = total.saturating_add(meta.len());
        }
    }
    Ok(total)
}

fn file_size(path: &Path) -> std::io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    Ok(path.metadata()?.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hf_repo_dir_uses_org_repo_mangling() {
        let p = hf_repo_dir(Path::new("/cache/hf"), "mayocream/comic-text-detector-onnx");
        assert_eq!(
            p,
            PathBuf::from("/cache/hf/hub/models--mayocream--comic-text-detector-onnx")
        );
    }

    #[test]
    fn dir_size_returns_zero_for_missing_path() {
        let p = PathBuf::from("/definitely/not/a/path/folio-test");
        assert_eq!(dir_size(&p).unwrap(), 0);
    }

    #[test]
    fn dir_size_sums_files_recursively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), vec![0u8; 100]).unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested/b"), vec![0u8; 250]).unwrap();
        assert_eq!(dir_size(dir.path()).unwrap(), 350);
    }

    #[test]
    fn dir_size_leaves_out_unfinished_downloads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".folio-part-x1Yz"), vec![0u8; 500]).unwrap();
        assert_eq!(dir_size(dir.path()).unwrap(), 0, "a partial is not a model");
        std::fs::write(dir.path().join("model.onnx"), vec![0u8; 40]).unwrap();
        assert_eq!(dir_size(dir.path()).unwrap(), 40);
    }

    /// The card's rows follow the pins: each names its repository, the
    /// expected size is what the loader fetches, and the line recogniser
    /// row is the one the server runs.
    #[test]
    fn onnx_rows_follow_the_pinned_files() {
        let ids: Vec<&str> = ONNX_MODELS.iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            [
                "comic-text-and-bubble-detector",
                "pp-ocrv5-mobile-det",
                models::WESTERN_REC.id,
                "manga-ocr",
                "comic-text-detector",
            ]
        );
        for m in ONNX_MODELS {
            let repo = m.files[0].repo;
            assert!(m.files.iter().all(|f| f.repo == repo), "{}", m.id);
            let view = onnx_model_view(Path::new("/cache/hf"), m);
            assert_eq!(view.source, format!("huggingface.co/{repo}"), "{}", m.id);
            assert_eq!(
                view.cache_dir,
                format!("/cache/hf/hub/models--{}", repo.replace('/', "--"))
            );
            assert_eq!(
                view.expected_bytes_approx,
                m.files.iter().map(|f| f.size).sum::<u64>()
            );
            assert!(!view.present && view.bytes_on_disk == 0);
        }
        assert_eq!(
            onnx_model_view(Path::new("/x"), &ONNX_MODELS[0]).expected_bytes_approx,
            11_120_765
        );
    }

    #[test]
    fn file_size_returns_zero_for_missing() {
        let p = PathBuf::from("/definitely/not/a/file/folio-test");
        assert_eq!(file_size(&p).unwrap(), 0);
    }
}
