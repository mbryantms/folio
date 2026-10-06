//! Coverage analysis works on the series' main run only: issues the
//! scanner tagged with a `special_type` (annuals / one-shots / specials /
//! collected editions) are left out of the local issue set and reported
//! separately, so `Annuals/… Annual 001.cbz` (number `1`) is never
//! analyzed as the run's #1.

mod common;

use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use server::metadata::coverage::{ExcludedSpecial, load_local, load_local_with_specials};
use std::path::Path;
use tempfile::tempdir;
use uuid::Uuid;

async fn issue(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    dir: &Path,
    name: &str,
    number: &str,
    special_type: Option<&str>,
) {
    let path = dir.join(format!("{name}.cbz"));
    let sort_number: f64 = number.parse().unwrap();
    let id = IssueSeed::new(lib_id, series_id, &path, name.as_bytes(), sort_number)
        .insert(&app.state().db)
        .await;
    let mut am: entity::issue::ActiveModel = entity::issue::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.number_raw = Set(Some(number.to_owned()));
    am.year = Set(Some(1987));
    am.special_type = Set(special_type.map(str::to_owned));
    am.update(&app.state().db).await.unwrap();
}

#[tokio::test]
async fn tagged_specials_are_excluded_from_the_local_run_and_reported() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Adventures of Superman")
        .insert(&app.state().db)
        .await;
    let d = dir.path();
    issue(&app, lib_id, series_id, d, "AoS 000", "0", None).await;
    issue(&app, lib_id, series_id, d, "AoS 424", "424", None).await;
    issue(&app, lib_id, series_id, d, "AoS 425", "425", None).await;
    // The annual's filename parsed to a bare `1`.
    issue(
        &app,
        lib_id,
        series_id,
        d,
        "AoS Annual 001",
        "1",
        Some("Annual"),
    )
    .await;
    issue(
        &app,
        lib_id,
        series_id,
        d,
        "AoS Annual 002",
        "2",
        Some("Annual"),
    )
    .await;
    issue(
        &app,
        lib_id,
        series_id,
        d,
        "AoS Special",
        "1",
        Some("Special"),
    )
    .await;

    let local = load_local(&app.state().db, series_id).await.unwrap();
    let numbers: Vec<&str> = local.iter().map(|l| l.canonical.as_str()).collect();
    assert_eq!(
        numbers,
        vec!["0", "424", "425"],
        "no #1 / #2 from the annuals"
    );

    let (again, excluded) = load_local_with_specials(&app.state().db, series_id)
        .await
        .unwrap();
    assert_eq!(again.len(), 3);
    assert_eq!(
        excluded,
        vec![
            ExcludedSpecial {
                number: Some("1".into()),
                special_type: "Annual".into()
            },
            ExcludedSpecial {
                number: Some("1".into()),
                special_type: "Special".into()
            },
            ExcludedSpecial {
                number: Some("2".into()),
                special_type: "Annual".into()
            },
        ]
    );
}
