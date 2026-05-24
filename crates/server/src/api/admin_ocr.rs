//! `GET /admin/ocr/models` — read-only OCR model surface
//! (text-detection-1.0 plan, M5).
//!
//! Reports which of the three OCR-pipeline models the operator has
//! on disk plus rough bytes-on-disk per model. Lives under the
//! `/admin/server` family so it picks up [`RequireAdmin`].
//!
//! Disk discovery:
//!
//! - `comic-text-detector` ONNX → `${HF_HOME}/hub/models--mayocream--comic-text-detector-onnx/`
//! - `manga-ocr` ONNX (encoder + decoder + vocab) → `${HF_HOME}/hub/models--mayocream--manga-ocr-onnx/`
//! - English Tesseract LSTM (`eng.traineddata`) → `${TESSDATA_PREFIX}/eng.traineddata`
//!   with fallback to `${HOME}/.tesseract-rs/tessdata/eng.traineddata`
//!   (the [`crate::ocr::recognizer::western::tessdata_dir`] resolver
//!   matches this exactly so the report mirrors what the recognizer
//!   actually reads on cold start).
//!
//! **Upstream gotcha (hf-hub 0.4.3):** `comic-text-detector` and
//! `manga-ocr` both call `hf_hub::api::sync::Api::new()`, which
//! resolves the cache via `Cache::default()` and **does not honor
//! `HF_HOME`**. The real cache root is
//! `dirs::home_dir() + .cache/huggingface/hub/`, i.e. driven by
//! `HOME`. Our `hf_home_dir()` below reads `HF_HOME` first and falls
//! back to `${HOME}/.cache/huggingface` for that reason — and the
//! Folio Dockerfile sets `HOME=/data` so both resolutions land on
//! the same persistent path. See `docs/dev/ocr.md`.
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
    /// Typical post-download size — informational, used by the UI to
    /// render a download-progress bar before the model is fully cached.
    pub expected_bytes_approx: u64,
    /// Where the binary fetches this model from on first init.
    /// `"huggingface.co/<repo>"` or `"tessdata_best (build script)"`.
    pub source: &'static str,
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
    let detector_dir = hf_repo_dir(&hf_home, "mayocream/comic-text-detector-onnx");
    let manga_dir = hf_repo_dir(&hf_home, "mayocream/manga-ocr-onnx");
    let tessdata_eng = tessdata_dir.join("eng.traineddata");

    let detector_bytes = dir_size(&detector_dir).unwrap_or(0);
    let manga_bytes = dir_size(&manga_dir).unwrap_or(0);
    let tessdata_bytes = file_size(&tessdata_eng).unwrap_or(0);

    let models = vec![
        OcrModelView {
            id: "comic-text-detector",
            purpose: "Text-bubble detection over cropped reader regions",
            kind: "onnx",
            cache_dir: detector_dir.to_string_lossy().into_owned(),
            present: detector_bytes > 0,
            bytes_on_disk: detector_bytes,
            // The on-disk blob is ~95 MB after HF dedups symlinks; we
            // round up for display.
            expected_bytes_approx: 95 * 1024 * 1024,
            source: "huggingface.co/mayocream/comic-text-detector-onnx",
        },
        OcrModelView {
            id: "manga-ocr",
            purpose: "Vertical Japanese OCR (encoder + decoder + vocab)",
            kind: "onnx",
            cache_dir: manga_dir.to_string_lossy().into_owned(),
            present: manga_bytes > 0,
            bytes_on_disk: manga_bytes,
            expected_bytes_approx: 250 * 1024 * 1024,
            source: "huggingface.co/mayocream/manga-ocr-onnx",
        },
        OcrModelView {
            id: "tesseract-eng",
            purpose: "Western OCR — Tesseract LSTM (English `tessdata_best`)",
            kind: "tessdata",
            cache_dir: tessdata_dir.to_string_lossy().into_owned(),
            present: tessdata_bytes > 0,
            bytes_on_disk: tessdata_bytes,
            // `tessdata_best/eng.traineddata` on the v4.1 LSTM branch.
            expected_bytes_approx: 15 * 1024 * 1024,
            source: "tessdata_best (build script)",
        },
    ];

    let total_bytes_on_disk = models.iter().map(|m| m.bytes_on_disk).sum();

    Json(OcrModelsView {
        hf_home: hf_home.to_string_lossy().into_owned(),
        tessdata_dir: tessdata_dir.to_string_lossy().into_owned(),
        models,
        total_bytes_on_disk,
    })
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
        if entry.file_type().is_file()
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
    fn file_size_returns_zero_for_missing() {
        let p = PathBuf::from("/definitely/not/a/file/folio-test");
        assert_eq!(file_size(&p).unwrap(), 0);
    }
}
