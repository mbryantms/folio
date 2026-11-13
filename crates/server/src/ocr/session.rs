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
//! The sessions use the options the engines were measured with:
//! graph-optimization level 3, the thread count, nothing else (per-session
//! thread pools, sequential execution, the CPU provider).

use std::path::Path;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

/// Most intra-op threads the page detector, the line recogniser and the
/// Japanese encoder and decoder use. Each call is a small image (or a
/// short token sequence); more threads have nothing to do.
pub(crate) const MAX_INTRA_THREADS: usize = 4;

/// Intra-op threads of the line finder. It measured faster with 2 than
/// with 4 on a balloon crop.
pub(crate) const LINE_FINDER_MAX_INTRA_THREADS: usize = 2;

/// `min(cores this process may use, cap)`, at least 1.
pub(crate) fn intra_threads(cap: usize) -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(cap)
        .max(1)
}

/// Build a session for the model at `path`. Blocking (reads and
/// optimizes the model): call from the blocking pool.
pub(crate) fn build(path: &Path, intra_threads: usize) -> anyhow::Result<Session> {
    Session::builder()
        .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
        .and_then(|b| b.with_intra_threads(intra_threads))
        .and_then(|b| b.commit_from_file(path))
        .map_err(|e| anyhow::anyhow!("could not load the model {}: {e}", path.display()))
}

/// Names of a session's inputs, for the load-time shape checks.
pub(crate) fn input_names(session: &Session) -> Vec<&str> {
    session.inputs.iter().map(|i| i.name.as_str()).collect()
}

/// Names of a session's outputs.
pub(crate) fn output_names(session: &Session) -> Vec<&str> {
    session.outputs.iter().map(|o| o.name.as_str()).collect()
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
}
