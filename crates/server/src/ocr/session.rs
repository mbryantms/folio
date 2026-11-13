//! ONNX Runtime sessions for the engines Folio runs itself.
//!
//! Every session is built here with an explicit intra-op thread count.
//! ONNX Runtime's default is one thread per physical core of the
//! machine, whatever number of cores the process may use; in a container
//! limited to a few cores that starts far more threads than can run and
//! the inference time multiplies. `available_parallelism` reads the
//! cgroup quota and the affinity mask, so the counts below follow the
//! cores the process really has.
//!
//! The sessions use every graph optimization (`ORT_ENABLE_ALL`), the
//! thread count, intra-op spinning off, and nothing else (per-session
//! thread pools, sequential execution, the CPU provider). Spinning is set
//! explicitly because ONNX Runtime builds disagree on its default. With
//! it on, the idle threads of one session's pool spin on the cores the
//! next session needs (a balloon runs the line finder, then the
//! recogniser; a Japanese read alternates encoder and decoder), and on
//! four cores a balloon took about twice as long and a Japanese read
//! about four times as long (`docs/dev/ocr.md`, "ONNX Runtime 1.31"). `ort` rc.15 maps
//! `GraphOptimizationLevel::Level3` to `ORT_ENABLE_LAYOUT`; `All` is
//! what `Level3` meant in rc.10.
//!
//! Every session belongs to the one ONNX Runtime environment of the
//! process ([`environment`]). `ort` allows a single environment per
//! process, so `crates/tools/ocr-bench` builds its sessions on this one
//! too.

use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

/// The process's ONNX Runtime environment, built on first use and kept
/// for the life of the process.
static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();

/// Serialises the first build: a second `ort::init().build()` in the
/// process fails, so two first callers must not race.
static ENVIRONMENT_INIT: Mutex<()> = Mutex::new(());

/// The ONNX Runtime environment every session is built on. Built once;
/// a failed build is not kept, so the next call tries again.
pub fn environment() -> anyhow::Result<&'static Environment> {
    if let Some(env) = ENVIRONMENT.get() {
        return Ok(env);
    }
    let _guard = ENVIRONMENT_INIT
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(env) = ENVIRONMENT.get() {
        return Ok(env);
    }
    if !cpu_runs_the_prebuilt_runtime() {
        anyhow::bail!(
            "this CPU has no AVX2, which the prebuilt ONNX Runtime library needs; \
             OCR is unavailable (running it would end the process with an illegal \
             instruction)"
        );
    }
    let env = ort::init()
        .with_name("folio")
        .with_telemetry(false)
        .build()
        .map_err(|e| anyhow::anyhow!("could not start ONNX Runtime: {e}"))?;
    Ok(ENVIRONMENT.get_or_init(|| env))
}

/// Whether this CPU can run the ONNX Runtime library linked in. The
/// prebuilt x86_64 library `ort` downloads (ONNX Runtime 1.31) is compiled
/// for x86-64-v3 and executes AVX2 instructions; on a CPU without AVX2 the
/// first inference would kill the whole server with SIGILL, which no
/// `catch_unwind` can stop. Refusing to build the environment turns that
/// into an OCR error instead.
fn cpu_runs_the_prebuilt_runtime() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        !ort::sys::USING_PYKE_BINARIES || std::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

/// The `ort` crate this build uses, exactly as pinned in the workspace
/// `Cargo.toml` (a unit test holds the two together). Part of the OCR
/// cache fingerprint with [`runtime_build_info`].
pub const ORT_CRATE_VERSION: &str = "2.0.0-rc.15";

/// ONNX Runtime's own build string for the library this binary links
/// (release branch, commit, build flags). Part of the OCR cache
/// fingerprint, so results computed by one runtime are not served after
/// an upgrade. Not asked of the library on a CPU that cannot run it
/// ([`cpu_runs_the_prebuilt_runtime`]); no OCR result exists there to key.
#[must_use]
pub fn runtime_build_info() -> &'static str {
    if cpu_runs_the_prebuilt_runtime() {
        ort::info()
    } else {
        "unavailable: no AVX2"
    }
}

/// The ONNX Runtime release this binary links, read from
/// [`runtime_build_info`] (`git-branch=rel-1.31.0` gives `1.31.0`), for
/// logs and reports. `None` when the build string has no release branch.
#[must_use]
pub fn runtime_version() -> Option<&'static str> {
    runtime_build_info()
        .split([',', ' '])
        .find_map(|w| w.strip_prefix("git-branch=rel-"))
}

/// Most intra-op threads the page detector, the line recogniser and the
/// Japanese encoder and decoder use. Each call is a small image (or a
/// short token sequence); more threads have nothing to do.
pub(crate) const MAX_INTRA_THREADS: usize = 4;

/// Intra-op threads of the line finder. It measured faster with 2 than
/// with 4 on a balloon crop.
pub(crate) const LINE_FINDER_MAX_INTRA_THREADS: usize = 2;

/// `min(cores this process may use, cap)`, at least 1.
#[must_use]
pub fn intra_threads(cap: usize) -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(cap)
        .max(1)
}

/// Build a session for the model at `path`. Blocking (reads and
/// optimizes the model): call from the blocking pool.
pub fn build(path: &Path, intra_threads: usize) -> anyhow::Result<Session> {
    let env = environment()?;
    Session::builder(env)
        .map_err(ort::Error::from)
        .and_then(|b| {
            b.with_optimization_level(GraphOptimizationLevel::All)
                .and_then(|b| b.with_intra_threads(intra_threads))
                .and_then(|b| b.with_intra_op_spinning(false))
                .map_err(ort::Error::from)
        })
        .and_then(|mut b| b.commit_from_file(path))
        .map_err(|e| anyhow::anyhow!("could not load the model {}: {e}", path.display()))
}

/// Names of a session's inputs, for the load-time shape checks.
pub(crate) fn input_names(session: &Session) -> Vec<&str> {
    session
        .inputs()
        .iter()
        .map(ort::value::Outlet::name)
        .collect()
}

/// Names of a session's outputs.
pub(crate) fn output_names(session: &Session) -> Vec<&str> {
    session
        .outputs()
        .iter()
        .map(ort::value::Outlet::name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_counts_are_capped_and_never_zero() {
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        assert_eq!(intra_threads(MAX_INTRA_THREADS), cores.min(4));
        assert_eq!(intra_threads(LINE_FINDER_MAX_INTRA_THREADS), cores.min(2));
        assert_eq!(intra_threads(1), 1);
        assert_eq!(intra_threads(0), 1, "a zero cap still gives one thread");
    }

    #[test]
    fn the_runtime_version_is_a_dotted_number() {
        let v = runtime_version().unwrap_or_else(|| panic!("{:?}", runtime_build_info()));
        assert!(
            v.split('.').count() == 3 && v.split('.').all(|p| p.parse::<u32>().is_ok()),
            "{v}"
        );
    }

    /// The constant names the `ort` the workspace pins, so the fingerprint
    /// moves when the pin does.
    #[test]
    fn the_crate_version_matches_the_workspace_pin() {
        let manifest = include_str!("../../../../Cargo.toml");
        let pin = format!("ort = \"={ORT_CRATE_VERSION}\"");
        assert!(manifest.lines().any(|l| l.trim() == pin), "no `{pin}` line");
    }

    #[test]
    fn the_environment_is_built_once() {
        let a = environment().expect("environment");
        let b = environment().expect("environment");
        assert!(std::ptr::eq(a, b));
    }
}
