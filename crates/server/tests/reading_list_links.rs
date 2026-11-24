//! Linked reading lists, PR 1: the `reading_list_links` table and how a
//! link's anchor survives a list refresh.
//!
//! - a refresh that moves the anchor issue moves the link's position hint;
//! - a refresh that drops it marks the link `lost` (kept, last position),
//!   and a later refresh that brings it back marks it `placed` again;
//! - a chapter break (PR 3a) follows its issue the same way;
//! - deleting either list removes the link;
//! - the table refuses self-links, anchored `continues` links, anchorless
//!   `event` links and a second `continues` from one list.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::reading_list_link::{self, AnchorStatus, LinkKind, LinkPlacement};
use entity::{cbl_list, reading_list_break};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, ModelTrait};
use server::cbl::import::{RefreshTrigger, apply_parsed};
use uuid::Uuid;

fn cbl(name: &str, books: &[(&str, &str, i32)]) -> String {
    let mut xml =
        format!(r#"<?xml version="1.0" encoding="utf-8"?><ReadingList><Name>{name}</Name><Books>"#);
    for (series, number, cv) in books {
        xml.push_str(&format!(
            r#"<Book Series="{series}" Number="{number}" Volume="2004" Year="2005"><Database Name="cv" Series="1" Issue="{cv}" /></Book>"#
        ));
    }
    xml.push_str("</Books></ReadingList>");
    xml
}

/// Insert a list row and run the real import over `xml`.
async fn import_list(app: &TestApp, xml: &str) -> Uuid {
    let db = &app.state().db;
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    cbl_list::ActiveModel {
        id: Set(id),
        owner_user_id: Set(None),
        source_kind: Set("upload".into()),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set(xml.into()),
        parsed_name: Set("list".into()),
        parsed_matchers_present: Set(true),
        imported_at: Set(now),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    refresh(app, id, xml).await;
    id
}

async fn refresh(app: &TestApp, id: Uuid, xml: &str) -> server::cbl::import::ImportSummary {
    let parsed = parsers::cbl::parse(xml.as_bytes()).unwrap();
    apply_parsed(
        &app.state().db,
        id,
        &parsed,
        xml,
        None,
        RefreshTrigger::Manual,
    )
    .await
    .unwrap()
}

fn event_link(
    parent: Uuid,
    child: Uuid,
    cv: i32,
    series: &str,
    number: &str,
    hint: i32,
) -> reading_list_link::ActiveModel {
    let now = Utc::now().fixed_offset();
    reading_list_link::ActiveModel {
        id: Set(Uuid::now_v7()),
        parent_list_id: Set(parent),
        child_list_id: Set(child),
        kind: Set(LinkKind::Event),
        anchor_cv_issue_id: Set(Some(cv)),
        anchor_series: Set(Some(series.into())),
        anchor_number: Set(Some(number.into())),
        anchor_volume: Set(Some("2004".into())),
        anchor_year: Set(Some("2005".into())),
        anchor_position_hint: Set(Some(hint)),
        anchor_status: Set(AnchorStatus::Placed),
        placement: Set(LinkPlacement::Manual),
        sort_within_anchor: Set(0),
        created_by: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
}

fn continues_link(parent: Uuid, child: Uuid) -> reading_list_link::ActiveModel {
    let mut am = event_link(parent, child, 0, "", "", 0);
    am.kind = Set(LinkKind::Continues);
    am.anchor_cv_issue_id = Set(None);
    am.anchor_series = Set(None);
    am.anchor_number = Set(None);
    am.anchor_volume = Set(None);
    am.anchor_year = Set(None);
    am.anchor_position_hint = Set(None);
    am
}

const MASTER: &[(&str, &str, i32)] = &[
    ("Black Panther", "8", 101),
    ("Excalibur", "11", 102),
    ("Excalibur", "12", 103),
    ("Cable & Deadpool", "18", 104),
];

#[tokio::test]
async fn anchor_follows_a_reorder_and_survives_removal() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let master = import_list(&app, &cbl("Master", MASTER)).await;
    let event = import_list(&app, &cbl("House of M", &[("House of M", "1", 900)])).await;
    let link = event_link(master, event, 103, "Excalibur", "12", 2)
        .insert(db)
        .await
        .unwrap();

    // Upstream inserts two issues before the anchor: it moves to 4.
    let reordered = [
        ("New Avengers", "1", 201),
        ("New Avengers", "2", 202),
        ("Black Panther", "8", 101),
        ("Excalibur", "11", 102),
        ("Excalibur", "12", 103),
        ("Cable & Deadpool", "18", 104),
    ];
    let summary = refresh(&app, master, &cbl("Master", &reordered)).await;
    assert_eq!((summary.links_lost, summary.links_restored), (0, 0));
    let row = reading_list_link::Entity::find_by_id(link.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.anchor_status, AnchorStatus::Placed);
    assert_eq!(row.anchor_position_hint, Some(4));

    // Upstream drops the anchor issue: the link is kept, lost, in place.
    let dropped: Vec<_> = reordered.iter().copied().filter(|b| b.2 != 103).collect();
    let summary = refresh(&app, master, &cbl("Master", &dropped)).await;
    assert_eq!((summary.links_lost, summary.links_restored), (1, 0));
    let row = reading_list_link::Entity::find_by_id(link.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.anchor_status, AnchorStatus::Lost);
    assert_eq!(row.anchor_position_hint, Some(4), "drawn where it last was");

    // It comes back: placed again.
    let summary = refresh(&app, master, &cbl("Master", &reordered)).await;
    assert_eq!((summary.links_lost, summary.links_restored), (0, 1));
    let row = reading_list_link::Entity::find_by_id(link.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.anchor_status, AnchorStatus::Placed);
    assert_eq!(row.anchor_position_hint, Some(4));
}

#[tokio::test]
async fn a_chapter_break_follows_its_issue_across_refreshes() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let master = import_list(&app, &cbl("Master", MASTER)).await;
    let now = Utc::now().fixed_offset();
    let brk = reading_list_break::ActiveModel {
        id: Set(Uuid::now_v7()),
        list_id: Set(master),
        label: Set(Some("Messiah Complex".into())),
        anchor_cv_issue_id: Set(Some(104)),
        anchor_series: Set(Some("Cable & Deadpool".into())),
        anchor_number: Set(Some("18".into())),
        anchor_volume: Set(Some("2004".into())),
        anchor_year: Set(Some("2005".into())),
        anchor_position_hint: Set(Some(3)),
        anchor_status: Set(AnchorStatus::Placed),
        created_by: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();

    let moved = [
        ("Cable & Deadpool", "18", 104),
        ("Black Panther", "8", 101),
        ("Excalibur", "11", 102),
    ];
    let summary = refresh(&app, master, &cbl("Master", &moved)).await;
    assert_eq!((summary.links_lost, summary.links_restored), (0, 0));
    let row = reading_list_break::Entity::find_by_id(brk.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.anchor_status, row.anchor_position_hint),
        (AnchorStatus::Placed, Some(0))
    );

    let summary = refresh(&app, master, &cbl("Master", &moved[1..])).await;
    assert_eq!((summary.links_lost, summary.links_restored), (1, 0));
    let row = reading_list_break::Entity::find_by_id(brk.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.anchor_status, row.anchor_position_hint),
        (AnchorStatus::Lost, Some(0)),
        "kept where it last was"
    );
}

#[tokio::test]
async fn anchor_resolves_by_name_when_the_list_has_no_comicvine_ids() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let master = import_list(&app, &cbl("Master", MASTER)).await;
    let event = import_list(&app, &cbl("Decimation", &[("Decimation", "1", 901)])).await;
    let link = event_link(master, event, 104, "Cable & Deadpool", "18", 3)
        .insert(db)
        .await
        .unwrap();
    let no_ids = r#"<?xml version="1.0" encoding="utf-8"?><ReadingList><Name>Master</Name><Books>
<Book Series="Cable &amp; Deadpool" Number="18" Volume="2004" Year="2005" />
<Book Series="Black Panther" Number="8" Volume="2004" Year="2005" />
</Books></ReadingList>"#;
    refresh(&app, master, no_ids).await;
    let row = reading_list_link::Entity::find_by_id(link.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.anchor_status, AnchorStatus::Placed);
    assert_eq!(row.anchor_position_hint, Some(0));
}

#[tokio::test]
async fn deleting_either_list_removes_the_link() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let master = import_list(&app, &cbl("Master", MASTER)).await;
    let a = import_list(&app, &cbl("A", &[("A", "1", 1)])).await;
    let b = import_list(&app, &cbl("B", &[("B", "1", 2)])).await;
    let to_a = event_link(master, a, 101, "Black Panther", "8", 0)
        .insert(db)
        .await
        .unwrap();
    let to_b = continues_link(master, b).insert(db).await.unwrap();

    cbl_list::Entity::find_by_id(a)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .delete(db)
        .await
        .unwrap();
    assert!(
        reading_list_link::Entity::find_by_id(to_a.id)
            .one(db)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        reading_list_link::Entity::find_by_id(to_b.id)
            .one(db)
            .await
            .unwrap()
            .is_some()
    );

    cbl_list::Entity::find_by_id(master)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .delete(db)
        .await
        .unwrap();
    assert!(
        reading_list_link::Entity::find_by_id(to_b.id)
            .one(db)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn table_constraints_refuse_malformed_links() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let master = import_list(&app, &cbl("Master", MASTER)).await;
    let other = import_list(&app, &cbl("Other", &[("O", "1", 3)])).await;
    let third = import_list(&app, &cbl("Third", &[("T", "1", 4)])).await;

    assert!(
        event_link(master, master, 101, "Black Panther", "8", 0)
            .insert(db)
            .await
            .is_err(),
        "self-link"
    );

    let mut anchored_continues = continues_link(master, other);
    anchored_continues.anchor_cv_issue_id = Set(Some(101));
    assert!(
        anchored_continues.insert(db).await.is_err(),
        "continues with an anchor"
    );

    let mut anchorless_event = continues_link(master, other);
    anchorless_event.kind = Set(LinkKind::Event);
    assert!(
        anchorless_event.insert(db).await.is_err(),
        "event without an anchor"
    );

    continues_link(master, other).insert(db).await.unwrap();
    assert!(
        continues_link(master, third).insert(db).await.is_err(),
        "second continues"
    );
    assert!(
        event_link(master, other, 101, "Black Panther", "8", 0)
            .insert(db)
            .await
            .is_ok(),
        "the same pair may still be linked as an event"
    );
}
