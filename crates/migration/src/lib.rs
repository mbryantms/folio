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
mod m20260514_000001_app_settings;
mod m20260514_000002_series_volume_uniq;
mod m20260514_000003_cbl_list_cascade_saved_view;
mod m20260514_000004_user_sidebar_entries;
mod m20260515_000001_user_pages;
mod m20260515_000002_sidebar_entries_page_kind;
mod m20260515_000003_user_page_description;
mod m20260515_000004_sidebar_headers_spacers;
mod m20260516_000001_issues_content_hash_idx;
mod m20260518_000001_series_reading_direction;
mod m20260519_000001_drop_orphan_cbl_lists;
mod m20260520_000001_marker_kind_favorite;
mod m20260520_000002_user_default_page_animation;
mod m20260522_000001_progress_records_finished_at;
mod m20260522_000002_log_widgets;
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
mod m20261219_000001_character_team_location_junctions;
mod m20261220_000001_opds_reorder_opt_outs;
mod m20261221_000001_opds_progress_glyphs;
mod m20261222_000001_user_max_rails_per_page;
mod m20261223_000001_person;
mod m20261224_000001_drop_stale_unstarted_templates;
mod m20261225_000001_credit_person_id;
mod m20261226_000001_progress_is_backfill;
mod m20261227_000001_hide_from_log;
mod m20261228_000001_metadata_providers_schema;
mod m20261229_000001_metadata_cache;
mod m20261230_000001_metadata_run_candidates;
mod m20261231_000001_archive_writeback_schema;
mod m20270101_000001_match_outcomes;
mod m20270102_000001_library_publisher_blacklist;
mod m20270103_000001_library_filename_inference_flags;
mod m20270104_000001_issue_cover_page_index;
mod m20270105_000001_library_auto_apply_strong_matches;
mod m20270106_000001_metadata_cache_schema_version;
mod m20270107_000001_archive_backup_retain_allow_zero;
mod m20270108_000001_archive_edit_schema;
mod m20270109_000001_series_auto_sync_opt_in;

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
            Box::new(m20260514_000001_app_settings::Migration),
            Box::new(m20260514_000002_series_volume_uniq::Migration),
            Box::new(m20260514_000003_cbl_list_cascade_saved_view::Migration),
            Box::new(m20260514_000004_user_sidebar_entries::Migration),
            Box::new(m20260515_000001_user_pages::Migration),
            Box::new(m20260515_000002_sidebar_entries_page_kind::Migration),
            Box::new(m20260515_000003_user_page_description::Migration),
            Box::new(m20260515_000004_sidebar_headers_spacers::Migration),
            Box::new(m20260516_000001_issues_content_hash_idx::Migration),
            Box::new(m20260518_000001_series_reading_direction::Migration),
            Box::new(m20261219_000001_character_team_location_junctions::Migration),
            Box::new(m20261220_000001_opds_reorder_opt_outs::Migration),
            Box::new(m20261221_000001_opds_progress_glyphs::Migration),
            Box::new(m20260519_000001_drop_orphan_cbl_lists::Migration),
            Box::new(m20260520_000001_marker_kind_favorite::Migration),
            Box::new(m20260520_000002_user_default_page_animation::Migration),
            Box::new(m20260522_000001_progress_records_finished_at::Migration),
            Box::new(m20260522_000002_log_widgets::Migration),
            Box::new(m20261222_000001_user_max_rails_per_page::Migration),
            Box::new(m20261223_000001_person::Migration),
            Box::new(m20261224_000001_drop_stale_unstarted_templates::Migration),
            Box::new(m20261225_000001_credit_person_id::Migration),
            Box::new(m20261226_000001_progress_is_backfill::Migration),
            Box::new(m20261227_000001_hide_from_log::Migration),
            Box::new(m20261228_000001_metadata_providers_schema::Migration),
            Box::new(m20261229_000001_metadata_cache::Migration),
            Box::new(m20261230_000001_metadata_run_candidates::Migration),
            Box::new(m20261231_000001_archive_writeback_schema::Migration),
            Box::new(m20270101_000001_match_outcomes::Migration),
            Box::new(m20270102_000001_library_publisher_blacklist::Migration),
            Box::new(m20270103_000001_library_filename_inference_flags::Migration),
            Box::new(m20270104_000001_issue_cover_page_index::Migration),
            Box::new(m20270105_000001_library_auto_apply_strong_matches::Migration),
            Box::new(m20270106_000001_metadata_cache_schema_version::Migration),
            Box::new(m20270107_000001_archive_backup_retain_allow_zero::Migration),
            Box::new(m20270108_000001_archive_edit_schema::Migration),
            Box::new(m20270109_000001_series_auto_sync_opt_in::Migration),
        ]
    }
}

#[cfg(test)]
mod tests {
    /// Guards against out-of-order `mod m<timestamp>` declarations at the
    /// top of this file. We don't enforce this on `Migrator::migrations()`
    /// because that vec's order is intentionally non-chronological — some
    /// migrations were inserted out-of-band and must run after later ones.
    /// The `mod` block, however, is a flat alphabetical list and a B-class
    /// gate failure (cargo fmt --check) caught the last time it drifted.
    #[test]
    fn migration_mod_declarations_are_sorted() {
        let src = include_str!("lib.rs");
        let mods: Vec<&str> = src
            .lines()
            .filter_map(|l| l.strip_prefix("mod "))
            .filter_map(|l| l.strip_suffix(';'))
            .filter(|l| l.starts_with("m2"))
            .collect();
        for w in mods.windows(2) {
            assert!(
                w[0] < w[1],
                "migration mod declarations out of order: `{}` precedes `{}`",
                w[0],
                w[1],
            );
        }
    }
}
