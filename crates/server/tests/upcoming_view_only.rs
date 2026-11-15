//! The view-only invariant of the Upcoming feature, held structurally
//! (upcoming-releases 1.0).
//!
//! Folio shows what Mylar is tracking and never controls it. These tests
//! fail the build if someone adds a way to reach Mylar:
//!
//! - the only route under `/integrations/mylar` is the snapshot POST;
//! - no runtime setting holds a Mylar address, port or key;
//! - the code that handles Mylar data makes no outbound request.

use server::settings::registry;

#[test]
fn the_only_mylar_route_is_the_snapshot_post() {
    let spec = serde_json::to_value(server::app::openapi_spec()).unwrap();
    let paths = spec["paths"].as_object().expect("paths");
    let mylar: Vec<(&String, Vec<&String>)> = paths
        .iter()
        .filter(|(path, _)| path.contains("/integrations/mylar"))
        .map(|(path, item)| (path, item.as_object().unwrap().keys().collect()))
        .collect();

    let ingest: Vec<_> = mylar
        .iter()
        .filter(|(path, _)| !path.starts_with("/api/admin/"))
        .collect();
    assert_eq!(ingest.len(), 1, "{ingest:?}");
    assert_eq!(ingest[0].0, "/integrations/mylar/snapshot");
    assert_eq!(ingest[0].1, [&"post".to_owned()], "POST only");

    // The admin side manages Folio's own token: create and revoke.
    let admin: Vec<_> = mylar
        .iter()
        .filter(|(path, _)| path.starts_with("/api/admin/"))
        .collect();
    assert_eq!(admin.len(), 1, "{admin:?}");
    assert_eq!(admin[0].0, "/api/admin/integrations/mylar/token");
    let mut methods: Vec<&str> = admin[0].1.iter().map(|m| m.as_str()).collect();
    methods.sort_unstable();
    assert_eq!(methods, ["delete", "post"]);
}

#[test]
fn no_setting_holds_a_mylar_address_or_key() {
    let mylar_keys: Vec<&str> = registry::registry()
        .iter()
        .map(|d| d.key)
        .filter(|k| k.contains("mylar"))
        .collect();
    assert_eq!(
        mylar_keys,
        [
            "integrations.mylar.enabled",
            "integrations.mylar.stale_after_minutes"
        ],
        "a new Mylar setting must not make Folio able to call Mylar"
    );
    for key in &mylar_keys {
        for forbidden in [
            "url", "host", "port", "address", "key", "token", "password", "secret",
        ] {
            assert!(
                !key.contains(forbidden),
                "{key}: Folio must not hold a Mylar {forbidden}"
            );
        }
    }
    assert!(
        registry::registry()
            .iter()
            .filter(|d| d.key.contains("mylar"))
            .all(|d| !d.is_secret),
        "no Mylar credential is stored"
    );
}

#[test]
fn the_mylar_code_makes_no_outbound_request() {
    // Source scan: the modules that receive, store and show Mylar data.
    let sources = [
        (
            "upcoming/mylar.rs",
            include_str!("../src/upcoming/mylar.rs"),
        ),
        (
            "upcoming/reconcile.rs",
            include_str!("../src/upcoming/reconcile.rs"),
        ),
        ("upcoming/view.rs", include_str!("../src/upcoming/view.rs")),
        (
            "api/mylar_ingest.rs",
            include_str!("../src/api/mylar_ingest.rs"),
        ),
        ("api/upcoming.rs", include_str!("../src/api/upcoming.rs")),
    ];
    for (name, text) in sources {
        for needle in [
            "reqwest",
            "hyper::Client",
            "build_client(",
            "fetch_public",
            "TcpStream",
        ] {
            assert!(
                !text.contains(needle),
                "{name} mentions `{needle}`: Mylar data arrives by push only"
            );
        }
    }
}
