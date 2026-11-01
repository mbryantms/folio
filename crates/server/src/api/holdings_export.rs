//! `GET /me/holdings.csv` — the caller's holdings as a spreadsheet-ready
//! CSV (product audit 2026-11 §5 item 14).
//!
//! Two scopes: `series` (one row per series, with owned / read counts and
//! the series-level provider ids) and `issues` (one row per issue file).
//! Only what the caller can open is listed: the library grant and age cap
//! (`access::visible_series_select` / `visible_issues_select`), no
//! soft-deleted series or issues, and only issues whose scan state is
//! `active` (the same set the issue grid lists). Admins additionally get
//! the on-disk folder / file path as the last column; readers never see
//! paths.
//!
//! **Streaming.** Rows are read in id-keyset batches of
//! [`HOLDINGS_BATCH`] and each batch is encoded and sent before the next
//! is fetched, so a 50k-issue library never sits in memory. Every batch
//! projects the columns it writes (never the wide `issue::Model`) and
//! hydrates series / progress / provider ids with one `IN (...)` query
//! each. The first batch is fetched before the response head is sent, so
//! a database failure up front answers a proper 500 envelope; a failure
//! on a later batch aborts the body and the client sees a truncated
//! download.
//!
//! **Encoding.** RFC 4180 via the `csv` crate (fields with a comma,
//! quote, CR or LF are quoted, quotes doubled, CRLF line ends), UTF-8
//! with a leading byte-order mark so Excel opens non-ASCII names
//! correctly. Text cells that start with `=`, `+`, `-`, `@`, tab or CR
//! and are not plain numbers get a leading `'` so a spreadsheet never
//! evaluates a tagged title as a formula (`-1` stays `-1`).
//!
//! Rows come out in id order; sort in the spreadsheet.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{Query, State, rejection::QueryRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{external_id, issue, library, progress_record, series};
use futures::StreamExt;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    QuerySelect,
};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::api::respond;
use crate::auth::CurrentUser;
use crate::library::access::{self, VisibleLibraries};
use crate::middleware::rate_limit;
use crate::state::AppState;
use server_macros::handler;
use shared::error::ApiErrorCode;

/// Rows per keyset batch. Each batch is one projected page query plus
/// one `IN (...)` hydration query per related table, so this also bounds
/// the bind-parameter count.
pub const HOLDINGS_BATCH: u64 = 500;

/// UTF-8 byte-order mark written before the header row. Excel needs it to
/// read the file as UTF-8 instead of the system code page.
pub const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// `external_ids.source` values exported, in column order.
const PROVIDER_SOURCES: [&str; 3] = ["comicvine", "metron", "gcd"];

pub fn routes() -> OpenApiRouter<AppState> {
    // Own sub-router so the rate-limit `route_layer` doesn't leak onto
    // sibling routes when this is merged into the `api` group.
    OpenApiRouter::new()
        .routes(routes!(holdings_csv))
        .route_layer(rate_limit::HOLDINGS_EXPORT.build())
}

/// Which holdings to list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HoldingsScope {
    /// One row per series.
    #[default]
    Series,
    /// One row per issue file.
    Issues,
}

impl HoldingsScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Series => "series",
            Self::Issues => "issues",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct HoldingsQuery {
    #[serde(default)]
    pub scope: HoldingsScope,
    /// Library slug to restrict the export to.
    #[serde(default)]
    pub library: Option<String>,
}

#[utoipa::path(
    operation_id = "holdings_export",
    get,
    path = "/me/holdings.csv",
    params(
        ("scope" = Option<String>, Query, description = "`series` (default) or `issues`; anything else is a 400"),
        ("library" = Option<String>, Query, description = "Library slug; omit for every library the caller can see"),
    ),
    responses(
        (status = 200, content_type = "text/csv", description = "UTF-8 CSV with a byte-order mark, streamed; `Content-Disposition: attachment; filename=\"folio-holdings-<scope>-<date>.csv\"`. Admins get a trailing folder / file path column."),
        (status = 400, description = "unknown scope"),
        (status = 401),
        (status = 404, description = "library slug unknown or not visible to the caller"),
        (status = 429),
    )
)]
#[handler]
pub async fn holdings_csv(
    State(app): State<AppState>,
    user: CurrentUser,
    query: Result<Query<HoldingsQuery>, QueryRejection>,
) -> Response {
    // axum's own rejection is plain text; answer it (an unknown `scope`,
    // usually) with the canonical envelope instead.
    let q = match query {
        Ok(Query(q)) => q,
        Err(rej) => {
            return respond(
                StatusCode::BAD_REQUEST,
                ApiErrorCode::Validation,
                format!(
                    "invalid query (scope is series or issues): {}",
                    rej.body_text()
                ),
            );
        }
    };
    let acl = access::for_user(&app, &user).await;
    // Admins resolve to the unrestricted ACL; that is the only caller who
    // gets on-disk paths.
    let include_paths = acl.unrestricted;

    let library_id = match q.library.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(slug) => {
            let found = library::Entity::find()
                .filter(library::Column::Slug.eq(slug))
                .select_only()
                .column(library::Column::Id)
                .into_tuple::<Uuid>()
                .one(&app.db)
                .await;
            match found {
                Ok(Some(id)) if acl.contains(id) => Some(id),
                Ok(_) => {
                    return respond(
                        StatusCode::NOT_FOUND,
                        ApiErrorCode::NotFound,
                        "library not found",
                    );
                }
                Err(e) => return internal(&e),
            }
        }
    };

    let library_names = match library_names(&app.db).await {
        Ok(m) => m,
        Err(e) => return internal(&e),
    };

    let ctx = Arc::new(Ctx {
        db: app.db.clone(),
        acl,
        user_id: user.id,
        library_id,
        include_paths,
        library_names,
        scope: q.scope,
    });

    // First batch before the head goes out, so an up-front DB failure is a
    // real 500 rather than a 200 with an empty body.
    let first = match next_chunk(&ctx, None).await {
        Ok(c) => c,
        Err(e) => return internal(&e),
    };
    let mut head = Vec::with_capacity(UTF8_BOM.len() + 256 + first.bytes.len());
    head.extend_from_slice(UTF8_BOM);
    head.extend_from_slice(&header_row(ctx.scope, ctx.include_paths));
    head.extend_from_slice(&first.bytes);

    let rest = futures::stream::try_unfold((ctx.clone(), first.next), |(ctx, cursor)| async move {
        let Some(cursor) = cursor else {
            return Ok(None);
        };
        match next_chunk(&ctx, Some(cursor)).await {
            Ok(chunk) => Ok(Some((Bytes::from(chunk.bytes), (ctx, chunk.next)))),
            Err(e) => {
                tracing::error!(error = %e, "holdings export: batch failed mid-stream");
                Err(std::io::Error::other("holdings export batch failed"))
            }
        }
    });
    let body = futures::stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(head)) })
        .chain(rest);

    let filename = format!(
        "folio-holdings-{}-{}.csv",
        ctx.scope.as_str(),
        Utc::now().format("%Y-%m-%d")
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
        ],
        Body::from_stream(body),
    )
        .into_response()
}

fn internal(e: &DbErr) -> Response {
    tracing::error!(error = %e, "holdings export failed");
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "failed to build holdings export",
    )
}

// ───────── batching ─────────

struct Ctx {
    db: DatabaseConnection,
    acl: VisibleLibraries,
    user_id: Uuid,
    library_id: Option<Uuid>,
    include_paths: bool,
    /// Every library's display name. The table is tiny (one row per
    /// configured library), so one up-front read beats a join per batch.
    library_names: HashMap<Uuid, String>,
    scope: HoldingsScope,
}

/// Keyset position: the last id written.
enum Cursor {
    Series(Uuid),
    Issue(String),
}

struct Chunk {
    bytes: Vec<u8>,
    /// `Some` when the batch was full (more rows may follow).
    next: Option<Cursor>,
}

async fn next_chunk(ctx: &Ctx, after: Option<Cursor>) -> Result<Chunk, DbErr> {
    match (ctx.scope, after) {
        (HoldingsScope::Series, None) => series_chunk(ctx, None).await,
        (HoldingsScope::Series, Some(Cursor::Series(id))) => series_chunk(ctx, Some(id)).await,
        (HoldingsScope::Issues, None) => issue_chunk(ctx, None).await,
        (HoldingsScope::Issues, Some(Cursor::Issue(id))) => issue_chunk(ctx, Some(id)).await,
        // A cursor of the other scope would restart the keyset at the top
        // and repeat batch one forever; fail the stream instead.
        _ => Err(DbErr::Custom("holdings cursor does not match scope".into())),
    }
}

async fn library_names(db: &DatabaseConnection) -> Result<HashMap<Uuid, String>, DbErr> {
    let rows: Vec<(Uuid, String)> = library::Entity::find()
        .select_only()
        .column(library::Column::Id)
        .column(library::Column::Name)
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows.into_iter().collect())
}

/// `entity_id → [comicvine, metron, gcd]` (empty string when unset).
type ProviderIds = HashMap<String, [String; 3]>;

async fn provider_ids(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_ids: Vec<String>,
) -> Result<ProviderIds, DbErr> {
    if entity_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, String, String)> = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::EntityId.is_in(entity_ids))
        .filter(external_id::Column::Source.is_in(PROVIDER_SOURCES))
        .select_only()
        .column(external_id::Column::EntityId)
        .column(external_id::Column::Source)
        .column(external_id::Column::ExternalId)
        .into_tuple()
        .all(db)
        .await?;
    let mut out = ProviderIds::new();
    for (entity, source, ext) in rows {
        if let Some(slot) = PROVIDER_SOURCES.iter().position(|s| *s == source) {
            out.entry(entity).or_default()[slot] = ext;
        }
    }
    Ok(out)
}

/// The three provider-id cells for one entity, in column order.
fn provider_cells<'a>(ids: &'a ProviderIds, entity: &str) -> [Cow<'a, str>; 3] {
    match ids.get(entity) {
        Some([cv, metron, gcd]) => [text(cv), text(metron), text(gcd)],
        None => [Cow::Borrowed(""), Cow::Borrowed(""), Cow::Borrowed("")],
    }
}

#[derive(Debug, FromQueryResult)]
struct SeriesRow {
    id: Uuid,
    library_id: Uuid,
    name: String,
    publisher: Option<String>,
    volume: Option<i32>,
    year: Option<i32>,
    status: String,
    total_issues: Option<i32>,
    folder_path: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct SeriesCountRow {
    series_id: Uuid,
    owned: i64,
    read: i64,
}

async fn series_chunk(ctx: &Ctx, after: Option<Uuid>) -> Result<Chunk, DbErr> {
    let mut sel = access::visible_series_select(&ctx.acl);
    if let Some(lib) = ctx.library_id {
        sel = sel.filter(series::Column::LibraryId.eq(lib));
    }
    if let Some(after) = after {
        sel = sel.filter(series::Column::Id.gt(after));
    }
    let rows: Vec<SeriesRow> = sel
        .select_only()
        .column(series::Column::Id)
        .column(series::Column::LibraryId)
        .column(series::Column::Name)
        .column(series::Column::Publisher)
        .column(series::Column::Volume)
        .column(series::Column::Year)
        .column(series::Column::Status)
        .column(series::Column::TotalIssues)
        .column(series::Column::FolderPath)
        .order_by_asc(series::Column::Id)
        .limit(HOLDINGS_BATCH)
        .into_model::<SeriesRow>()
        .all(&ctx.db)
        .await?;

    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let (counts, ext) = futures::try_join!(
        series_counts(ctx, ids.clone()),
        provider_ids(&ctx.db, "series", ids.iter().map(Uuid::to_string).collect()),
    )?;

    let mut w = csv_writer();
    for r in &rows {
        let sid = r.id.to_string();
        let (owned, read) = counts.get(&r.id).copied().unwrap_or((0, 0));
        let [cv, metron, gcd] = provider_cells(&ext, &sid);
        let mut rec: Vec<Cow<'_, str>> = vec![
            text(library_name(ctx, r.library_id)),
            text(r.publisher.as_deref().unwrap_or("")),
            text(&r.name),
            opt_num(r.volume),
            opt_num(r.year),
            text(&r.status),
            Cow::Owned(owned.to_string()),
            opt_num(r.total_issues),
            Cow::Owned(read.to_string()),
            cv,
            metron,
            gcd,
        ];
        if ctx.include_paths {
            rec.push(text(r.folder_path.as_deref().unwrap_or("")));
        }
        write_record(&mut w, &rec)?;
    }
    let next = (rows.len() as u64 == HOLDINGS_BATCH)
        .then(|| rows.last().map(|r| Cursor::Series(r.id)))
        .flatten();
    Ok(Chunk {
        bytes: finish(w)?,
        next,
    })
}

/// `series_id → (owned, read)`. Owned = the series' active issues the caller
/// can open (the issues scope's predicate, so a series row's count equals
/// its issue rows); read = the subset the caller finished. Under an age cap
/// the two scopes can still differ in which *series* appear: a series row
/// follows the series rating, an issue row its own rating first.
async fn series_counts(ctx: &Ctx, ids: Vec<Uuid>) -> Result<HashMap<Uuid, (i64, i64)>, DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = access::visible_issues_select(&ctx.acl)
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::SeriesId.is_in(ids))
        .select_only()
        .column(issue::Column::SeriesId)
        .column_as(Expr::cust("COUNT(*)"), "owned")
        .column_as(
            Expr::cust_with_values(
                r#"COUNT(*) FILTER (WHERE EXISTS (SELECT 1 FROM "progress_records" "pr" WHERE "pr"."user_id" = $1 AND "pr"."issue_id" = "issues"."id" AND "pr"."finished"))"#,
                [ctx.user_id],
            ),
            "read",
        )
        .group_by(issue::Column::SeriesId)
        .into_model::<SeriesCountRow>()
        .all(&ctx.db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.series_id, (r.owned, r.read)))
        .collect())
}

#[derive(Debug, FromQueryResult)]
struct IssueRow {
    id: String,
    library_id: Uuid,
    series_id: Uuid,
    number_raw: Option<String>,
    title: Option<String>,
    volume: Option<i32>,
    year: Option<i32>,
    month: Option<i32>,
    day: Option<i32>,
    format: Option<String>,
    page_count: Option<i32>,
    file_size: i64,
    file_path: String,
}

#[derive(Debug, FromQueryResult)]
struct IssueSeriesRow {
    id: Uuid,
    name: String,
    volume: Option<i32>,
    publisher: Option<String>,
}

async fn issue_chunk(ctx: &Ctx, after: Option<String>) -> Result<Chunk, DbErr> {
    let mut sel = access::visible_issues_select(&ctx.acl).filter(issue::Column::State.eq("active"));
    if let Some(lib) = ctx.library_id {
        sel = sel.filter(issue::Column::LibraryId.eq(lib));
    }
    if let Some(after) = after {
        sel = sel.filter(issue::Column::Id.gt(after));
    }
    let rows: Vec<IssueRow> = sel
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::LibraryId)
        .column(issue::Column::SeriesId)
        .column(issue::Column::NumberRaw)
        .column(issue::Column::Title)
        .column(issue::Column::Volume)
        .column(issue::Column::Year)
        .column(issue::Column::Month)
        .column(issue::Column::Day)
        .column(issue::Column::Format)
        .column(issue::Column::PageCount)
        .column(issue::Column::FileSize)
        .column(issue::Column::FilePath)
        .order_by_asc(issue::Column::Id)
        .limit(HOLDINGS_BATCH)
        .into_model::<IssueRow>()
        .all(&ctx.db)
        .await?;

    let mut series_ids: Vec<Uuid> = rows.iter().map(|r| r.series_id).collect();
    series_ids.sort_unstable();
    series_ids.dedup();
    let issue_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();

    let (series_by_id, progress, ext) = futures::try_join!(
        issue_series(&ctx.db, series_ids),
        issue_progress(ctx, issue_ids.clone()),
        provider_ids(&ctx.db, "issue", issue_ids),
    )?;

    let mut w = csv_writer();
    for r in &rows {
        let s = series_by_id.get(&r.series_id);
        let [cv, metron, gcd] = provider_cells(&ext, &r.id);
        let read_state = match progress.get(&r.id) {
            Some((true, _)) => "read",
            Some((false, last_page)) if *last_page > 0 => "in_progress",
            _ => "unread",
        };
        let mut rec: Vec<Cow<'_, str>> = vec![
            text(library_name(ctx, r.library_id)),
            text(s.and_then(|s| s.publisher.as_deref()).unwrap_or("")),
            text(s.map(|s| s.name.as_str()).unwrap_or("")),
            opt_num(r.volume.or_else(|| s.and_then(|s| s.volume))),
            text(r.number_raw.as_deref().unwrap_or("")),
            text(r.title.as_deref().unwrap_or("")),
            Cow::Owned(cover_date(r.year, r.month, r.day)),
            text(r.format.as_deref().unwrap_or("")),
            opt_num(r.page_count),
            Cow::Owned(r.file_size.to_string()),
            Cow::Borrowed(read_state),
            cv,
            metron,
            gcd,
        ];
        if ctx.include_paths {
            rec.push(text(&r.file_path));
        }
        write_record(&mut w, &rec)?;
    }
    let next = (rows.len() as u64 == HOLDINGS_BATCH)
        .then(|| rows.last().map(|r| Cursor::Issue(r.id.clone())))
        .flatten();
    Ok(Chunk {
        bytes: finish(w)?,
        next,
    })
}

async fn issue_series(
    db: &DatabaseConnection,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, IssueSeriesRow>, DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = series::Entity::find()
        .filter(series::Column::Id.is_in(ids))
        .select_only()
        .column(series::Column::Id)
        .column(series::Column::Name)
        .column(series::Column::Volume)
        .column(series::Column::Publisher)
        .into_model::<IssueSeriesRow>()
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|s| (s.id, s)).collect())
}

/// `issue_id → (finished, last_page)` for the caller.
async fn issue_progress(
    ctx: &Ctx,
    ids: Vec<String>,
) -> Result<HashMap<String, (bool, i32)>, DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, bool, i32)> = progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(ctx.user_id))
        .filter(progress_record::Column::IssueId.is_in(ids))
        .select_only()
        .column(progress_record::Column::IssueId)
        .column(progress_record::Column::Finished)
        .column(progress_record::Column::LastPage)
        .into_tuple()
        .all(&ctx.db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, finished, last_page)| (id, (finished, last_page)))
        .collect())
}

fn library_name(ctx: &Ctx, id: Uuid) -> &str {
    ctx.library_names.get(&id).map(String::as_str).unwrap_or("")
}

// ───────── encoding ─────────

/// Column headers, in row order. The path column is appended for admins only.
pub fn header_names(scope: HoldingsScope, include_paths: bool) -> Vec<&'static str> {
    let mut cols: Vec<&'static str> = match scope {
        HoldingsScope::Series => vec![
            "library",
            "publisher",
            "series",
            "volume",
            "start_year",
            "status",
            "issues_owned",
            "total_issues",
            "issues_read",
            "comicvine_id",
            "metron_id",
            "gcd_id",
        ],
        HoldingsScope::Issues => vec![
            "library",
            "publisher",
            "series",
            "volume",
            "number",
            "title",
            "cover_date",
            "format",
            "page_count",
            "file_size_bytes",
            "read_state",
            "comicvine_id",
            "metron_id",
            "gcd_id",
        ],
    };
    if include_paths {
        cols.push(match scope {
            HoldingsScope::Series => "folder_path",
            HoldingsScope::Issues => "file_path",
        });
    }
    cols
}

fn header_row(scope: HoldingsScope, include_paths: bool) -> Vec<u8> {
    let mut w = csv_writer();
    // Static ASCII names: encoding into a Vec cannot fail.
    let _ = w.write_record(header_names(scope, include_paths));
    finish(w).unwrap_or_default()
}

fn csv_writer() -> csv::Writer<Vec<u8>> {
    csv::WriterBuilder::new()
        .terminator(csv::Terminator::CRLF)
        .from_writer(Vec::new())
}

fn write_record(w: &mut csv::Writer<Vec<u8>>, rec: &[Cow<'_, str>]) -> Result<(), DbErr> {
    w.write_record(rec.iter().map(|c| c.as_ref()))
        .map_err(|e| DbErr::Custom(format!("csv encode: {e}")))
}

fn finish(w: csv::Writer<Vec<u8>>) -> Result<Vec<u8>, DbErr> {
    w.into_inner()
        .map_err(|e| DbErr::Custom(format!("csv flush: {e}")))
}

fn opt_num(v: Option<i32>) -> Cow<'static, str> {
    v.map(|n| Cow::Owned(n.to_string()))
        .unwrap_or(Cow::Borrowed(""))
}

/// `YYYY-MM-DD`, `YYYY-MM` or `YYYY` from the ComicInfo date parts; empty
/// when the year is unknown.
fn cover_date(year: Option<i32>, month: Option<i32>, day: Option<i32>) -> String {
    match (year, month, day) {
        (Some(y), Some(m), Some(d)) => format!("{y:04}-{m:02}-{d:02}"),
        (Some(y), Some(m), None) => format!("{y:04}-{m:02}"),
        (Some(y), _, _) => format!("{y:04}"),
        _ => String::new(),
    }
}

/// Neutralise spreadsheet formula injection: a text cell that a
/// spreadsheet would evaluate (leading `=`, `+`, `-`, `@`, tab, CR) gets a
/// leading `'`, unless it is a plain number such as issue `-1`.
pub fn text(s: &str) -> Cow<'_, str> {
    let risky = s
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r'));
    if risky && !is_plain_number(s) {
        Cow::Owned(format!("'{s}"))
    } else {
        Cow::Borrowed(s)
    }
}

/// `[+-]digits[.digits]` and nothing else. Stricter than `f64::from_str`,
/// which also takes `inf` / `NaN` and would let `-inf` through unguarded.
fn is_plain_number(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    !int.is_empty() && digits(int) && digits(frac) && !(body.contains('.') && frac.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(fields: &[&str]) -> String {
        let mut w = csv_writer();
        let rec: Vec<Cow<'_, str>> = fields.iter().map(|f| text(f)).collect();
        write_record(&mut w, &rec).unwrap();
        String::from_utf8(finish(w).unwrap()).unwrap()
    }

    #[test]
    fn quotes_commas_quotes_and_newlines() {
        assert_eq!(
            encode(&["plain", "a, b", "say \"hi\"", "two\nlines"]),
            "plain,\"a, b\",\"say \"\"hi\"\"\",\"two\nlines\"\r\n"
        );
    }

    #[test]
    fn neutralises_formula_cells_but_not_numbers() {
        assert_eq!(text("=HYPERLINK(\"x\")"), "'=HYPERLINK(\"x\")");
        assert_eq!(text("+cmd"), "'+cmd");
        assert_eq!(text("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(text("-1"), "-1");
        assert_eq!(text("-0.5"), "-0.5");
        assert_eq!(text("-Hellblazer"), "'-Hellblazer");
        assert_eq!(text("-inf"), "'-inf");
        assert_eq!(text("+Infinity"), "'+Infinity");
        assert_eq!(text("-NaN"), "'-NaN");
        assert_eq!(text("\t1"), "'\t1");
        assert_eq!(text("-1."), "'-1.");
        assert_eq!(text("+3"), "+3");
        assert_eq!(text("Batman"), "Batman");
        assert_eq!(text(""), "");
    }

    #[test]
    fn cover_date_precision() {
        assert_eq!(cover_date(Some(1987), Some(2), Some(9)), "1987-02-09");
        assert_eq!(cover_date(Some(1987), Some(2), None), "1987-02");
        assert_eq!(cover_date(Some(1987), None, None), "1987");
        assert_eq!(cover_date(None, Some(2), Some(9)), "");
    }

    #[test]
    fn path_column_is_admin_only() {
        assert!(!header_names(HoldingsScope::Series, false).contains(&"folder_path"));
        assert_eq!(
            header_names(HoldingsScope::Series, true).last(),
            Some(&"folder_path")
        );
        assert!(!header_names(HoldingsScope::Issues, false).contains(&"file_path"));
        assert_eq!(
            header_names(HoldingsScope::Issues, true).last(),
            Some(&"file_path")
        );
    }
}
