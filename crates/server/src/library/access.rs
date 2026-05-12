//! Shared library-visibility helpers.
//!
//! `VisibleLibraries` collapses an admin / per-user-ACL distinction into
//! a single value: `unrestricted = true` (admin) or an explicit set of
//! library IDs (non-admin). API handlers use it to scope list endpoints
//! and the saved-views compiler uses it as the first WHERE predicate.

use crate::auth::extractor::CurrentUser;
use crate::state::AppState;
use entity::library_user_access;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct VisibleLibraries {
    /// Admin / unrestricted access.
    pub unrestricted: bool,
    /// Library IDs the user has explicit access to (only used when not
    /// unrestricted).
    pub allowed: HashSet<Uuid>,
}

impl VisibleLibraries {
    pub fn unrestricted() -> Self {
        Self {
            unrestricted: true,
            allowed: HashSet::new(),
        }
    }

    /// True iff the user can see series in this library.
    pub fn contains(&self, library_id: Uuid) -> bool {
        self.unrestricted || self.allowed.contains(&library_id)
    }
}

pub async fn for_user(app: &AppState, user: &CurrentUser) -> VisibleLibraries {
    if user.role == "admin" {
        return VisibleLibraries::unrestricted();
    }
    let granted = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    VisibleLibraries {
        unrestricted: false,
        allowed: granted.into_iter().map(|g| g.library_id).collect(),
    }
}
