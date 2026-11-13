//! Comic Reader server entry point.
//!
//! Layered structure (§14):
//!   `main` → bootstraps config, observability, DB pool, then hands off to `app::serve`.
//!   `app`  → assembles the Axum router with all middleware.
//!   `api`  → HTTP handlers (one module per resource).
//!   `auth` → OIDC + local + JWT + CSRF + WS ticket.

use server::{app, build_info, config, observability};

/// Refuse to start on an x86_64 CPU without AVX, with a message, before
/// the statically linked ONNX Runtime 1.31 runs its static initialisers
/// (they execute AVX instructions and would end the process with a bare
/// "Illegal instruction"; `docs/dev/ocr.md`, "CPU requirement"). A CPU
/// with AVX but without AVX2 is not refused here: the server runs there
/// and only text reading fails (`ocr::session::environment`).
///
/// The check is an entry in `.preinit_array`, which the dynamic loader
/// (or, for a static executable, the C runtime) runs before any
/// `.init_array` entry, and ONNX Runtime's initialisers are in
/// `.init_array`. It allocates nothing, cannot panic and uses no std I/O:
/// at that point nothing else in the process is set up.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[allow(unsafe_code)]
mod avx_check {
    const MESSAGE: &[u8] = b"folio: this CPU has no AVX. This build needs an x86_64 CPU with \
AVX2 (x86-64-v3) and cannot start; see docs/dev/ocr.md.\n";

    extern "C" fn check() {
        // `is_x86_feature_detected!("avx")` checks the CPUID bit and that
        // the OS saves the AVX registers (XGETBV); it caches in an atomic
        // and does not allocate.
        if std::is_x86_feature_detected!("avx") {
            return;
        }
        // SAFETY: `write` on fd 2 with a pointer and length taken from a
        // static byte string is valid for that whole length; a failed or
        // short write only loses the message. Its result is ignored on
        // purpose: there is nothing better to do before exiting.
        unsafe {
            libc::write(2, MESSAGE.as_ptr().cast(), MESSAGE.len());
        }
        // SAFETY: `_exit` ends the process without running atexit
        // handlers or destructors, none of which exist yet; it never
        // returns.
        unsafe { libc::_exit(1) }
    }

    // SAFETY: `.preinit_array` holds pointers to functions the loader
    // calls once, with no arguments the function needs to read (the
    // System V ABI passes argc/argv/envp in registers, which a function
    // taking none ignores). `#[used]` keeps the entry from being dropped.
    #[used]
    #[unsafe(link_section = ".preinit_array")]
    static PREINIT_AVX_CHECK: extern "C" fn() = check;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `dotenvy` loads `.env` from the current working directory. We deliberately
    // only do this in debug builds: in a containerized production deploy the
    // operator passes env via `--env-file` / compose `env_file:` / `environment:`,
    // and a stray `.env` sitting in the workdir (e.g. left behind in a bind-mount)
    // would silently override those — surprising and dangerous. Override with
    // `COMIC_LOAD_DOTENV=1` if you genuinely need it in a release binary.
    if cfg!(debug_assertions) || std::env::var("COMIC_LOAD_DOTENV").is_ok() {
        dotenvy::dotenv().ok();
    }

    // Allow `--emit-openapi` to print the spec and exit (used by `just openapi`).
    // Done before observability init so structured startup logs don't leak into
    // the JSON output on stdout.
    if std::env::args().any(|a| a == "--emit-openapi") {
        let spec = app::openapi_spec();
        // Intentional stdout: `just openapi` redirects this into the spec file.
        #[expect(clippy::print_stdout)]
        {
            println!("{}", serde_json::to_string_pretty(&spec)?);
        }
        return Ok(());
    }

    // Container healthcheck path. `docker compose` and `docker run --health-cmd`
    // invoke `/app/server --healthcheck`; we open a raw TCP connection to the
    // local bind port, issue `GET /readyz`, and exit 0 on `200 OK` else 1.
    // Raw TCP keeps this path zero-dep — it must not depend on the tokio
    // runtime spinning up or on observability init, both of which can hang
    // if the container is in a bad state, defeating the point of a healthcheck.
    if std::env::args().any(|a| a == "--healthcheck") {
        std::process::exit(healthcheck_probe());
    }

    let cfg = config::Config::load()?;

    // Observability before anything else so startup logs are structured.
    let handles = observability::init(&cfg)?;

    tracing::info!(
        version = build_info::VERSION,
        bind = %cfg.bind_addr,
        public_url = %cfg.public_url,
        auth_mode = %cfg.auth_mode,
        "Folio starting"
    );

    app::serve(cfg, handles).await
}

/// Synchronous in-container readiness probe. Returns the process exit code to
/// hand back to Docker / k8s exec-style probes: `0` healthy, `1` unhealthy.
///
/// Reads the port from `COMIC_BIND_ADDR` (matches the actual listener even if
/// the operator overrode the default), always connects to loopback (the probe
/// runs inside the container alongside the server), and uses short timeouts so
/// a stuck server doesn't block the healthcheck longer than the orchestrator's
/// own timeout window.
// Healthcheck failure messages go to stderr so `docker inspect --format '{{.State.Health.Log}}'`
// surfaces them to operators. Tracing isn't initialized this early so it's
// raw stderr or nothing.
#[expect(clippy::print_stderr)]
fn healthcheck_probe() -> i32 {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(3);

    let bind = std::env::var("COMIC_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let port = bind.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
    let Some(port) = port else {
        eprintln!("healthcheck: cannot parse port from COMIC_BIND_ADDR={bind:?}");
        return 1;
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    let mut stream = match TcpStream::connect_timeout(&addr, TIMEOUT) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("healthcheck: connect failed: {e}");
            return 1;
        }
    };
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));

    let req = b"GET /readyz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nUser-Agent: folio-healthcheck/1\r\n\r\n";
    if let Err(e) = stream.write_all(req) {
        eprintln!("healthcheck: write failed: {e}");
        return 1;
    }

    let mut head = [0u8; 16];
    let n = match stream.read(&mut head) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("healthcheck: read failed: {e}");
            return 1;
        }
    };
    // Status line starts `HTTP/1.1 200 …` — 16 bytes is enough for the code.
    if head
        .get(..n)
        .is_some_and(|s| s.starts_with(b"HTTP/1.1 200"))
    {
        0
    } else {
        let head_str = String::from_utf8_lossy(&head[..n]);
        eprintln!("healthcheck: unexpected status line: {head_str:?}");
        1
    }
}
