//! Boot-time Postgres version guard (`server::pg_version`) against a real
//! server. The parse/compare helpers are unit-tested in the module; this
//! proves the `SHOW server_version_num` round-trip through sea-orm returns
//! the text form the parser expects and that the harness Postgres (18 in
//! CI and locally) passes the ≥ 17 gate `app::serve` enforces.

mod common;

use common::TestApp;
use server::pg_version::{MIN_SERVER_VERSION_NUM, assert_supported, format_version_num};

#[tokio::test]
async fn harness_postgres_passes_the_boot_guard() {
    let app = TestApp::spawn().await;
    let num = assert_supported(&app.state().db)
        .await
        .expect("harness Postgres must satisfy the minimum version");
    assert!(
        num >= MIN_SERVER_VERSION_NUM,
        "guard returned {} ({}) below the minimum it enforces",
        num,
        format_version_num(num)
    );
    // Sanity: the number decodes to a plausible major (17..99), i.e. the
    // `SHOW` text was the `major*10000+minor` form and not, say, "18.1".
    let major = num / 10_000;
    assert!((17..100).contains(&major), "implausible major {major}");
}
