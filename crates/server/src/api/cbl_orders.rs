//! Linked reading lists 2.0: import a whole reading order from the
//! catalog in one go.
//!
//! Several sources publish one order as many files: CMRO cuts each depth
//! into 1,000-issue parts (Core is 25 files), Marvel Guides publishes an
//! era as dozens of named sub-chapters, CBRO numbers its master parts.
//! Importing them one at a time left the reader to import each file and
//! link each part to the next by hand.
//!
//! `POST /me/cbl-lists/import-order` takes any one file of an order
//! ([`crate::cbl::sources`] says which files belong to it), imports the
//! parts the caller doesn't have yet, in reading order, and chains every
//! adjacent pair (`continues`, `placement = auto_chain`).
//!
//! Each import matches its entries inline, so one call stops after
//! [`IMPORT_BUDGET`] and reports how many parts are `remaining`; the
//! client calls again until it is zero (imported parts are reused, so a
//! repeat only does what is left). One call spends one hourly import
//! token; each new list still counts against the per-user list cap.

use std::collections::HashMap;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{catalog_source, cbl_list};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use server_macros::handler;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use super::cbl_derive::{ApplyFailure, catalog_failure, link_part};
use super::cbl_lists::{SourceMeta, insert_list_from_parsed};
use super::error;
use crate::auth::CurrentUser;
use crate::cbl::{catalog, sources};
use crate::state::AppState;

/// Imports one call starts before it stops and reports the rest (the JSON
/// route times out at 60 s; an import of a 1,000-issue part takes a few
/// seconds).
const IMPORT_BUDGET: std::time::Duration = std::time::Duration::from_secs(35);
/// The index refresh and each download share this.
const CATALOG_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);
const _: () = assert!(
    IMPORT_BUDGET.as_secs() + CATALOG_BUDGET.as_secs() + 5
        < crate::app::JSON_ROUTE_TIMEOUT.as_secs()
);

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(import_order))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ImportOrderReq {
    pub catalog_source_id: Uuid,
    /// Any one file of the order.
    pub catalog_path: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ImportOrderResp {
    /// The order's name: its files' shared prefix.
    pub order_name: String,
    /// Files in the order.
    pub total_parts: i32,
    /// Lists this call imported.
    pub imported: i32,
    /// `continues` links this call made between adjacent parts.
    pub linked: i32,
    /// Parts still to import. Call again while this is above zero and
    /// `imported` was (nothing imported and nothing failed means the
    /// budget ran out before the first one: also worth one more call).
    pub remaining: i32,
    pub failed: Vec<ApplyFailure>,
    /// The caller's lists for the order's parts, in reading order.
    pub list_ids: Vec<String>,
    /// The reading-lists group (named after the order) the parts were
    /// filed into, when this call imported or chained any. A part the
    /// caller already keeps in a group of their own stays there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

fn internal() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

#[utoipa::path(
    operation_id = "cbl_lists_import_order",
    post,
    path = "/me/cbl-lists/import-order",
    request_body = ImportOrderReq,
    responses(
        (status = 200, body = ImportOrderResp),
        (status = 404, description = "catalog source not found"),
        (status = 422, description = "the file is not one part of an order"),
        (status = 429, description = "import limit reached"),
        (status = 502, description = "the catalog could not be read"),
    )
)]
#[handler]
pub async fn import_order(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<ImportOrderReq>,
) -> Response {
    let source = match catalog_source::Entity::find_by_id(req.catalog_source_id)
        .one(&app.db)
        .await
    {
        Ok(Some(s)) if s.enabled => s,
        Ok(_) => return error(StatusCode::NOT_FOUND, "not_found", "catalog source"),
        Err(_) => return internal(),
    };
    let index = match tokio::time::timeout(
        CATALOG_BUDGET,
        catalog::refresh_index(&app.db, &source, false),
    )
    .await
    {
        Ok(Ok(i)) => i,
        Ok(Err(e)) => return catalog_failure(&e),
        Err(_) => {
            return error(
                StatusCode::BAD_GATEWAY,
                "fetch_failed",
                "the catalog took too long to answer",
            );
        }
    };
    let index_paths: Vec<&str> = index.entries.iter().map(|e| e.path.as_str()).collect();
    let parts: Vec<String> = sources::order_parts(&req.catalog_path, &index_paths)
        .into_iter()
        .map(|(_, p)| p.to_owned())
        .collect();
    if parts.is_empty() {
        return super::cbl_links::field_error(
            "catalog_path",
            "this list is not one part of a reading order with several parts",
        );
    }
    let order_name = sources::order_name(&req.catalog_path)
        .unwrap_or_else(|| sources::file_stem(&req.catalog_path).to_owned());

    // The caller's own lists for these files (a shared copy is someone
    // else's to chain).
    let mut have: HashMap<String, cbl_list::Model> = match cbl_list::Entity::find()
        .filter(cbl_list::Column::CatalogSourceId.eq(source.id))
        .filter(cbl_list::Column::CatalogPath.is_in(parts.clone()))
        .filter(cbl_list::Column::OwnerUserId.eq(user.id))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|l| l.catalog_path.clone().map(|p| (p, l)))
            .collect(),
        Err(_) => return internal(),
    };

    let mut failed: Vec<ApplyFailure> = Vec::new();
    let mut imported = 0i32;
    let missing: Vec<&String> = parts.iter().filter(|p| !have.contains_key(*p)).collect();
    if !missing.is_empty() {
        if let Err(resp) = crate::cbl::quota::consume_import(&app, &user).await {
            return resp;
        }
        let entry_of: HashMap<&str, &catalog::CatalogEntry> =
            index.entries.iter().map(|e| (e.path.as_str(), e)).collect();
        let deadline = tokio::time::Instant::now() + IMPORT_BUDGET;
        for path in missing {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let name = sources::file_stem(path).to_owned();
            let mut fail = |message: &str| {
                failed.push(ApplyFailure {
                    name: name.clone(),
                    message: message.to_owned(),
                });
            };
            if crate::cbl::quota::check_remote_list_cap(&app, &user)
                .await
                .is_err()
            {
                // Every later part would hit the same wall.
                fail("you've reached your reading-list limit");
                break;
            }
            let Some(entry) = entry_of.get(path.as_str()) else {
                fail("not in the catalog");
                continue;
            };
            let blob =
                match tokio::time::timeout(CATALOG_BUDGET, catalog::download_blob(&source, entry))
                    .await
                {
                    Ok(Ok(b)) => b,
                    Ok(Err(e)) => {
                        fail(&e.public_message());
                        continue;
                    }
                    Err(_) => {
                        fail("the catalog took too long to answer");
                        continue;
                    }
                };
            let xml = String::from_utf8_lossy(&blob.bytes).into_owned();
            let parsed = match parsers::cbl::parse(xml.as_bytes()) {
                Ok(p) if p.books.len() <= crate::cbl::MAX_BOOKS_PER_LIST => p,
                Ok(_) => {
                    fail(&crate::cbl::too_many_entries_message());
                    continue;
                }
                Err(e) => {
                    fail(&e.to_string());
                    continue;
                }
            };
            let list = match insert_list_from_parsed(
                &app,
                Some(user.id),
                &parsed,
                &xml,
                SourceMeta::Catalog {
                    catalog_source_id: source.id,
                    catalog_path: path.clone(),
                    blob_sha: blob.blob_sha,
                    description: None,
                    refresh_schedule: None,
                },
            )
            .await
            {
                Ok(l) => l,
                Err(_) => {
                    fail("import failed");
                    continue;
                }
            };
            if let Err(e) = super::saved_views::ensure_cbl_view(&app.db, user.id, &list).await {
                tracing::warn!(error = %e, "cbl orders: couldn't create the part's view");
            }
            imported += 1;
            have.insert(path.clone(), list);
        }
    }

    // Chain every adjacent pair the caller now has. A gap (a part that
    // failed) leaves its neighbours unchained until it is imported.
    let mut linked = 0i32;
    for pair in parts.windows(2) {
        let (Some(from), Some(to)) = (have.get(&pair[0]), have.get(&pair[1])) else {
            continue;
        };
        match link_part(&app, from, to).await {
            Ok(true) => linked += 1,
            Ok(false) => {}
            Err(message) => failed.push(ApplyFailure {
                name: to.parsed_name.clone(),
                message,
            }),
        }
    }

    // One group for the order, so 25 parts don't flood the lists page.
    // Only when this call imported or chained something: a reader who
    // took the group apart keeps their arrangement.
    let mut group_id = None;
    if imported > 0 || linked > 0 {
        let mut views = Vec::with_capacity(parts.len());
        for path in &parts {
            let Some(list) = have.get(path) else { continue };
            match super::saved_views::ensure_cbl_view(&app.db, user.id, list).await {
                Ok((view, _)) => views.push(view),
                Err(e) => {
                    tracing::warn!(error = %e, "cbl orders: couldn't find the part's view");
                }
            }
        }
        match super::view_groups::file_into_group(
            &app,
            user.id,
            super::view_groups::SCOPE_CBL,
            &order_name,
            &views,
        )
        .await
        {
            Ok(g) => group_id = g.map(|g| g.to_string()),
            Err(e) => tracing::warn!(error = %e, "cbl orders: couldn't group the parts"),
        }
    }

    let remaining = parts.iter().filter(|p| !have.contains_key(*p)).count();
    Json(ImportOrderResp {
        order_name,
        total_parts: i32::try_from(parts.len()).unwrap_or(i32::MAX),
        imported,
        linked,
        remaining: i32::try_from(remaining).unwrap_or(i32::MAX),
        failed,
        list_ids: parts
            .iter()
            .filter_map(|p| have.get(p).map(|l| l.id.to_string()))
            .collect(),
        group_id,
    })
    .into_response()
}
