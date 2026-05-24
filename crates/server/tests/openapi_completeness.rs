//! M1.5 spec-completeness gate.
//!
//! Two cheap guards that fire at `cargo test` time:
//!
//! 1. The OpenApi router builds without panicking. This catches the
//!    "Overlapping method route" class of bug — utoipa-axum's `routes!()`
//!    macro panics at construction time when two handlers with the same
//!    method are bundled at different paths. Without this test, the
//!    failure mode is `--emit-openapi` panicking inside `just openapi`.
//!
//! 2. The emitted spec contains a substantial number of paths and key
//!    surfaces. A regression that accidentally drops a `.routes()` call
//!    from `build_openapi_router` would shrink the spec; the floor check
//!    surfaces that loudly.

use server::app::{build_openapi_router, openapi_spec};

#[test]
fn build_openapi_router_does_not_panic() {
    let (_router, _spec) = build_openapi_router().split_for_parts();
}

#[test]
fn emitted_spec_contains_core_surfaces() {
    let spec = openapi_spec();
    let paths: std::collections::HashSet<&str> =
        spec.paths.paths.keys().map(String::as_str).collect();

    // Floor on size — a major M1 regression would drop this significantly.
    assert!(
        paths.len() >= 100,
        "openapi spec is suspiciously small: {} paths",
        paths.len()
    );

    // Spot-check the canonical surfaces. If any of these go missing the
    // most likely cause is a `.merge(...)` call being deleted from
    // `build_openapi_router` in `app.rs`.
    let canonical = [
        "/healthz",
        // Auth routes live in both `bare` and `api` groups, but M1b deduped
        // the spec so each operation appears once — under the `api` prefix
        // (the canonical JSON contract for the frontend).
        "/api/auth/me",
        "/api/auth/refresh",
        "/api/admin/users",
        "/api/admin/users/{id}",
        "/api/libraries",
        "/api/series",
        "/api/me/markers",
        "/api/me/reading-log",
        "/api/me/sessions",
        "/api/admin/settings",
        "/api/admin/audit",
    ];
    for p in canonical {
        assert!(
            paths.contains(p),
            "missing canonical path {p:?} from openapi spec (have {} paths)",
            paths.len()
        );
    }
}
