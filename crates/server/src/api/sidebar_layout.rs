//! Per-user sidebar layout API (navigation customization M1).
//!
//! The sidebar today is a hardcoded three-section list in
//! `web/components/library/main-nav.ts` (Browse built-ins → Libraries →
//! Saved views). This module is the read+write surface that lets a user
//! reorder and hide any row across all three groups from one settings
//! page.
//!
//! ## Storage model
//!
//! `user_sidebar_entries` stores **explicit overrides only**. A user
//! with zero rows here gets the same default sidebar as today. Each
//! override row is keyed by `(user_id, kind, ref_id)`:
//!
//!   - `kind = 'builtin'` → `ref_id` is a [`BUILTIN_REGISTRY`] key
//!     (e.g. `home`, `bookmarks`, `collections`, `want_to_read`,
//!     `all_libraries`).
//!   - `kind = 'library'` → `ref_id` is `libraries.id` (UUID as text).
//!   - `kind = 'view'`    → `ref_id` is `saved_views.id`.
//!
//! When `compute_layout` builds the response it:
//!
//!   1. Materializes the **default layout** for this user (built-ins +
//!      libraries the user can see + saved views flagged
//!      `show_in_sidebar`), each with a default position derived from
//!      its place in the list.
//!   2. Merges override rows on top — visibility and position win when
//!      present; items not mentioned in overrides keep their default
//!      position.
//!   3. Drops items whose underlying resource has disappeared (deleted
//!      library, revoked access, deleted view) so a stale override
//!      can't produce a ghost row.
//!
//! The PATCH endpoint accepts a complete layout: every entry the client
//! wants tracked, with its desired visibility and position. The server
//! replaces the user's override rows wholesale. Items not present in
//! the payload revert to "no override" → default visibility.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use entity::{library, saved_view, user_page, user_sidebar_entry, user_view_pin};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;

pub const KIND_BUILTIN: &str = "builtin";
pub const KIND_LIBRARY: &str = "library";
pub const KIND_VIEW: &str = "view";
/// Multi-page rails M4 — a user-created page (`user_page.id`).
/// `ref_id` carries the page id; the entry surfaces in the sidebar
/// between the Home built-in and Bookmarks.
pub const KIND_PAGE: &str = "page";
/// Custom section title row. `ref_id` is a client-generated UUID
/// (composite-PK uniqueness); `label` carries the displayed text.
pub const KIND_HEADER: &str = "header";
/// Visual gap row — no label, no link. `ref_id` is a client-generated
/// UUID for PK uniqueness.
pub const KIND_SPACER: &str = "spacer";

/// Built-in sidebar entries, in their default top-down order. The
/// client maps each `key` to an icon via the same registry used by
/// `main-nav.ts`. The `href` here is the locale-neutral path; the
/// client prepends its locale prefix at render time.
///
/// "All Libraries" is intentionally *not* here — see [`ALL_LIBRARIES_REF_ID`].
pub const BUILTIN_REGISTRY: &[BuiltinDef] = &[
    BuiltinDef {
        key: "home",
        label: "Home",
        icon: "Home",
        href: "/",
    },
    BuiltinDef {
        key: "bookmarks",
        label: "Bookmarks",
        icon: "Bookmark",
        href: "/bookmarks",
    },
    BuiltinDef {
        key: "collections",
        label: "Collections",
        icon: "Folder",
        href: "/collections",
    },
    BuiltinDef {
        key: "want_to_read",
        label: "Want to Read",
        icon: "ListPlus",
        href: "/views/want-to-read",
    },
];

/// "All Libraries" is a synthetic `kind = 'library'` entry so the client
/// groups it visually with the per-library rows instead of in the Browse
/// section. `ref_id = "all"` is the sentinel — never a real library
/// UUID, so it can't collide with one. Kept as a constant so the M3
/// drag-and-drop UI on `/settings/navigation` can persist overrides
/// against the same key the server emits.
pub const ALL_LIBRARIES_REF_ID: &str = "all";

pub struct BuiltinDef {
    pub key: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub href: &'static str,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/me/sidebar-layout", get(get_layout).patch(update_layout))
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SidebarLayoutView {
    pub entries: Vec<SidebarEntryView>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SidebarEntryView {
    pub kind: String,
    pub ref_id: String,
    pub label: String,
    pub icon: String,
    pub href: String,
    pub visible: bool,
    pub position: i32,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdateLayoutReq {
    pub entries: Vec<UpdateEntryReq>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdateEntryReq {
    pub kind: String,
    pub ref_id: String,
    pub visible: bool,
    pub position: i32,
    /// Optional label override (required for `kind='header'`; ignored
    /// for `kind='spacer'`; optional for other kinds).
    #[serde(default)]
    pub label: Option<String>,
}

#[utoipa::path(
    get,
    path = "/me/sidebar-layout",
    responses(
        (status = 200, body = SidebarLayoutView),
    )
)]
pub async fn get_layout(State(app): State<AppState>, user: CurrentUser) -> Response {
    match compute_layout(&app, &user).await {
        Ok(entries) => Json(SidebarLayoutView { entries }).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "compute_layout failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[utoipa::path(
    patch,
    path = "/me/sidebar-layout",
    request_body = UpdateLayoutReq,
    responses(
        (status = 200, body = SidebarLayoutView),
        (status = 400, description = "invalid kind or duplicate ref"),
    )
)]
pub async fn update_layout(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<UpdateLayoutReq>,
) -> Response {
    // Validate the payload before opening a transaction. `seen` catches
    // duplicate (kind, ref_id) pairs that would otherwise produce
    // conflicting override rows.
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for e in &req.entries {
        match e.kind.as_str() {
            KIND_BUILTIN | KIND_LIBRARY | KIND_VIEW | KIND_PAGE | KIND_HEADER | KIND_SPACER => {}
            other => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    &format!("unknown sidebar entry kind '{other}'"),
                );
            }
        }
        // Header rows must carry a non-empty label; otherwise the
        // sidebar would render a mute row with no affordance.
        if e.kind == KIND_HEADER && e.label.as_deref().map(str::trim).unwrap_or("").is_empty() {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "header entries require a non-empty label",
            );
        }
        if !seen.insert((e.kind.clone(), e.ref_id.clone())) {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                &format!("duplicate {} entry for ref_id={}", e.kind, e.ref_id),
            );
        }
    }

    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "begin txn failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Whole-array replace: drop the user's current overrides, then
    // re-insert from the payload. Cheaper than diffing and avoids the
    // PK-collision dance that a row-by-row UPSERT would need.
    if let Err(e) = user_sidebar_entry::Entity::delete_many()
        .filter(user_sidebar_entry::Column::UserId.eq(user.id))
        .exec(&txn)
        .await
    {
        tracing::error!(error = %e, "delete overrides failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    for e in req.entries {
        let label = e
            .label
            .as_ref()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let am = user_sidebar_entry::ActiveModel {
            user_id: Set(user.id),
            kind: Set(e.kind),
            ref_id: Set(e.ref_id),
            visible: Set(e.visible),
            position: Set(e.position),
            label: Set(label),
        };
        if let Err(err) = am.insert(&txn).await {
            tracing::error!(error = %err, "insert override failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }

    if let Err(e) = txn.commit().await {
        tracing::error!(error = %e, "commit overrides failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    match compute_layout(&app, &user).await {
        Ok(entries) => Json(SidebarLayoutView { entries }).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "compute_layout after write failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

/// Resolves the user's effective sidebar: default-built list ∪
/// override rows, with overrides winning on visibility + position.
pub async fn compute_layout(
    app: &AppState,
    user: &CurrentUser,
) -> Result<Vec<SidebarEntryView>, sea_orm::DbErr> {
    // 1. Pull the override rows once, indexed by (kind, ref_id) so the
    //    later default-list walk can apply them in O(1).
    let overrides: Vec<user_sidebar_entry::Model> = user_sidebar_entry::Entity::find()
        .filter(user_sidebar_entry::Column::UserId.eq(user.id))
        .all(&app.db)
        .await?;
    let mut override_by_key: HashMap<(String, String), user_sidebar_entry::Model> = HashMap::new();
    for o in overrides {
        override_by_key.insert((o.kind.clone(), o.ref_id.clone()), o);
    }

    // Multi-page rails M4: pull every user_page row in one query. The
    // system row's `name` overrides the Home built-in's hardcoded label
    // (so renaming Home → "Library" sticks in the sidebar); custom rows
    // produce kind='page' sidebar entries interleaved after Home.
    let pages: Vec<user_page::Model> = user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user.id))
        .order_by_asc(user_page::Column::Position)
        .order_by_asc(user_page::Column::CreatedAt)
        .all(&app.db)
        .await?;
    let home_label: Option<String> = pages.iter().find(|p| p.is_system).map(|p| p.name.clone());
    let custom_pages: Vec<&user_page::Model> = pages.iter().filter(|p| !p.is_system).collect();

    let mut defaults: Vec<SidebarEntryView> = Vec::new();

    // 2. Built-ins, with a leading "Browse" section header. Each
    //    default header carries a stable `ref_id` of the form
    //    `default:<section>` so user overrides can hide/move/rename
    //    them through the regular override path. Custom headers
    //    inserted by the user use kind='header' with client-generated
    //    UUIDs as ref_id and never collide with these defaults.
    defaults.push(default_header("default:browse", "Browse"));
    for b in BUILTIN_REGISTRY.iter() {
        let label = if b.key == "home" {
            home_label.clone().unwrap_or_else(|| b.label.to_string())
        } else {
            b.label.to_string()
        };
        defaults.push(SidebarEntryView {
            kind: KIND_BUILTIN.to_string(),
            ref_id: b.key.to_string(),
            label,
            icon: b.icon.to_string(),
            href: b.href.to_string(),
            visible: true,
            position: 0, // overwritten in the position-assignment pass
        });
    }

    // 3. "All Libraries" synthetic entry, then the libraries the user
    //    can see (alphabetical, matching the current hardcoded main-nav
    //    ordering). Both share `kind = 'library'` so the client groups
    //    them in one section even though one is virtual.
    defaults.push(default_header("default:libraries", "Libraries"));
    defaults.push(SidebarEntryView {
        kind: KIND_LIBRARY.to_string(),
        ref_id: ALL_LIBRARIES_REF_ID.to_string(),
        label: "All Libraries".to_string(),
        icon: "Library".to_string(),
        href: "/?library=all".to_string(),
        visible: true,
        position: 0,
    });
    let visible_libs = access::for_user(app, user).await;
    let libs: Vec<library::Model> = library::Entity::find()
        .order_by_asc(library::Column::Name)
        .all(&app.db)
        .await?;
    for lib in libs {
        if !visible_libs.contains(lib.id) {
            continue;
        }
        defaults.push(SidebarEntryView {
            kind: KIND_LIBRARY.to_string(),
            ref_id: lib.id.to_string(),
            label: lib.name,
            icon: "Library".to_string(),
            href: format!("/?library={}", lib.id),
            visible: true,
            position: 0,
        });
    }

    // 3b. User-created pages follow the libraries by default. The user
    //     can drag them anywhere afterwards; the default position keeps
    //     pages out of the Browse section so the curated built-ins +
    //     library list aren't disrupted by every new page.
    if !custom_pages.is_empty() {
        defaults.push(default_header("default:pages", "Pages"));
        for page in &custom_pages {
            defaults.push(SidebarEntryView {
                kind: KIND_PAGE.to_string(),
                ref_id: page.id.to_string(),
                label: page.name.clone(),
                icon: "LayoutGrid".to_string(),
                href: format!("/pages/{}", page.slug),
                visible: true,
                position: 0,
            });
        }
    }

    // 4. Saved views the user has flagged `show_in_sidebar = true`
    //    (legacy column — a follow-on milestone retires it once
    //    `user_sidebar_entries` is the only reader). Pulled in
    //    pin-position order so the sidebar mirrors the user's curated
    //    rail order until they customize further.
    let pin_rows: Vec<user_view_pin::Model> = user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user.id))
        .filter(user_view_pin::Column::ShowInSidebar.eq(true))
        .order_by_asc(user_view_pin::Column::Position)
        .all(&app.db)
        .await?;
    if !pin_rows.is_empty() {
        defaults.push(default_header("default:views", "Saved views"));
        let view_ids: Vec<Uuid> = pin_rows.iter().map(|p| p.view_id).collect();
        let view_rows: Vec<saved_view::Model> = saved_view::Entity::find()
            .filter(saved_view::Column::Id.is_in(view_ids))
            .all(&app.db)
            .await?;
        let view_by_id: HashMap<Uuid, saved_view::Model> =
            view_rows.into_iter().map(|v| (v.id, v)).collect();
        for p in pin_rows {
            let Some(v) = view_by_id.get(&p.view_id) else {
                continue;
            };
            defaults.push(SidebarEntryView {
                kind: KIND_VIEW.to_string(),
                ref_id: v.id.to_string(),
                label: v.name.clone(),
                icon: p.icon.clone().unwrap_or_else(|| view_default_icon(v)),
                href: view_href(v),
                visible: true,
                position: 0,
            });
        }
    }

    // 5. Assign default positions (index in the default list) and apply
    //    overrides. Overrides win on (visible, position, label). Items
    //    not in the override map keep their default values. After this
    //    pass, every (kind, ref_id) override that *also* matched a
    //    default has been consumed; what's left in `override_by_key`
    //    is the user's purely-additive rows (custom headers, spacers,
    //    or overrides for resources that have since disappeared).
    let max_default_position = defaults.len() as i32;
    for (idx, entry) in defaults.iter_mut().enumerate() {
        entry.position = idx as i32;
        if let Some(o) = override_by_key.remove(&(entry.kind.clone(), entry.ref_id.clone())) {
            entry.visible = o.visible;
            entry.position = o.position;
            if let Some(label) = o.label
                && !label.is_empty()
            {
                entry.label = label;
            }
        }
    }

    // 6. Surface custom rows that have no default counterpart — user-
    //    inserted headers, spacers, and label-overrides for resources
    //    we no longer know about (which would be filtered out anyway
    //    by their kind handler if reintroduced). Headers + spacers
    //    always emit; orphans of other kinds are dropped so the layout
    //    stays consistent with the rest of the app.
    let mut leftover_position = max_default_position;
    let mut extras: Vec<user_sidebar_entry::Model> = override_by_key.into_values().collect();
    extras.sort_by_key(|o| (o.position, o.kind.clone(), o.ref_id.clone()));
    for o in extras {
        match o.kind.as_str() {
            KIND_HEADER => {
                defaults.push(SidebarEntryView {
                    kind: KIND_HEADER.to_string(),
                    ref_id: o.ref_id,
                    label: o.label.unwrap_or_default(),
                    icon: String::new(),
                    href: String::new(),
                    visible: o.visible,
                    position: o.position,
                });
            }
            KIND_SPACER => {
                defaults.push(SidebarEntryView {
                    kind: KIND_SPACER.to_string(),
                    ref_id: o.ref_id,
                    label: String::new(),
                    icon: String::new(),
                    href: String::new(),
                    visible: o.visible,
                    position: o.position,
                });
            }
            _ => {
                // Orphan override (resource disappeared) — drop it.
            }
        }
        leftover_position += 1;
    }
    let _ = leftover_position; // sentinel for future re-ordering logic

    // 7. Stable sort by effective position. Two items with the same
    //    explicit override position keep their default-list relative
    //    order — predictable for the client.
    defaults.sort_by_key(|e| e.position);

    Ok(defaults)
}

fn default_header(ref_id: &str, label: &str) -> SidebarEntryView {
    SidebarEntryView {
        kind: KIND_HEADER.to_string(),
        ref_id: ref_id.to_string(),
        label: label.to_string(),
        icon: String::new(),
        href: String::new(),
        visible: true,
        position: 0,
    }
}

fn view_default_icon(v: &saved_view::Model) -> String {
    match v.kind.as_str() {
        "system" => "sparkles",
        "filter_series" => "filter",
        "cbl" => "list-ordered",
        "collection" => "Folder",
        _ => "filter",
    }
    .to_string()
}

fn view_href(v: &saved_view::Model) -> String {
    if v.kind == "system"
        && let Some(key) = v.system_key.as_ref()
    {
        return format!("/views/{}", key.replace('_', "-"));
    }
    format!("/views/{}", v.id)
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
