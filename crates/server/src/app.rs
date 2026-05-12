//! Router assembly + listener.

use crate::api;
use crate::auth;
use crate::config::Config;
use crate::middleware::request_context::{self, TrustedProxies};
use crate::middleware::security_headers::{self, SecurityHeaders};
use crate::observability::ObservabilityHandles;
use crate::state::AppState;
use axum::Router;
use sea_orm::{ConnectOptions, Database};
use std::sync::Arc;
use std::time::Duration;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use tracing::Level;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Comic Reader API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Self-hostable comic reader. See comic-reader-spec.md."
    ),
    paths(
        api::health::healthz,
        api::health::readyz,
        auth::local::register,
        auth::local::login,
        auth::local::refresh,
        auth::local::logout,
        auth::local::me,
        auth::local::update_preferences,
        auth::local::request_password_reset,
        auth::local::reset_password,
        auth::local::verify_email,
        auth::local::resend_verification,
        api::account::update,
        auth::ws_ticket::mint,
        api::libraries::list,
        api::libraries::get_one,
        api::libraries::create,
        api::libraries::update_settings,
        api::libraries::delete_one,
        api::libraries::scan,
        api::libraries::scan_preview,
        api::health_issues::list,
        api::health_issues::dismiss,
        api::reconcile::list_removed,
        api::reconcile::restore_issue,
        api::reconcile::confirm_issue,
        api::scan_runs::list,
        api::admin_queue::queue_depth,
        api::admin_queue::clear_queue,
        api::admin_thumbs::library_status,
        api::admin_thumbs::get_settings,
        api::admin_thumbs::update_settings,
        api::admin_thumbs::generate_missing,
        api::admin_thumbs::generate_page_map,
        api::admin_thumbs::force_recreate,
        api::admin_thumbs::delete_all,
        api::admin_thumbs::regenerate_series_cover,
        api::admin_thumbs::generate_series_page_map,
        api::admin_thumbs::force_recreate_series_page_map,
        api::admin_thumbs::regenerate_issue_cover,
        api::admin_thumbs::generate_issue_page_map,
        api::admin_thumbs::force_recreate_issue_page_map,
        api::admin_users::list,
        api::admin_users::get_one,
        api::admin_users::update,
        api::admin_users::disable,
        api::admin_users::enable,
        api::admin_users::set_library_access,
        api::audit::list,
        api::series::list,
        api::series::get_one,
        api::series::update_series,
        api::series::scan_series,
        api::series::list_issues,
        api::issues::get_one,
        api::issues::update,
        api::issues::scan_issue,
        api::issues::next_in_series,
        api::series::resume,
        api::issues::search,
        api::issues::list,
        api::people::list,
        api::ratings::set_series_rating,
        api::ratings::set_issue_rating,
        api::progress::upsert_series,
        api::rails::continue_reading,
        api::rails::on_deck,
        api::rails::create_dismissal,
        api::rails::delete_dismissal,
        api::reading_sessions::upsert,
        api::reading_sessions::list,
        api::reading_sessions::stats,
        api::reading_sessions::clear_history,
        api::admin_stats::overview,
        api::admin_stats::user_reading_stats,
        api::admin_stats::users_list,
        api::admin_stats::engagement,
        api::admin_stats::content,
        api::admin_stats::quality,
        api::saved_views::list,
        api::saved_views::create,
        api::saved_views::update,
        api::saved_views::delete_one,
        api::saved_views::pin,
        api::saved_views::unpin,
        api::saved_views::set_sidebar,
        api::saved_views::set_icon,
        api::saved_views::reorder,
        api::saved_views::results,
        api::saved_views::preview,
        api::saved_views::admin_create,
        api::saved_views::admin_update,
        api::saved_views::admin_delete,
        api::cbl_lists::list,
        api::cbl_lists::detail,
        api::cbl_lists::upload,
        api::cbl_lists::create_from_json,
        api::cbl_lists::update,
        api::cbl_lists::delete_one,
        api::cbl_lists::refresh_one,
        api::cbl_lists::refresh_log,
        api::cbl_lists::issues,
        api::cbl_lists::reading_window,
        api::cbl_lists::manual_match,
        api::cbl_lists::clear_match,
        api::cbl_lists::export,
        api::cbl_lists::list_catalog_sources,
        api::cbl_lists::list_catalog_entries,
        api::cbl_lists::refresh_catalog_index,
        api::cbl_lists::admin_create_catalog_source,
        api::cbl_lists::admin_update_catalog_source,
        api::cbl_lists::admin_delete_catalog_source,
        api::collections::list,
        api::collections::create,
        api::collections::update,
        api::collections::delete_one,
        api::collections::list_entries,
        api::collections::add_entry,
        api::collections::remove_entry,
        api::collections::reorder_entries,
        api::markers::list,
        api::markers::create,
        api::markers::count,
        api::markers::tags_index,
        api::markers::update,
        api::markers::delete_one,
        api::markers::list_for_issue,
        api::filter_options::genres,
        api::filter_options::tags,
        api::filter_options::credits,
        api::filter_options::publishers,
        api::filter_options::languages,
        api::filter_options::age_ratings,
        api::filter_options::characters,
        api::filter_options::teams,
        api::filter_options::locations,
        api::admin_logs::list,
        api::admin_activity::list,
        api::auth_config::get_config,
        api::auth_config::get_public_config,
        api::server_info::info,
        api::sessions::list,
        api::sessions::revoke_one,
        api::sessions::revoke_all,
        api::app_passwords::list,
        api::app_passwords::create,
        api::app_passwords::revoke,
    ),
    components(schemas(
        shared::error::ApiError,
        shared::error::ApiErrorBody,
        auth::local::RegisterReq,
        auth::local::LoginReq,
        auth::local::LoginResp,
        auth::local::MeResp,
        auth::local::PreferencesReq,
        auth::local::RequestPasswordResetReq,
        auth::local::ResetPasswordReq,
        auth::local::ResendVerificationReq,
        api::account::AccountReq,
        auth::ws_ticket::WsTicketResp,
        api::libraries::LibraryView,
        api::libraries::CreateLibraryReq,
        api::libraries::UpdateLibraryReq,
        api::libraries::DeleteLibraryResp,
        api::libraries::ScanResp,
        api::libraries::ScanMode,
        api::libraries::ScanPreviewView,
        api::health_issues::HealthIssueView,
        api::reconcile::RemovedListView,
        api::reconcile::RemovedIssueView,
        api::reconcile::RemovedSeriesView,
        api::scan_runs::ScanRunView,
        api::admin_queue::QueueDepthView,
        api::admin_queue::QueueClearReq,
        api::admin_queue::QueueClearResp,
        api::admin_queue::QueueClearTarget,
        api::admin_thumbs::ThumbnailsStatusView,
        api::admin_thumbs::ThumbnailsSettingsView,
        api::admin_thumbs::UpdateThumbnailsSettingsReq,
        api::admin_thumbs::RegenerateResp,
        api::admin_thumbs::DeleteAllResp,
        api::admin_users::AdminUserView,
        api::admin_users::AdminUserListView,
        api::admin_users::AdminUserDetailView,
        api::admin_users::LibraryAccessGrantView,
        api::admin_users::UpdateUserReq,
        api::admin_users::LibraryAccessReq,
        api::audit::AuditEntryView,
        api::audit::AuditListView,
        api::series::SeriesView,
        api::series::SeriesListView,
        api::series::SeriesResumeView,
        api::series::UpdateSeriesReq,
        api::series::IssueSummaryView,
        api::series::IssueListView,
        api::series::IssueDetailView,
        api::series::IssueLink,
        api::issues::UpdateIssueReq,
        api::issues::NextInSeriesView,
        api::issues::IssueSearchView,
        api::issues::IssueSearchHit,
        api::people::PeopleListView,
        api::people::PersonHit,
        api::ratings::SetRatingReq,
        api::ratings::RatingView,
        api::progress::UpsertSeriesReq,
        api::progress::UpsertSeriesResp,
        api::rails::ContinueReadingView,
        api::rails::ContinueReadingCard,
        api::rails::OnDeckView,
        api::rails::OnDeckCard,
        api::rails::ProgressInfo,
        api::rails::CreateDismissalReq,
        api::reading_sessions::UpsertReq,
        api::reading_sessions::ReadingSessionView,
        api::reading_sessions::ReadingSessionListView,
        api::reading_sessions::ReadingStatsView,
        api::reading_sessions::TotalsView,
        api::reading_sessions::DayBucket,
        api::reading_sessions::TopSeriesEntry,
        api::reading_sessions::TopNameEntry,
        api::reading_sessions::TopCreatorEntry,
        api::reading_sessions::DowHourCell,
        api::reading_sessions::TimeOfDayBuckets,
        api::reading_sessions::TimeOfDayCell,
        api::reading_sessions::PacePoint,
        api::reading_sessions::RereadIssueEntry,
        api::reading_sessions::RereadSeriesEntry,
        api::reading_sessions::CompletionView,
        api::reading_sessions::ClearHistoryResp,
        api::admin_stats::OverviewView,
        api::admin_stats::TotalsBlock,
        api::admin_stats::HealthBlock,
        api::admin_stats::AdminUserStatsListView,
        api::admin_stats::AdminUserStatsRow,
        api::admin_stats::DeviceBucket,
        api::admin_stats::EngagementView,
        api::admin_stats::EngagementPoint,
        api::admin_stats::ContentInsightsView,
        api::admin_stats::DeadStockEntry,
        api::admin_stats::AbandonedEntry,
        api::admin_stats::FunnelBucket,
        api::admin_stats::DataQualityView,
        api::admin_stats::MetadataCoverageView,
        api::saved_views::SavedViewView,
        api::saved_views::SavedViewListView,
        api::saved_views::CreateSavedViewReq,
        api::saved_views::UpdateSavedViewReq,
        api::saved_views::ReorderReq,
        api::saved_views::PinView,
        api::saved_views::PreviewReq,
        api::saved_views::SetIconReq,
        crate::views::dsl::FilterDsl,
        crate::views::dsl::Condition,
        crate::views::dsl::Field,
        crate::views::dsl::Op,
        crate::views::dsl::MatchMode,
        crate::views::dsl::SortField,
        crate::views::dsl::SortOrder,
        api::cbl_lists::CblListView,
        api::cbl_lists::CblListListView,
        api::cbl_lists::CblStatsView,
        api::cbl_lists::CblEntryView,
        api::cbl_lists::CblWindowView,
        api::cbl_lists::CblWindowEntry,
        api::cbl_lists::CblDetailView,
        api::cbl_lists::CreateCblListReq,
        api::cbl_lists::UpdateCblListReq,
        api::cbl_lists::ManualMatchReq,
        api::cbl_lists::RefreshLogEntryView,
        api::cbl_lists::RefreshLogListView,
        api::cbl_lists::CatalogSourceView,
        api::cbl_lists::CatalogSourceListView,
        api::cbl_lists::CatalogEntryView,
        api::cbl_lists::CatalogEntriesView,
        api::cbl_lists::CreateCatalogSourceReq,
        api::cbl_lists::UpdateCatalogSourceReq,
        crate::cbl::import::ImportSummary,
        api::collections::CollectionEntryView,
        api::collections::CollectionEntriesView,
        api::collections::CreateCollectionReq,
        api::collections::UpdateCollectionReq,
        api::collections::AddEntryReq,
        api::collections::ReorderEntriesReq,
        api::markers::MarkerView,
        api::markers::MarkerListView,
        api::markers::MarkerCountView,
        api::markers::MarkerTagsView,
        api::markers::TagEntryView,
        api::markers::IssueMarkersView,
        api::markers::CreateMarkerReq,
        api::markers::UpdateMarkerReq,
        api::filter_options::OptionsView,
        api::admin_logs::LogsResp,
        api::admin_logs::LogEntryView,
        api::admin_activity::ActivityListView,
        api::admin_activity::ActivityEntryView,
        api::auth_config::AuthConfigView,
        api::auth_config::OidcConfigView,
        api::auth_config::LocalConfigView,
        api::auth_config::PublicAuthConfigView,
        api::server_info::ServerInfoView,
        api::sessions::SessionView,
        api::sessions::SessionListView,
        api::sessions::RevokeAllResp,
        api::app_passwords::AppPasswordView,
        api::app_passwords::AppPasswordListView,
        api::app_passwords::AppPasswordCreatedView,
        api::app_passwords::CreateAppPasswordReq,
    ))
)]
pub struct ApiDoc;

pub fn openapi_spec() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

pub async fn serve(cfg: Config, handles: ObservabilityHandles) -> anyhow::Result<()> {
    let mut db_opts = ConnectOptions::new(cfg.database_url.clone());
    db_opts
        .max_connections(30)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(60))
        .sqlx_logging(false);
    let db = Database::connect(db_opts).await?;

    if cfg.auto_migrate {
        use migration::MigratorTrait;
        tracing::info!("running migrations");
        migration::Migrator::up(&db, None).await?;
    }

    let secrets = crate::secrets::Secrets::load(&cfg.data_path)?;

    let jobs = crate::jobs::JobRuntime::new(&cfg.redis_url, db.clone()).await?;

    let email = crate::email::build(&cfg)?;

    let bind = cfg.bind_addr;
    let state = AppState::new(
        cfg,
        db,
        secrets,
        handles.prometheus,
        handles.log_buffer,
        jobs,
        email,
    );

    // Thumbnail catchup (M6): always run at boot — finds rows with
    // `thumbnails_generated_at IS NULL` (new since last boot) or
    // `thumbnail_version < CURRENT` (constant bumped). Cheap query, gated
    // by the partial index `issues_thumbs_pending_idx`.
    let _ = crate::jobs::post_scan::enqueue_pending_all_libraries(&state).await;

    // Spawn the apalis monitor in the background. We only `await` the HTTP
    // server here; on graceful shutdown the monitor task is dropped and
    // tokio cancels the workers.
    let jobs_handle = state.jobs.clone();
    let jobs_state = state.clone();
    tokio::spawn(async move {
        jobs_handle.run(jobs_state).await;
    });

    // Cron scheduler — best-effort. Failure here doesn't block startup.
    match crate::jobs::scheduler::start(state.clone()).await {
        Ok(scheduler) => {
            state.set_scheduler(scheduler).await;
        }
        Err(e) => tracing::error!(error = %e, "scheduler failed to start"),
    }

    let app = router(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(addr = %bind, "listening");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

pub fn router(state: AppState) -> Router {
    let request_id_header = axum::http::HeaderName::from_static("x-request-id");
    let sec_headers = Arc::new(SecurityHeaders::new(&state.cfg));
    let trusted_proxies = TrustedProxies::from_config(&state.cfg.trusted_proxies);

    Router::new()
        .merge(api::health::routes())
        .merge(api::csp::routes())
        .merge(api::meta::routes())
        .merge(auth::local::routes())
        .merge(api::account::routes())
        .merge(auth::oidc::routes())
        .merge(auth::ws_ticket::routes())
        .merge(api::libraries::routes())
        .merge(api::health_issues::routes())
        .merge(api::reconcile::routes())
        .merge(api::scan_runs::routes())
        .merge(api::admin_queue::routes())
        .merge(api::admin_thumbs::routes())
        .merge(api::admin_users::routes())
        .merge(api::audit::routes())
        .merge(api::series::routes())
        .merge(api::ws_scan_events::routes())
        .merge(api::issues::routes())
        .merge(api::people::routes())
        .merge(api::page_bytes::routes())
        .merge(api::thumbnails::routes())
        .merge(api::progress::routes())
        .merge(api::rails::routes())
        .merge(api::ratings::routes())
        .merge(api::reading_sessions::routes())
        .merge(api::admin_stats::routes())
        .merge(api::saved_views::routes())
        .merge(api::cbl_lists::routes())
        .merge(api::collections::routes())
        .merge(api::markers::routes())
        .merge(api::filter_options::routes())
        .merge(api::admin_logs::routes())
        .merge(api::admin_activity::routes())
        .merge(api::auth_config::routes())
        .merge(api::server_info::routes())
        .merge(api::sessions::routes())
        .merge(api::app_passwords::routes())
        .merge(api::opds::routes())
        .layer(axum::middleware::from_fn(auth::csrf::require_csrf))
        // Order matters: outermost wraps innermost. `set_context` needs to run
        // before handlers so `Request::extensions::get::<RequestContext>()`
        // works inside extractors and handlers.
        .layer(axum::middleware::from_fn_with_state(
            trusted_proxies,
            request_context::set_context,
        ))
        .layer(axum::middleware::from_fn_with_state(
            sec_headers,
            security_headers::set_headers,
        ))
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(
            request_id_header.clone(),
            MakeRequestUuid,
        ))
        // Custom request span: emit `path` only, never `uri`. The default
        // `TraceLayer::new_for_http()` captures the full URI including the
        // query string, which would leak credentials into the trace log if
        // anything ever submitted them as GET params (e.g. an unprotected
        // form falling through to its native handler). The auth-hardening
        // M9 audit found exactly this regression in the wild — fixing it
        // here is defense-in-depth so any future surface that mis-handles
        // credentials in a query string doesn't also poison journald.
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
                tracing::span!(
                    Level::INFO,
                    "http",
                    method = %req.method(),
                    path = %req.uri().path(),
                )
            }),
        )
        .with_state(state)
}

async fn shutdown_signal() {
    use tokio::signal;
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received; draining");
}
