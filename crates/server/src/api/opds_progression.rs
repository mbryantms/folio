//! OPDS Progression 1.0 — M5 of progress-writeback-2.0.
//!
//! Spec: <https://drafts.opds.io/opds-progression-1.0> (merged into
//! `opds-community/drafts` 2026-03-01). Defines two verbs:
//!
//! - **GET** `/opds/v1/progression/{issue_id}` returns the user's
//!   last-known progression as `application/opds-progression+json`.
//!   404 when no progress row exists (spec is silent; 404 is the
//!   most charitable interpretation — "no resource yet" rather than
//!   inventing a zero-filled body).
//! - **PUT** `/opds/v1/progression/{issue_id}` accepts the same media
//!   type and writes progression. 204 on success.
//!
//! Discovery: each OPDS issue entry advertises a
//! `<link rel="http://opds-spec.org/progression"
//!        href="/opds/v1/progression/{id}"
//!        type="application/opds-progression+json"/>` so clients can
//! find the endpoint per-publication. See
//! `crate::api::opds::render_issue_acq_entry`.
//!
//! Error responses are RFC 7807 Problem Details with typed URIs at
//! `https://registry.opds.io/error#progression-*`:
//! - `progression-invalid-payload` → 400 (malformed body)
//! - `progression-incorrect-user`  → 403 (cross-user; N/A — we always
//!   operate on the caller's record)
//! - `progression-locked`          → 423 (publication-level lock; not
//!   modeled by Folio yet)
//! - `progression-date`            → 409 (stale write — body.modified
//!   older than the DB record)
//!
//! Field mapping:
//! - `progression` (0.0..=1.0) ↔ `progress_record.percent`
//! - `modified` ↔ `progress_record.updated_at`
//! - `device.id` + `device.name` ↔ `progress_record.device` packed as
//!   `"{name} ({id})"` on GET; loose parse on PUT.
//! - `references` (CBZ page-image filenames) — accept on PUT, discard;
//!   Folio doesn't model per-archive page refs. Documented hold.
//! - `title` — informational, accept-and-discard on PUT; not emitted
//!   on GET (the OPDS entry already carries the title).
//!
//! This endpoint is **always active** regardless of
//! `compat.opds_panels_mode` — it's the spec-clean path and has no
//! Folio-identity tradeoffs.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, FixedOffset};
use entity::{issue, progress_record};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};

use crate::api::not_found;
use crate::auth::extractor::{CurrentUser, RequireProgressScope};
use crate::library::access;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/opds/v1/progression/{issue_id}",
        get(get_progression).put(put_progression),
    )
}

pub const MEDIA_TYPE: &str = "application/opds-progression+json";
pub const REL: &str = "http://opds-spec.org/progression";

#[derive(Debug, Serialize, Deserialize)]
struct Progression {
    modified: DateTime<FixedOffset>,
    device: Device,
    progression: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    references: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Device {
    id: String,
    name: String,
}

async fn get_progression(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(issue_id): Path<String>,
) -> Response {
    let issue_row = match issue::Entity::find_by_id(issue_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(e) => {
            tracing::warn!(error = %e, "opds_progression: issue lookup failed");
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                "progression-invalid-payload",
                "internal",
            );
        }
    };
    let visible = access::for_user(&app, &user).await;
    if !visible.contains(issue_row.library_id) {
        return not_found();
    }
    let pr = match progress_record::Entity::find_by_id((user.id, issue_id.clone()))
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(e) => {
            tracing::warn!(error = %e, "opds_progression: progress lookup failed");
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                "progression-invalid-payload",
                "internal",
            );
        }
    };
    let (device_id, device_name) = unpack_device(pr.device.as_deref());
    let body = Progression {
        modified: pr.updated_at,
        device: Device {
            id: device_id,
            name: device_name,
        },
        progression: pr.percent.clamp(0.0, 1.0),
        references: None,
        title: None,
    };
    progression_response(StatusCode::OK, &body)
}

async fn put_progression(
    State(app): State<AppState>,
    user: RequireProgressScope,
    Path(issue_id): Path<String>,
    Json(body): Json<Progression>,
) -> Response {
    if !(0.0..=1.0).contains(&body.progression) || body.progression.is_nan() {
        // OPDS-Progression 1.0 spec defines `progression-invalid-payload`
        // as 400 (per the registry URI; see the module docstring). Stays
        // 400 even though our internal convention reclassifies semantic-
        // validation failures to 422 — the wire protocol is governed by
        // the spec, not our convention.
        return problem(
            StatusCode::BAD_REQUEST,
            "progression-invalid-payload",
            "`progression` must be in [0.0, 1.0]",
        );
    }
    let issue_row = match issue::Entity::find_by_id(issue_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(e) => {
            tracing::warn!(error = %e, "opds_progression: issue lookup failed");
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                "progression-invalid-payload",
                "internal",
            );
        }
    };
    let visible = access::for_user(&app, &user.0).await;
    if !visible.contains(issue_row.library_id) {
        return not_found();
    }
    // Stale-write detection (spec error `progression-date`). If a
    // prior record exists with a `modified` newer than what the
    // client submitted, reject with 409 so the client can re-fetch
    // and merge.
    if let Ok(Some(existing)) = progress_record::Entity::find_by_id((user.0.id, issue_id.clone()))
        .one(&app.db)
        .await
        && existing.updated_at > body.modified
    {
        return problem(
            StatusCode::CONFLICT,
            "progression-date",
            "the server's progression is newer than the submitted `modified`",
        );
    }

    // Map progression (0..1) to a 0-indexed last_page using page_count.
    // upsert_for recomputes `percent` from `page / page_count`, so it
    // may round to slightly different than the input value — that's
    // by design (Folio stores last_page as the canonical signal).
    let page_count = issue_row.page_count.unwrap_or(0).max(0);
    let last_page = if page_count > 0 {
        ((body.progression * page_count as f64).floor() as i32).clamp(0, page_count - 1)
    } else {
        0
    };
    let finished = if page_count > 0 && body.progression >= 1.0 {
        Some(true)
    } else {
        None
    };
    let device_packed = pack_device(&body.device);

    if let Err(e) = crate::api::progress::upsert_for(
        &app,
        user.0.id,
        &issue_row,
        last_page,
        finished,
        Some(device_packed),
    )
    .await
    {
        tracing::warn!(error = %e, "opds_progression: upsert failed");
        return problem(
            StatusCode::INTERNAL_SERVER_ERROR,
            "progression-invalid-payload",
            "internal",
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

fn pack_device(d: &Device) -> String {
    format!("{} ({})", d.name, d.id)
}

/// Parse a packed `"{name} ({id})"` string back into (id, name).
/// Permissive: when the packed shape is missing (DB row predates this
/// endpoint or was written by another caller), returns empty strings.
/// Spec requires both fields; emitting empties is preferable to
/// returning 500 or fabricating identifiers.
fn unpack_device(packed: Option<&str>) -> (String, String) {
    let Some(s) = packed else {
        return (String::new(), String::new());
    };
    if let Some(open) = s.rfind(" (")
        && let Some(close) = s.rfind(')')
        && close > open
    {
        let name = s[..open].to_owned();
        let id = s[open + 2..close].to_owned();
        return (id, name);
    }
    // Couldn't recover structure — return the whole string as both,
    // which at least round-trips informationally.
    (s.to_owned(), s.to_owned())
}

fn progression_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let json = match serde_json::to_vec(body) {
        Ok(v) => v,
        Err(_) => {
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                "progression-invalid-payload",
                "internal serialization",
            );
        }
    };
    let mut resp = Response::builder()
        .status(status)
        .body(axum::body::Body::from(json))
        .expect("response build");
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(MEDIA_TYPE));
    resp
}

/// RFC 7807 Problem Details with typed URI from the OPDS Progression
/// 1.0 error registry. Spec defines four error tags; we use them
/// verbatim so spec-conformant clients can branch on the URI.
fn problem(status: StatusCode, tag: &str, detail: &str) -> Response {
    let body = serde_json::json!({
        "type": format!("https://registry.opds.io/error#{tag}"),
        "title": tag,
        "status": status.as_u16(),
        "detail": detail,
    });
    let bytes = serde_json::to_vec(&body).expect("problem body");
    let mut resp = Response::builder()
        .status(status)
        .body(axum::body::Body::from(bytes))
        .expect("response build");
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    resp
}
