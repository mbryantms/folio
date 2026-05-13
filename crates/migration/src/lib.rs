pub use sea_orm_migration::prelude::*;

mod m20260101_000001_extensions;
mod m20260101_000002_users;
mod m20260101_000003_auth_sessions;
mod m20260101_000004_library_user_access;
mod m20260101_000005_audit_log;
mod m20260201_000001_libraries;
mod m20260201_000002_series_issues;
mod m20260201_000003_progress_placeholder;
mod m20260301_000001_search_docs;
mod m20260507_000001_add_slugs;
mod m20260507_000002_user_language;
mod m20260512_000001_auth_session_id_token_hint;
mod m20260513_000001_app_passwords;
mod m20260513_000002_app_password_scopes;
mod m20260601_000001_user_reading_direction;
mod m20260801_000001_scanner_v1;
mod m20260901_000001_user_preferences;
mod m20260902_000001_thumbnail_state;
mod m20260903_000001_thumbnail_settings;
mod m20260904_000001_thumbnail_quality;
mod m20260910_000001_issue_user_edits;
mod m20260920_000001_scan_run_kind;
mod m20261001_000001_reading_sessions;
mod m20261101_000001_default_cover_solo;
mod m20261201_000001_series_overrides;
mod m20261202_000001_user_ratings;
mod m20261203_000001_metadata_junctions;
mod m20261204_000001_user_series_progress_view;
mod m20261205_000001_saved_views;
mod m20261206_000001_cbl_backend;
mod m20261207_000001_built_in_templates;
mod m20261208_000001_user_view_sidebar;
mod m20261209_000001_issue_comicinfo_count;
mod m20261209_000002_series_status_user_set;
mod m20261210_000001_scanner_perf_state;
mod m20261210_000002_remove_page_count_mismatch_health;
mod m20261211_000001_generate_page_thumbs_on_scan;
mod m20261212_000001_system_saved_views;
mod m20261212_000002_rail_dismissals;
mod m20261213_000001_view_pin_icon;
mod m20261214_000001_user_exclude_aggregates;
mod m20261215_000001_collections;
mod m20261215_000002_markers;
mod m20261215_000003_rename_unstarted_template;
mod m20261216_000001_user_show_marker_count;
mod m20261217_000001_marker_favorite_flag;
mod m20261217_000002_marker_tags;
mod m20261218_000001_people_search;

#[derive(Debug)]
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260101_000001_extensions::Migration),
            Box::new(m20260101_000002_users::Migration),
            Box::new(m20260101_000003_auth_sessions::Migration),
            Box::new(m20260101_000004_library_user_access::Migration),
            Box::new(m20260101_000005_audit_log::Migration),
            Box::new(m20260201_000001_libraries::Migration),
            Box::new(m20260201_000002_series_issues::Migration),
            Box::new(m20260201_000003_progress_placeholder::Migration),
            Box::new(m20260301_000001_search_docs::Migration),
            Box::new(m20260601_000001_user_reading_direction::Migration),
            Box::new(m20260801_000001_scanner_v1::Migration),
            Box::new(m20260901_000001_user_preferences::Migration),
            Box::new(m20260902_000001_thumbnail_state::Migration),
            Box::new(m20260903_000001_thumbnail_settings::Migration),
            Box::new(m20260904_000001_thumbnail_quality::Migration),
            Box::new(m20260910_000001_issue_user_edits::Migration),
            Box::new(m20260920_000001_scan_run_kind::Migration),
            Box::new(m20261001_000001_reading_sessions::Migration),
            Box::new(m20261101_000001_default_cover_solo::Migration),
            Box::new(m20260507_000001_add_slugs::Migration),
            Box::new(m20260507_000002_user_language::Migration),
            Box::new(m20261201_000001_series_overrides::Migration),
            Box::new(m20261202_000001_user_ratings::Migration),
            Box::new(m20261203_000001_metadata_junctions::Migration),
            Box::new(m20261204_000001_user_series_progress_view::Migration),
            Box::new(m20261205_000001_saved_views::Migration),
            Box::new(m20261206_000001_cbl_backend::Migration),
            Box::new(m20261207_000001_built_in_templates::Migration),
            Box::new(m20261208_000001_user_view_sidebar::Migration),
            Box::new(m20261209_000001_issue_comicinfo_count::Migration),
            Box::new(m20261209_000002_series_status_user_set::Migration),
            Box::new(m20261210_000001_scanner_perf_state::Migration),
            Box::new(m20261210_000002_remove_page_count_mismatch_health::Migration),
            Box::new(m20261211_000001_generate_page_thumbs_on_scan::Migration),
            Box::new(m20261212_000001_system_saved_views::Migration),
            Box::new(m20261212_000002_rail_dismissals::Migration),
            Box::new(m20261213_000001_view_pin_icon::Migration),
            Box::new(m20261214_000001_user_exclude_aggregates::Migration),
            Box::new(m20261215_000001_collections::Migration),
            Box::new(m20261215_000002_markers::Migration),
            Box::new(m20261215_000003_rename_unstarted_template::Migration),
            Box::new(m20261216_000001_user_show_marker_count::Migration),
            Box::new(m20261217_000001_marker_favorite_flag::Migration),
            Box::new(m20261217_000002_marker_tags::Migration),
            Box::new(m20261218_000001_people_search::Migration),
            Box::new(m20260512_000001_auth_session_id_token_hint::Migration),
            Box::new(m20260513_000001_app_passwords::Migration),
            Box::new(m20260513_000002_app_password_scopes::Migration),
        ]
    }
}
