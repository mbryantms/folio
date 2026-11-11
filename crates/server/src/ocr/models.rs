//! Pinned, digest-verified OCR model files.
//!
//! `comic-text-detector` and `manga-ocr` load their ONNX models through
//! `hf_hub::api::sync::Api::new()`, which reads the Hugging Face cache
//! (`Cache::default()`, i.e. `$HOME/.cache/huggingface/hub`) and would
//! otherwise download the repository's `main` branch, unpinned and
//! unchecked. Before either constructor runs, [`ensure_pinned`] puts
//! every file the constructor reads into that cache itself:
//!
//! 1. the file is fetched over https from `/resolve/<commit>/<file>` (an
//!    immutable revision, never a branch), with timeouts, bounded
//!    https-only redirects that refuse internal address literals, and a
//!    byte cap equal to the file's published size;
//! 2. its SHA-256 is computed while it streams to an exclusive temporary
//!    file and compared with the pinned digest; a mismatch drops the
//!    temporary file and fails with [`ModelVerifyError`], so nothing
//!    unverified is ever moved into place;
//! 3. it lands at `snapshots/<commit>/<file>`, and `refs/main` is
//!    pointed at `<commit>`, which is exactly where the upstream
//!    constructor looks first, so it loads the verified file and never
//!    reaches the network.
//!
//! The fetch runs in a detached task, one per repository for the whole
//! process ([`Pinner`]): a request that times out or disconnects stops
//! waiting, but the download carries on, and later requests wait on the
//! same task or find the verified files. After a failed attempt the
//! repository is not tried again for [`RETRY_AFTER`]; requests in that
//! window get the same error without touching the disk or the network.
//!
//! A file already in the cache (a cache populated before pinning, or one
//! changed on disk) is hashed too, once per process. A cached file whose
//! digest does not match is deleted and fetched again from the pinned
//! revision.
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

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

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
/// Redirect hops followed (the hub answers LFS files with one redirect
/// to its CDN).
const MAX_REDIRECTS: usize = 5;
/// How long a repository is left alone after a failed attempt.
pub const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);
/// Temporary download names start with this; nothing else in the cache
/// does.
const PART_PREFIX: &str = ".folio-part-";
/// A temporary file older than this belongs to a download that cannot
/// still be running (it outlived [`TOTAL_TIMEOUT`]) and is swept.
const STALE_PART_AGE: Duration = Duration::from_secs(35 * 60);

/// A model file could not be fetched or did not match its pin. The OCR
/// request that triggered the load fails with this; the file is never
/// loaded.
#[derive(Debug, Clone)]
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

/// `true` for a temporary download file name, so disk reports can leave
/// unfinished downloads out.
pub fn is_partial_download(name: &str) -> bool {
    name.starts_with(PART_PREFIX)
}

/// The hub cache root the upstream crates read (`Cache::default()`).
pub fn hub_cache_root() -> Result<PathBuf, String> {
    std::panic::catch_unwind(hf_hub::Cache::default)
        .map(|c| c.path().clone())
        .map_err(|_| "no home directory for the OCR model cache".to_owned())
}

/// The download client. `require_https` is off only in this module's
/// tests, whose local servers speak plain http.
fn http_client(require_https: bool) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .https_only(require_https)
        .redirect(crate::util::ssrf::outbound_redirect_policy(
            MAX_REDIRECTS,
            require_https,
        ))
        .build()
        .map_err(|e| format!("could not build the download client: {e}"))
}

type Outcome = Result<(), ModelVerifyError>;

#[derive(Default)]
struct RepoState {
    verified: bool,
    failed: Option<(Instant, ModelVerifyError)>,
    inflight: Option<watch::Receiver<Option<Outcome>>>,
}

/// Single-flight fetch-and-verify, one detached task per repository.
pub struct Pinner {
    client: reqwest::Client,
    endpoint: String,
    cache_root: PathBuf,
    retry_after: Duration,
    repos: Mutex<HashMap<&'static str, RepoState>>,
}

impl Pinner {
    fn new(
        endpoint: &str,
        cache_root: PathBuf,
        retry_after: Duration,
        require_https: bool,
    ) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            client: http_client(require_https)?,
            endpoint: endpoint.to_owned(),
            cache_root,
            retry_after,
            repos: Mutex::new(HashMap::new()),
        }))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, HashMap<&'static str, RepoState>> {
        // A poisoned map only means a panic elsewhere mid-update; the
        // flags it holds are still meaningful.
        self.repos.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Make `files` (all of one repository) present and verified. Waits
    /// on the repository's download task; dropping this future stops the
    /// wait, never the download.
    pub async fn ensure(self: &Arc<Self>, files: &'static [PinnedFile]) -> Outcome {
        let Some(first) = files.first() else {
            return Ok(());
        };
        let repo = first.repo;
        let mut rx = {
            let mut map = self.state();
            let st = map.entry(repo).or_default();
            if st.verified {
                return Ok(());
            }
            if let Some((at, err)) = &st.failed {
                let since = at.elapsed();
                if since < self.retry_after {
                    return Err(ModelVerifyError {
                        reason: format!(
                            "{}; not retried until {} s after that attempt",
                            err.reason,
                            self.retry_after.as_secs()
                        ),
                        ..err.clone()
                    });
                }
            }
            match &st.inflight {
                Some(rx) => rx.clone(),
                None => {
                    let (tx, rx) = watch::channel(None);
                    st.inflight = Some(rx.clone());
                    let me = Arc::clone(self);
                    tokio::spawn(async move {
                        let res = me.fetch_and_verify(files).await;
                        {
                            let mut map = me.state();
                            let st = map.entry(repo).or_default();
                            st.inflight = None;
                            match &res {
                                Ok(()) => {
                                    st.verified = true;
                                    st.failed = None;
                                }
                                Err(e) => st.failed = Some((Instant::now(), e.clone())),
                            }
                        }
                        let _ = tx.send(Some(res));
                    });
                    rx
                }
            }
        };
        // Copy the answer out so the borrow of `rx` ends before the match.
        let waited = rx.wait_for(Option::is_some).await.map(|done| done.clone());
        match waited {
            Ok(done) => done.unwrap_or(Ok(())),
            Err(_) => {
                // The task ended without an answer (it panicked): let
                // the next request start a fresh one. Another waiter may
                // already have done that, so only forget the channel
                // this call was waiting on.
                {
                    let mut map = self.state();
                    let st = map.entry(repo).or_default();
                    if st
                        .inflight
                        .as_ref()
                        .is_some_and(|stored| stored.same_channel(&rx))
                    {
                        st.inflight = None;
                    }
                }
                Err(first.fail("the download task ended without a result"))
            }
        }
    }

    async fn fetch_and_verify(&self, files: &[PinnedFile]) -> Outcome {
        for f in files {
            ensure_file(&self.client, &self.endpoint, &self.cache_root, f).await?;
        }
        // Point the branch the upstream crates ask for at the pinned
        // commit, after every file of the repository is in place.
        let mut seen: Vec<&str> = Vec::new();
        for f in files {
            if seen.contains(&f.repo) {
                continue;
            }
            seen.push(f.repo);
            write_ref(&f.repo_dir(&self.cache_root), f.revision)
                .await
                .map_err(|e| f.fail(format!("could not write the cache ref: {e}")))?;
        }
        Ok(())
    }
}

/// Make every file in `files` present and verified in the default hub
/// cache, from the hub over https. Call before the upstream constructor
/// that reads them.
pub async fn ensure_pinned(files: &'static [PinnedFile]) -> anyhow::Result<()> {
    static DEFAULT: OnceLock<Result<Arc<Pinner>, String>> = OnceLock::new();
    let pinner = DEFAULT.get_or_init(|| {
        let root = hub_cache_root()?;
        Pinner::new(HF_ENDPOINT, root, RETRY_AFTER, true)
    });
    let pinner = match pinner {
        Ok(p) => p,
        Err(e) => {
            let f = files.first().copied().unwrap_or(DETECTOR_FILES[0]);
            return Err(f.fail(e.clone()).into());
        }
    };
    pinner.ensure(files).await.map_err(Into::into)
}

async fn write_ref(repo_dir: &Path, revision: &str) -> std::io::Result<()> {
    let refs = repo_dir.join("refs");
    tokio::fs::create_dir_all(&refs).await?;
    let target = refs.join("main");
    if tokio::fs::read_to_string(&target).await.ok().as_deref() == Some(revision) {
        return Ok(());
    }
    let revision = revision.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut tmp = tempfile::Builder::new()
            .prefix(PART_PREFIX)
            .tempfile_in(&refs)?;
        std::io::Write::write_all(&mut tmp, revision.as_bytes())?;
        tmp.persist(&target).map_err(|e| e.error)?;
        Ok(())
    })
    .await
    .map_err(std::io::Error::other)?
}

/// One file: verify the cached copy, or fetch and verify a fresh one.
/// Returns the verified path.
pub async fn ensure_file(
    client: &reqwest::Client,
    endpoint: &str,
    cache_root: &Path,
    f: &PinnedFile,
) -> Result<PathBuf, ModelVerifyError> {
    let path = f.snapshot_path(cache_root);
    if let Some(dir) = path.parent() {
        sweep_stale_parts(dir).await;
    }
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

/// Remove temporary files a killed process left behind: ours, and the
/// `.<file>.<pid>.part` names of the first pinning build. Only files old
/// enough that no download can still be writing them.
async fn sweep_stale_parts(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ours = is_partial_download(&name) || (name.starts_with('.') && name.ends_with(".part"));
        if !ours {
            continue;
        }
        let old = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > STALE_PART_AGE);
        if old {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
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
) -> Result<(), ModelVerifyError> {
    let dir = dest
        .parent()
        .ok_or_else(|| f.fail("cache path has no parent"))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| f.fail(format!("could not create the cache folder: {e}")))?;
    // Exclusive, randomly named, removed when dropped: a failed or
    // cancelled download leaves nothing, and two processes sharing the
    // volume never write into the same file.
    let tmp = tempfile::Builder::new()
        .prefix(PART_PREFIX)
        .tempfile_in(dir)
        .map_err(|e| f.fail(format!("could not create a temporary file: {e}")))?;
    let out = tmp
        .as_file()
        .try_clone()
        .map_err(|e| f.fail(format!("could not open the temporary file: {e}")))?;

    let digest = stream_to(client, &f.url(endpoint), f, tokio::fs::File::from_std(out)).await?;
    if digest != f.sha256 {
        // `tmp` drops here and takes the bytes with it. The caller logs.
        return Err(f.fail(format!(
            "sha256 {digest} does not match the pinned {}; the download was deleted",
            f.sha256
        )));
    }
    tmp.persist(dest).map_err(|e| {
        f.fail(format!(
            "could not move the verified file into place: {}",
            e.error
        ))
    })?;
    tracing::info!(
        repo = f.repo,
        file = f.file,
        revision = f.revision,
        "ocr model: downloaded and verified"
    );
    Ok(())
}

/// Stream the body into `out`, hashing as it goes; stop past `f.size`.
async fn stream_to(
    client: &reqwest::Client,
    url: &str,
    f: &PinnedFile,
    mut out: tokio::fs::File,
) -> Result<String, ModelVerifyError> {
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| f.fail(format!("download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(f.fail(format!("download answered {}", resp.status())));
    }
    if let Some(n) = resp.content_length()
        && n > f.size
    {
        return Err(f.fail(format!(
            "the server announced {n} bytes, more than the pinned {}",
            f.size
        )));
    }
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
    use tokio::io::AsyncReadExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BODY: &[u8] = b"pretend onnx bytes";
    const REV: &str = "0123456789abcdef0123456789abcdef01234567";
    const ROUTE: &str =
        "/acme/model-onnx/resolve/0123456789abcdef0123456789abcdef01234567/model.onnx";

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    fn spec(sha256: &'static str, size: u64) -> PinnedFile {
        PinnedFile {
            repo: "acme/model-onnx",
            revision: REV,
            file: "model.onnx",
            sha256,
            size,
        }
    }

    fn files(f: PinnedFile) -> &'static [PinnedFile] {
        Box::leak(vec![f].into_boxed_slice())
    }

    fn client() -> reqwest::Client {
        http_client(false).unwrap()
    }

    fn pinner(endpoint: &str, root: &Path, retry_after: Duration) -> Arc<Pinner> {
        Pinner::new(endpoint, root.to_path_buf(), retry_after, false).unwrap()
    }

    async fn serve(template: ResponseTemplate, expect: u64) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(template)
            .expect(expect)
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
    async fn the_production_client_refuses_plain_http() {
        let server = serve(ResponseTemplate::new(200).set_body_bytes(BODY), 0).await;
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(BODY)), BODY.len() as u64);
        let https_only = http_client(true).unwrap();
        let err = ensure_file(&https_only, &server.uri(), root.path(), &f)
            .await
            .unwrap_err();
        assert!(err.reason.contains("download failed"), "{err}");
    }

    #[tokio::test]
    async fn accepts_the_pinned_bytes_and_points_the_ref_at_the_commit() {
        let server = serve(ResponseTemplate::new(200).set_body_bytes(BODY), 1).await;
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(BODY)), BODY.len() as u64);
        pinner(&server.uri(), root.path(), RETRY_AFTER)
            .ensure(files(f))
            .await
            .expect("verified");
        let path = f.snapshot_path(root.path());
        assert_eq!(std::fs::read(&path).unwrap(), BODY);
        let refs = root.path().join("models--acme--model-onnx/refs");
        assert_eq!(
            std::fs::read_to_string(refs.join("main")).unwrap(),
            f.revision
        );
        assert_eq!(std::fs::read_dir(&refs).unwrap().count(), 1, "no temp left");
        // `hf_hub`'s own cache lookup now resolves to the verified file.
        let cached = hf_hub::Cache::new(root.path().to_path_buf())
            .model(f.repo.to_owned())
            .get(f.file);
        assert_eq!(cached, Some(path));
    }

    #[tokio::test]
    async fn refuses_and_deletes_bytes_with_the_wrong_digest() {
        let server = serve(ResponseTemplate::new(200).set_body_bytes(BODY), 1).await;
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(b"something else")), BODY.len() as u64);
        let err = pinner(&server.uri(), root.path(), RETRY_AFTER)
            .ensure(files(f))
            .await
            .unwrap_err();
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
    async fn refuses_an_announced_length_larger_than_the_pin() {
        let server = serve(ResponseTemplate::new(200).set_body_bytes(BODY), 1).await;
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(BODY)), (BODY.len() - 1) as u64);
        let err = ensure_file(&client(), &server.uri(), root.path(), &f)
            .await
            .unwrap_err();
        assert!(err.reason.contains("announced"), "{err}");
        assert!(!f.snapshot_path(root.path()).exists());
    }

    /// A chunked body has no Content-Length, so only the streaming cap
    /// can stop it.
    #[tokio::test]
    async fn cuts_off_a_chunked_body_past_the_pin() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut seen = Vec::new();
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    return;
                }
                seen.extend_from_slice(&buf[..n]);
            }
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await;
            // 4 KiB chunks, well past a 64-byte pin.
            let chunk = vec![b'x'; 4096];
            for _ in 0..16 {
                let head = format!("{:x}\r\n", chunk.len());
                if sock.write_all(head.as_bytes()).await.is_err()
                    || sock.write_all(&chunk).await.is_err()
                    || sock.write_all(b"\r\n").await.is_err()
                {
                    return;
                }
            }
            let _ = sock.write_all(b"0\r\n\r\n").await;
        });
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(b"irrelevant")), 64);
        let err = ensure_file(&client(), &format!("http://{addr}"), root.path(), &f)
            .await
            .unwrap_err();
        assert!(err.reason.contains("exceeded"), "{err}");
        let dir = f.snapshot_path(root.path()).parent().unwrap().to_path_buf();
        assert_eq!(
            std::fs::read_dir(dir).unwrap().count(),
            0,
            "no partial file left"
        );
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
        let err = ensure_file(&client(), "http://127.0.0.1:9", root.path(), &f)
            .await
            .unwrap_err();
        assert!(err.reason.contains("download failed"), "{err}");
        assert!(!path.exists(), "the altered file is deleted");

        // With the source back, it is replaced by the verified bytes.
        std::fs::write(&path, b"altered on disk").unwrap();
        let server = serve(ResponseTemplate::new(200).set_body_bytes(BODY), 1).await;
        let got = ensure_file(&client(), &server.uri(), root.path(), &f)
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
        let got = ensure_file(&client(), "http://127.0.0.1:9", root.path(), &f)
            .await
            .unwrap();
        assert_eq!(got, path);
    }

    #[tokio::test]
    async fn stale_part_files_are_swept_and_fresh_ones_kept() {
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(BODY)), BODY.len() as u64);
        let path = f.snapshot_path(root.path());
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&path, BODY).unwrap();
        let old = dir.join(format!("{PART_PREFIX}old"));
        let legacy = dir.join(".model.onnx.42.part");
        let fresh = dir.join(format!("{PART_PREFIX}fresh"));
        for p in [&old, &legacy, &fresh] {
            std::fs::write(p, b"partial").unwrap();
        }
        let long_ago = SystemTime::now() - Duration::from_secs(3600);
        for p in [&old, &legacy] {
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(long_ago)
                .unwrap();
        }
        ensure_file(&client(), "http://127.0.0.1:9", root.path(), &f)
            .await
            .unwrap();
        assert!(!old.exists() && !legacy.exists(), "stale parts swept");
        assert!(fresh.exists(), "a part that may still be written is kept");
        assert!(is_partial_download(
            &fresh.file_name().unwrap().to_string_lossy()
        ));
    }

    /// A request that stops waiting (timeout, disconnect) does not stop
    /// the download: it completes in the background and the next request
    /// finds the verified file without another download.
    #[tokio::test]
    async fn a_dropped_wait_does_not_abort_the_download() {
        let server = serve(
            ResponseTemplate::new(200)
                .set_body_bytes(BODY)
                .set_delay(Duration::from_millis(400)),
            1,
        )
        .await;
        let root = tempfile::tempdir().unwrap();
        let f = spec(leak(sha(BODY)), BODY.len() as u64);
        let fs = files(f);
        let p = pinner(&server.uri(), root.path(), RETRY_AFTER);

        let first = tokio::time::timeout(Duration::from_millis(50), p.ensure(fs)).await;
        assert!(first.is_err(), "the first wait is dropped mid-download");

        let path = f.snapshot_path(root.path());
        for _ in 0..100 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            BODY,
            "the download completed"
        );
        // Verified once for the process: no second request (the mock
        // expects exactly one).
        p.ensure(fs).await.expect("later call succeeds");
        p.ensure(fs).await.expect("and again");
        server.verify().await;
    }

    /// Concurrent waiters share one download.
    #[tokio::test]
    async fn concurrent_requests_share_one_download() {
        let server = serve(
            ResponseTemplate::new(200)
                .set_body_bytes(BODY)
                .set_delay(Duration::from_millis(200)),
            1,
        )
        .await;
        let root = tempfile::tempdir().unwrap();
        let fs = files(spec(leak(sha(BODY)), BODY.len() as u64));
        let p = pinner(&server.uri(), root.path(), RETRY_AFTER);
        let (a, b, c) = tokio::join!(p.ensure(fs), p.ensure(fs), p.ensure(fs));
        assert!(a.is_ok() && b.is_ok() && c.is_ok());
        server.verify().await;
    }

    /// After a failure the repository is left alone for the retry
    /// interval: requests in that window get the error without a
    /// download, and the next one after it tries again.
    #[tokio::test]
    async fn a_failed_attempt_is_not_retried_at_once() {
        let server = serve(ResponseTemplate::new(503), 2).await;
        let root = tempfile::tempdir().unwrap();
        let fs = files(spec(leak(sha(BODY)), BODY.len() as u64));
        let p = pinner(&server.uri(), root.path(), Duration::from_millis(300));

        let e1 = p.ensure(fs).await.unwrap_err();
        assert!(e1.reason.contains("503"), "{e1}");
        let e2 = p.ensure(fs).await.unwrap_err();
        assert!(e2.reason.contains("not retried"), "{e2}");
        assert_eq!(e2.repo, "acme/model-onnx");

        tokio::time::sleep(Duration::from_millis(350)).await;
        let e3 = p.ensure(fs).await.unwrap_err();
        assert!(!e3.reason.contains("not retried"), "{e3}");
        server.verify().await;
    }
}
