//! The "an instance always keeps an active admin" invariant.
//!
//! Every path that can take a user out of the active-admin set — an admin
//! deleting their own account (`DELETE /me`), an admin demoting or
//! deactivating an admin (`PATCH /admin/users/{id}`,
//! `POST /admin/users/{id}/disable`) — decides with [`LockedAdmins`], inside
//! the same transaction as its write:
//!
//! 1. [`LockedAdmins::lock`] reads every `role = 'admin'` row `FOR UPDATE`,
//!    in id order. A second caller on any of these paths blocks on the
//!    first lock until the first transaction ends, then re-reads the rows
//!    (READ COMMITTED re-checks the `WHERE` against the committed version),
//!    so a row the first caller just demoted is no longer in its set.
//! 2. The caller asks [`LockedAdmins::is_last_active_admin`] before writing
//!    and answers `409 conflict` when the change would leave no active
//!    admin.
//!
//! Locking in one order (admins by id, then any non-admin target row) on
//! every path keeps two of these transactions from deadlocking. The admin
//! user handlers take the admin lock for *every* change they make,
//! promotions and enables included (they also re-check the actor under it),
//! so a new path that locks `users` rows must take [`LockedAdmins::lock`]
//! first too.
//!
//! Paths that only ever add an admin outside those handlers (first-user
//! bootstrap under its advisory lock, email verification of a pending
//! user) can't break the invariant and take no admin lock.

use entity::user::{self, UserRole, UserState};
use sea_orm::{
    ColumnTrait, DatabaseTransaction, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

/// Every admin row, locked `FOR UPDATE` for the rest of the transaction.
pub struct LockedAdmins {
    rows: Vec<user::Model>,
}

impl LockedAdmins {
    /// Lock every admin row in id order. Takes a transaction on purpose: a
    /// `FOR UPDATE` outside one releases its locks as soon as it returns.
    pub async fn lock(txn: &DatabaseTransaction) -> Result<Self, DbErr> {
        let rows = user::Entity::find()
            .filter(user::Column::Role.eq(UserRole::Admin))
            .order_by_asc(user::Column::Id)
            .lock_exclusive()
            .all(txn)
            .await?;
        Ok(Self { rows })
    }

    /// `id`'s row when it is (still) an admin.
    pub fn get(&self, id: Uuid) -> Option<&user::Model> {
        self.rows.iter().find(|a| a.id == id)
    }

    /// Whether `id` is (still) an admin.
    pub fn contains(&self, id: Uuid) -> bool {
        self.get(id).is_some()
    }

    /// Whether `id` is an admin and no *other* admin is active: taking `id`
    /// out of the active-admin set (deletion, demotion, a non-active state)
    /// would leave the instance without one.
    pub fn is_last_active_admin(&self, id: Uuid) -> bool {
        self.contains(id)
            && !self
                .rows
                .iter()
                .any(|a| a.id != id && a.state == UserState::Active)
    }
}

/// Whether moving a user from `from` to `to` (role, state) takes it out of
/// the active-admin set — an active admin that becomes a non-admin, or
/// leaves the `active` state, or both.
pub fn leaves_active_admins(from: (UserRole, UserState), to: (UserRole, UserState)) -> bool {
    let active_admin = |(role, state): (UserRole, UserState)| {
        role == UserRole::Admin && state == UserState::Active
    };
    active_admin(from) && !active_admin(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_active_admin_leaving_counts() {
        use UserRole::{Admin, User};
        use UserState::{Active, Disabled, PendingVerification};
        assert!(leaves_active_admins((Admin, Active), (User, Active)));
        assert!(leaves_active_admins((Admin, Active), (Admin, Disabled)));
        assert!(leaves_active_admins(
            (Admin, Active),
            (Admin, PendingVerification)
        ));
        assert!(leaves_active_admins((Admin, Active), (User, Disabled)));
        assert!(!leaves_active_admins((Admin, Active), (Admin, Active)));
        assert!(!leaves_active_admins((Admin, Disabled), (User, Disabled)));
        assert!(!leaves_active_admins((User, Active), (User, Disabled)));
        assert!(!leaves_active_admins((User, Active), (Admin, Active)));
    }
}
