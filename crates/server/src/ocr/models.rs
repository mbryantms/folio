//! Pinned, digest-verified OCR model files.
//!
//! `comic-text-detector` and `manga-ocr` load their ONNX models through
//! `hf_hub::api::sync::Api::new()`, which reads the Hugging Face cache
//! (`Cache::default()`, i.e. `$HOME/.cache/huggingface/hub`) and would
//! otherwise download the repository's `main` branch, unpinned and
//! unchecked. Before either constructor runs, [`ensure_pinned`] puts
//! every file the constructor reads into that cache itself:
//!
//! 1. the file is fetched from `/resolve/<commit>/<file>` (an immutable
//!    revision, never a branch), with timeouts and a byte cap equal to
//!    the file's published size;
//! 2. its SHA-256 is computed while it streams to a temporary file and
//!    compared with the pinned digest; a mismatch deletes the temporary
//!    file and fails with [`ModelVerifyError`], so nothing unverified is
//!    ever renamed into place;
//! 3. it lands at `snapshots/<commit>/<file>`, and `refs/main` is
//!    pointed at `<commit>`, which is exactly where the upstream
//!    constructor looks first, so it loads the verified file and never
//!    reaches the network.
//!
//! A file already in the cache (a cache populated before pinning, or one
//! changed on disk) is hashed too, once per process: the callers run this
//! inside their `OnceCell` initializer. A cached file whose digest does
//! not match is deleted and fetched again from the pinned revision.
//!
//! There is no setting for another model source: the upstream crates
//! hardcode the repositories and `Api::new()` ignores `HF_ENDPOINT`, so
//! the pin covers every way the models can be loaded. Operators on
//! air-gapped hosts pre-stage the files at `snapshots/<commit>/` (see
//! `docs/dev/ocr.md`); they are verified like any other cached file.
//!
//! # Updating the pin
//!
//! Looked up 2026-11-11 (UTC): the `main` branch of both repositories
//! pointed at the commits below (last modified 2025-05-08). To move it:
//!
//! 1. `curl -s https://huggingface.co/api/models/<repo>/revision/main?blobs=true`
//!    gives the commit (`sha`) and, per LFS file, `lfs.sha256` and `size`.
//! 2. For a non-LFS file (`vocab.txt`), download
//!    `https://huggingface.co/<repo>/resolve/<commit>/<file>` and run
//!    `sha256sum` on it (`git hash-object` should equal its `blobId`).
//! 3. Update `revision`, `sha256` and `size` below, and the table in
//!    `docs/dev/ocr.md`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// The only host the models come from.
pub const HF_ENDPOINT: &str = "https://huggingface.co";

/// One model file at an immutable revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedFile {
    /// `org/name` on the hub.
    pub repo: &'static str,
    /// Full commit hash. Never a branch or tag.
    pub revision: &'static str,
    pub file: &'static str,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: &'static str,
    /// Published size in bytes; a download is cut off past it.
    pub size: u64,
}

const DETECTOR_REPO: &str = "mayocream/comic-text-detector-onnx";
const DETECTOR_REVISION: &str = "a5d67ec772adef819ef5b0e7aa701fcf4c8bf74a";
const MANGA_REPO: &str = "mayocream/manga-ocr-onnx";
const MANGA_REVISION: &str = "24b12778d85800835e2ca409236de281b8ab7b9f";

/// What `ComicTextDetector::new` reads.
pub const DETECTOR_FILES: &[PinnedFile] = &[PinnedFile {
    repo: DETECTOR_REPO,
    revision: DETECTOR_REVISION,
    file: "comic-text-detector.onnx",
    sha256: "1a86ace74961413cbd650002e7bb4dcec4980ffa21b2f19b86933372071d718f",
    size: 94_669_756,
}];

/// What `MangaOCR::new` reads.
pub const MANGA_FILES: &[PinnedFile] = &[
    PinnedFile {
        repo: MANGA_REPO,
        revision: MANGA_REVISION,
        file: "encoder_model.onnx",
        sha256: "15fa8155fe9bc1a7d25d9bb353debaa4def033d0174e907dbd2dd6d995def85f",
        size: 343_454_249,
    },
    PinnedFile {
        repo: MANGA_REPO,
        revision: MANGA_REVISION,
        file: "decoder_model.onnx",
        sha256: "ef7765261e9d1cdc34d89356986c2bbc2a082897f753a89605ae80fdfa61f5e8",
        size: 117_480_262,
    },
    PinnedFile {
        repo: MANGA_REPO,
        revision: MANGA_REVISION,
        file: "vocab.txt",
        sha256: "5cb5c5586d98a2f331d9f8828e4586479b0611bfba5d8c3b6dadffc84d6a36a3",
        size: 30_216,
    },
];

/// Connect and per-read timeouts plus an overall ceiling sized for the
/// largest file (~330 MiB) on a slow link.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// A model file could not be fetched or did not match its pin. The OCR
/// request that triggered the load fails with this; the file is never
/// loaded.
#[derive(Debug)]
pub struct ModelVerifyError {
    pub repo: &'static str,
    pub file: &'static str,
    pub reason: String,
}

impl fmt::Display for ModelVerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OCR model {}/{} was not loaded: {}",
            self.repo, self.file, self.reason
        )
    }
}

impl std::error::Error for ModelVerifyError {}

impl PinnedFile {
    fn fail(&self, reason: impl Into<String>) -> ModelVerifyError {
        ModelVerifyError {
            repo: self.repo,
            file: self.file,
            reason: reason.into(),
        }
    }

    /// `{endpoint}/{repo}/resolve/{commit}/{file}`.
    pub fn url(&self, endpoint: &str) -> String {
        format!(
            "{}/{}/resolve/{}/{}",
            endpoint.trim_end_matches('/'),
            self.repo,
            self.revision,
            self.file
        )
    }

    /// `models--org--name`, the hub cache folder for the repository.
    fn repo_dir(&self, cache_root: &Path) -> PathBuf {
        cache_root.join(format!("models--{}", self.repo.replace('/', "--")))
    }

    /// Where `hf_hub`'s cache lookup finds the file once `refs/main`
    /// names [`Self::revision`].
    pub fn snapshot_path(&self, cache_root: &Path) -> PathBuf {
        self.repo_dir(cache_root)
            .join("snapshots")
            .join(self.revision)
            .join(self.file)
    }
}

/// The hub cache root the upstream crates read (`Cache::default()`).
pub fn hub_cache_root() -> anyhow::Result<PathBuf> {
    std::panic::catch_unwind(hf_hub::Cache::default)
        .map(|c| c.path().clone())
        .map_err(|_| anyhow::anyhow!("no home directory for the OCR model cache"))
}

fn http_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .build()?)
}

/// Make every file in `files` present and verified in the default hub
/// cache, from the default endpoint. Call before the upstream
/// constructor that reads them.
pub async fn ensure_pinned(files: &[PinnedFile]) -> anyhow::Result<()> {
    let root = hub_cache_root()?;
    let client = http_client()?;
    ensure_in(&client, HF_ENDPOINT, &root, files).await
}

/// [`ensure_pinned`] against an explicit endpoint and cache root (test
/// seam).
pub async fn ensure_in(
    client: &reqwest::Client,
    endpoint: &str,
    cache_root: &Path,
    files: &[PinnedFile],
) -> anyhow::Result<()> {
    for f in files {
        ensure_file(client, endpoint, cache_root, f).await?;
    }
    // Point the branch the upstream crates ask for at the pinned commit,
    // after every file of the repository is in place.
    let mut seen: Vec<&str> = Vec::new();
    for f in files {
        if seen.contains(&f.repo) {
            continue;
        }
        seen.push(f.repo);
        write_ref(&f.repo_dir(cache_root), f.revision)
            .await
            .map_err(|e| f.fail(format!("could not write the cache ref: {e}")))?;
    }
    Ok(())
}

async fn write_ref(repo_dir: &Path, revision: &str) -> std::io::Result<()> {
    let refs = repo_dir.join("refs");
    tokio::fs::create_dir_all(&refs).await?;
    let target = refs.join("main");
    if tokio::fs::read_to_string(&target).await.ok().as_deref() == Some(revision) {
        return Ok(());
    }
    let tmp = refs.join(format!(".main.{}.tmp", std::process::id()));
    tokio::fs::write(&tmp, revision).await?;
    tokio::fs::rename(&tmp, &target).await
}

/// One file: verify the cached copy, or fetch and verify a fresh one.
/// Returns the verified path.
pub async fn ensure_file(
    client: &reqwest::Client,
    endpoint: &str,
    cache_root: &Path,
    f: &PinnedFile,
) -> anyhow::Result<PathBuf> {
    let path = f.snapshot_path(cache_root);
    if tokio::fs::symlink_metadata(&path).await.is_ok() {
        match sha256_file(&path).await {
            Ok(digest) if digest == f.sha256 => return Ok(path),
            Ok(digest) => {
                tracing::warn!(
                    repo = f.repo,
                    file = f.file,
                    expected = f.sha256,
                    actual = %digest,
                    "ocr model: cached file does not match its pinned digest; removing it",
                );
            }
            Err(e) => {
                tracing::warn!(
                    repo = f.repo,
                    file = f.file,
                    error = %e,
                    "ocr model: cached file unreadable; removing it",
                );
            }
        }
        remove_cached(&path, &f.repo_dir(cache_root)).await;
    }
    download_verified(client, endpoint, f, &path).await?;
    Ok(path)
}

/// Delete a cached snapshot entry. `hf_hub` stores snapshot entries as
/// links into `blobs/`; the blob goes too when it lives in this
/// repository's cache folder.
async fn remove_cached(path: &Path, repo_dir: &Path) {
    if let Ok(meta) = tokio::fs::symlink_metadata(path).await
        && meta.file_type().is_symlink()
        && let Ok(target) = tokio::fs::canonicalize(path).await
        && let Ok(repo) = tokio::fs::canonicalize(repo_dir).await
        && target.starts_with(repo.join("blobs"))
    {
        let _ = tokio::fs::remove_file(&target).await;
    }
    let _ = tokio::fs::remove_file(path).await;
}

async fn download_verified(
    client: &reqwest::Client,
    endpoint: &str,
    f: &PinnedFile,
    dest: &Path,
) -> anyhow::Result<()> {
    let dir = dest
        .parent()
        .ok_or_else(|| f.fail("cache path has no parent"))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| f.fail(format!("could not create the cache folder: {e}")))?;
    let tmp = dir.join(format!(".{}.{}.part", f.file, std::process::id()));

    let result = stream_to(client, &f.url(endpoint), f, &tmp).await;
    match result {
        Ok(digest) if digest == f.sha256 => {
            tokio::fs::rename(&tmp, dest)
                .await
                .map_err(|e| f.fail(format!("could not move the verified file into place: {e}")))?;
            tracing::info!(
                repo = f.repo,
                file = f.file,
                revision = f.revision,
                "ocr model: downloaded and verified"
            );
            Ok(())
        }
        Ok(digest) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            let err = f.fail(format!(
                "sha256 {digest} does not match the pinned {}; the download was deleted",
                f.sha256
            ));
            tracing::error!(error = %err, "ocr model: digest mismatch");
            Err(err.into())
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            tracing::error!(error = %e, "ocr model: download failed");
            Err(e.into())
        }
    }
}

/// Stream the body to `tmp`, hashing as it goes; stop past `f.size`.
async fn stream_to(
    client: &reqwest::Client,
    url: &str,
    f: &PinnedFile,
    tmp: &Path,
) -> Result<String, ModelVerifyError> {
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| f.fail(format!("download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(f.fail(format!("download answered {}", resp.status())));
    }
    if resp.content_length().is_some_and(|n| n > f.size) {
        return Err(f.fail(format!(
            "the server announced {} bytes, more than the pinned {}",
            resp.content_length().unwrap_or_default(),
            f.size
        )));
    }
    let mut out = tokio::fs::File::create(tmp)
        .await
        .map_err(|e| f.fail(format!("could not create a temporary file: {e}")))?;
    let mut hasher = Sha256::new();
    let mut taken: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| f.fail(format!("download interrupted: {e}")))?
    {
        taken = taken.saturating_add(chunk.len() as u64);
        if taken > f.size {
            return Err(f.fail(format!("download exceeded the pinned {} bytes", f.size)));
        }
        hasher.update(&chunk);
        out.write_all(&chunk)
            .await
            .map_err(|e| f.fail(format!("could not write the download: {e}")))?;
    }
    out.flush()
        .await
        .map_err(|e| f.fail(format!("could not write the download: {e}")))?;
    out.sync_all()
        .await
        .map_err(|e| f.fail(format!("could not write the download: {e}")))?;
    Ok(hex(&hasher.finalize()))
}

/// SHA-256 of a file on disk, off the async runtime.
pub async fn sha256_file(path: &Path) -> std::io::Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = std::io::Read::read(&mut file, &mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex(&hasher.finalize()))
    })
    .await
    .map_err(std::io::Error::other)?
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BODY: &[u8] = b"pretend onnx bytes";

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn spec(sha256: &'static str, size: u64) -> PinnedFile {
        PinnedFile {
            repo: "acme/model-onnx",
            revision: "0123456789abcdef0123456789abcdef01234567",
            file: "model.onnx",
            sha256,
            size,
        }
    }

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    async fn serve(body: &'static [u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/acme/model-onnx/resolve/0123456789abcdef0123456789abcdef01234567/model.onnx",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;
        server
    }

    #[test]
    fn every_pin_is_a_commit_with_a_digest() {
        for f in DETECTOR_FILES.iter().chain(MANGA_FILES) {
            assert_eq!(f.revision.len(), 40, "{f:?}");
            assert!(f.revision.bytes().all(|b| b.is_ascii_hexdigit()), "{f:?}");
            assert_eq!(f.sha256.len(), 64, "{f:?}");
            assert!(
                f.sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{f:?}"
            );
            assert!(f.size > 0);
            let url = f.url(HF_ENDPOINT);
            assert_eq!(
                url,
                format!(
                    "https://huggingface.co/{}/resolve/{}/{}",
                    f.repo, f.revision, f.file
                )
            );
            assert!(!url.contains("/resolve/main/"));
        }
    }

    #[test]
    fn pins_cover_what_the_upstream_constructors_read() {
        let names: Vec<_> = DETECTOR_FILES
            .iter()
            .chain(MANGA_FILES)
            .map(|f| (f.repo, f.file))
            .collect();
        assert_eq!(
            names,
            [
                (
                    "mayocream/comic-text-detector-onnx",
                    "comic-text-detector.onnx"
                ),
                ("mayocream/manga-ocr-onnx", "encoder_model.onnx"),
                ("mayocream/manga-ocr-onnx", "decoder_model.onnx"),
                ("mayocream/manga-ocr-onnx", "vocab.txt"),
            ]
        );
    }

    #[tokio::test]
    async fn accepts_the_pinned_bytes_and_points_the_ref_at_the_commit() {
        let server = serve(BODY).await;
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(BODY)), BODY.len() as u64);
        ensure_in(&reqwest::Client::new(), &server.uri(), root.path(), &[f])
            .await
            .expect("verified");
        let path = f.snapshot_path(root.path());
        assert_eq!(std::fs::read(&path).unwrap(), BODY);
        let r = std::fs::read_to_string(root.path().join("models--acme--model-onnx/refs/main"))
            .unwrap();
        assert_eq!(r, f.revision);
        // `hf_hub`'s own cache lookup now resolves to the verified file.
        let cached = hf_hub::Cache::new(root.path().to_path_buf())
            .model(f.repo.to_owned())
            .get(f.file);
        assert_eq!(cached, Some(path));
    }

    #[tokio::test]
    async fn refuses_and_deletes_bytes_with_the_wrong_digest() {
        let server = serve(BODY).await;
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(b"something else")), BODY.len() as u64);
        let err = ensure_in(&reqwest::Client::new(), &server.uri(), root.path(), &[f])
            .await
            .unwrap_err();
        let err = err.downcast::<ModelVerifyError>().expect("typed error");
        assert!(err.reason.contains("does not match"), "{err}");
        let dir = f.snapshot_path(root.path()).parent().unwrap().to_path_buf();
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(left.is_empty(), "nothing is left behind: {left:?}");
        assert!(
            !root
                .path()
                .join("models--acme--model-onnx/refs/main")
                .exists(),
            "the ref is not moved to an unverified file"
        );
    }

    #[tokio::test]
    async fn refuses_a_body_larger_than_the_pin() {
        let server = serve(BODY).await;
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(BODY)), (BODY.len() - 1) as u64);
        let err = ensure_file(&reqwest::Client::new(), &server.uri(), root.path(), &f)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("bytes"), "{err}");
        assert!(!f.snapshot_path(root.path()).exists());
    }

    #[tokio::test]
    async fn a_cached_file_with_the_wrong_digest_is_not_trusted() {
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(BODY)), BODY.len() as u64);
        let path = f.snapshot_path(root.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"altered on disk").unwrap();

        // The pinned source is unreachable: the altered file is refused
        // and removed rather than loaded.
        let dead = "http://127.0.0.1:9";
        let err = ensure_file(&reqwest::Client::new(), dead, root.path(), &f)
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<ModelVerifyError>().is_some(), "{err}");
        assert!(!path.exists(), "the altered file is deleted");

        // With the source back, it is replaced by the verified bytes.
        std::fs::write(&path, b"altered on disk").unwrap();
        let server = serve(BODY).await;
        let got = ensure_file(&reqwest::Client::new(), &server.uri(), root.path(), &f)
            .await
            .unwrap();
        assert_eq!(std::fs::read(got).unwrap(), BODY);
    }

    #[tokio::test]
    async fn a_verified_cached_file_needs_no_network() {
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(BODY)), BODY.len() as u64);
        let path = f.snapshot_path(root.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, BODY).unwrap();
        let got = ensure_file(
            &reqwest::Client::new(),
            "http://127.0.0.1:9",
            root.path(),
            &f,
        )
        .await
        .unwrap();
        assert_eq!(got, path);
    }
}
