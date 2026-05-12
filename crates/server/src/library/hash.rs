//! Content hashing for issue dedupe (§5.1.2).
//!
//! BLAKE3 of the full file bytes, hex-encoded. Streamed in chunks so we
//! never hold the whole archive in memory. Buffer size is tunable via
//! `COMIC_SCAN_HASH_BUFFER_KB` — see `docs/dev/scanner-perf.md` F-9 for
//! why this matters (cold scans on real libraries are dominated by
//! kernel readahead/page-cache fill; larger buffers + sequential
//! `posix_fadvise` reduce the syscall and page-allocation overhead).

use blake3::Hasher;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Default chunk size when no override is provided (e.g. unit tests).
/// Matches `default_scan_hash_buffer_kb` in `crate::config`.
const DEFAULT_CHUNK_KB: usize = 1024;

pub fn blake3_file(path: impl AsRef<Path>) -> std::io::Result<String> {
    blake3_file_with_buffer(path, DEFAULT_CHUNK_KB)
}

/// Same as [`blake3_file`], with an explicit chunk size in KB. Caller
/// passes `state.cfg.scan_hash_buffer_kb` from the scanner hot path so the
/// buffer is tunable without touching code.
pub fn blake3_file_with_buffer(
    path: impl AsRef<Path>,
    buffer_kb: usize,
) -> std::io::Result<String> {
    let chunk = buffer_kb.max(64) * 1024; // floor at 64 KB to avoid pathological tiny reads
    let f = File::open(path.as_ref())?;
    advise_sequential(&f);
    let mut reader = BufReader::with_capacity(chunk, f);
    let mut hasher = Hasher::new();
    let mut buf = vec![0u8; chunk];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Hint the kernel to prefetch larger windows for sequential reads. Cuts
/// CPU time spent in `page_cache_ra_unbounded` / `folio_alloc_noprof` /
/// `kernel_init_pages` per the F-9 flamegraph. Best-effort — failures
/// (e.g. on a tmpfs that doesn't support fadvise) are silently ignored
/// since the hash itself doesn't depend on it.
#[cfg(unix)]
#[allow(unsafe_code)]
fn advise_sequential(f: &File) {
    use std::os::unix::io::AsRawFd;
    // SAFETY: `f` is a live, owned File; raw fd is valid for the duration
    // of this call. POSIX_FADV_SEQUENTIAL has no preconditions beyond a
    // valid fd; the syscall returns an errno on failure but never
    // corrupts state.
    unsafe {
        libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
    }
}

#[cfg(not(unix))]
fn advise_sequential(_f: &File) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn deterministic_hash() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"hello world").unwrap();
        let a = blake3_file(f.path()).unwrap();
        let b = blake3_file(f.path()).unwrap();
        assert_eq!(a, b);
        // BLAKE3 of "hello world" is well-known.
        assert_eq!(
            a,
            "d74981efa70a0c880b8d8c1985d075dbcbf679b99a5f9914e5aaf96b831a9e24"
        );
    }

    #[test]
    fn different_content_different_hash() {
        let f1 = tempfile::NamedTempFile::new().unwrap();
        let f2 = tempfile::NamedTempFile::new().unwrap();
        f1.as_file().write_all(b"alpha").unwrap();
        f2.as_file().write_all(b"beta").unwrap();
        assert_ne!(
            blake3_file(f1.path()).unwrap(),
            blake3_file(f2.path()).unwrap()
        );
    }
}
