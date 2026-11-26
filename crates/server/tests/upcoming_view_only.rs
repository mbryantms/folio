//! The view-only invariant of the Upcoming feature, held structurally
//! (upcoming-releases 1.0), and the bounds of the one exception
//! (add-to-mylar 1.0).
//!
//! The snapshot side shows what Mylar is tracking and never contacts it.
//! Add-to-Mylar is the single, deliberate way Folio reaches Mylar: an
//! admin adds a reading list's missing series. These tests fail the build
//! if either side grows:
//!
//! - the routes under `/integrations/mylar` are exactly the ones listed;
//! - the only Mylar settings beyond the snapshot's are the address and
//!   the key add-to-Mylar needs;
//! - the code that handles snapshot data makes no outbound request;
//! - the Mylar client can send only its five commands, none destructive.

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

    // The admin side manages Folio's own token (create, revoke), reads
    // the stored snapshot back, and queues add-to-Mylar requests (plus a
    // connection test). Nothing else, and no other verb.
    let mut admin: Vec<(&str, Vec<&str>)> = mylar
        .iter()
        .filter(|(path, _)| path.starts_with("/api/admin/"))
        .map(|(path, methods)| {
            let mut methods: Vec<&str> = methods.iter().map(|m| m.as_str()).collect();
            methods.sort_unstable();
            (path.as_str(), methods)
        })
        .collect();
    admin.sort();
    assert_eq!(
        admin,
        [
            ("/api/admin/integrations/mylar/activity", vec!["get"]),
            (
                "/api/admin/integrations/mylar/requests",
                vec!["get", "post"]
            ),
            ("/api/admin/integrations/mylar/test", vec!["post"]),
            (
                "/api/admin/integrations/mylar/token",
                vec!["delete", "post"]
            ),
        ]
    );
}

#[test]
fn the_mylar_settings_are_the_snapshot_pair_and_the_add_pair() {
    let mut mylar: Vec<(&str, bool)> = registry::registry()
        .iter()
        .filter(|d| d.key.contains("mylar"))
        .map(|d| (d.key, d.is_secret))
        .collect();
    mylar.sort_unstable();
    assert_eq!(
        mylar,
        [
            ("integrations.mylar.api_key", true),
            ("integrations.mylar.enabled", false),
            ("integrations.mylar.stale_after_minutes", false),
            ("integrations.mylar.url", false),
        ],
        "a new Mylar setting widens what Folio can do to Mylar: decide it on purpose"
    );
}

#[test]
fn the_mylar_client_sends_only_its_five_commands() {
    use server::mylar_control::client::Command;
    let mut sent: Vec<&str> = Command::ALL.iter().map(|c| c.as_str()).collect();
    sent.sort_unstable();
    assert_eq!(
        sent,
        [
            "addComic",
            "getComic",
            "getIssueInfo",
            "getVersion",
            "queueIssue"
        ]
    );
    // The client builds a request only from a `Command`: no free-form
    // `cmd` string, and none of Mylar's destructive commands by name.
    let source = include_str!("../src/mylar_control/client.rs");
    assert_eq!(source.matches("(\"cmd\",").count(), 1, "one place sets cmd");
    for forbidden in [
        "delComic",
        "pauseComic",
        "resumeComic",
        "refreshComic",
        "forceSearch",
        "forceProcess",
        "shutdown",
        "restart",
        "update",
        "unqueueIssue",
        "changeStatus",
    ] {
        assert!(
            !source.contains(&format!("\"{forbidden}\"")),
            "the Mylar client must not send `{forbidden}`"
        );
    }
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
                "{name} mentions `{needle}`: snapshot data arrives by push only; \
                 calls to Mylar live in `mylar_control`"
            );
        }
    }
}
