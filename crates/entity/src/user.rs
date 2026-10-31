use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// `PartialEq` only (no `Eq`): `reader_brightness` / `reader_sepia` are
// `f64`, like `issue::Model`'s numeric columns.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    /// `oidc:<issuer>|<sub>` for OIDC users; `local:<uuid>` for local users.
    /// Globally unique, used as Basic-Auth username for OPDS app passwords.
    #[sea_orm(unique)]
    pub external_id: String,

    /// User-displayed handle.
    pub display_name: String,

    /// Lowercased; nullable for OIDC users without an email claim.
    #[sea_orm(unique, nullable)]
    pub email: Option<String>,

    pub email_verified: bool,

    /// argon2id PHC hash; NULL for OIDC-only users.
    #[sea_orm(nullable)]
    pub password_hash: Option<String>,

    /// TOTP secret (base32) once enrolled.
    #[sea_orm(nullable)]
    pub totp_secret: Option<String>,

    /// `pending_verification` | `active` | `disabled`
    pub state: String,

    /// `admin` | `user`
    pub role: String,

    /// Bumped on logout-all / password reset / admin revoke (§17.2).
    pub token_version: i64,

    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub last_login_at: Option<DateTimeWithTimeZone>,

    /// Phase 3: `'ltr' | 'rtl' | 'auto'` (or null = auto). Used as the fallback
    /// for the reader's per-series direction when no per-series localStorage
    /// entry and no `Manga=YesAndRightToLeft` flag is present.
    #[sea_orm(nullable)]
    pub default_reading_direction: Option<String>,

    /// M4: reader default fit mode — `'width' | 'height' | 'original'`. Null
    /// means "fall back to the reader's built-in default (width)".
    #[sea_orm(nullable)]
    pub default_fit_mode: Option<String>,

    /// M4: reader default view mode — `'single' | 'double' | 'webtoon'`.
    /// Null defers to per-series auto-detection.
    #[sea_orm(nullable)]
    pub default_view_mode: Option<String>,

    /// M4: when true, the reader opens with the page strip visible.
    pub default_page_strip: bool,

    /// v0.3.44 / v0.3.45: reader page-turn animation — `'off' |
    /// 'slide' | 'fade'`. Null means "use the reader's built-in
    /// default" (currently `slide`); fresh users start here.
    /// Webtoon view mode bypasses this regardless of value. `fade`
    /// added in v0.3.45.
    #[sea_orm(nullable)]
    pub default_page_animation: Option<String>,

    /// Phase-double-page: when true (default), the reader's double-page view
    /// renders the front cover solo and pairs from page 2 — matches printed
    /// comic conventions. Per-series localStorage still wins at runtime.
    pub default_cover_solo: bool,

    /// M4: theme token — `'system' | 'dark' | 'light' | 'amber'`. Null means
    /// "no preference" (the client falls back to the design-system default).
    #[sea_orm(nullable)]
    pub theme: Option<String>,

    /// M4: accent palette token — `'amber' | 'blue' | 'emerald' | 'rose'`.
    /// Null means default accent (amber).
    #[sea_orm(nullable)]
    pub accent_color: Option<String>,

    /// M4: UI density token — `'comfortable' | 'compact'`. Null means
    /// `comfortable`.
    #[sea_orm(nullable)]
    pub density: Option<String>,

    /// Corner radius token — `'sharp' | 'default' | 'round'`. Null means
    /// `default` (shadcn plan PR 6).
    #[sea_orm(nullable)]
    pub corner_radius: Option<String>,

    /// Motion token — `'system' | 'reduced'`. Null means `system`.
    #[sea_orm(nullable)]
    pub motion: Option<String>,

    /// M4: per-action key overrides for the reader. JSON object of
    /// `{ action_name: key_string }`. Empty object means "use defaults".
    pub keybinds: Json,

    /// M6a: opt-out kill switch for reading-activity capture. When false the
    /// client tracker hook short-circuits and no `reading_sessions` rows are
    /// written. Default true.
    pub activity_tracking_enabled: bool,

    /// M6a: IANA timezone string (e.g. `America/Los_Angeles`). Used by the
    /// stats endpoint to bucket sessions into local-day rows so a heatmap
    /// doesn't wrap "Monday night" into "Tuesday UTC". Default `UTC`.
    pub timezone: String,

    /// M6a: minimum accumulated active ms below which a session is discarded
    /// (server-side validation; client also self-gates before posting).
    /// Default 30000.
    pub reading_min_active_ms: i32,

    /// M6a: minimum distinct pages dwelled on below which a session is
    /// discarded. Default 3.
    pub reading_min_pages: i32,

    /// M6a: client-side idle threshold in ms; after this much inactivity the
    /// session ends. Stored on the user so the server can validate sane
    /// bounds on PATCH /me/preferences. Default 180000 (3 min).
    pub reading_idle_ms: i32,

    /// Human-URLs M3: BCP-47 language tag. Drives next-intl message bundle
    /// selection and the `NEXT_LOCALE` cookie. Default `en` matches the
    /// single supported locale today; PATCH /me/preferences validates new
    /// values against the SUPPORTED_LOCALES list.
    pub language: String,

    /// Stats v2: when true, the admin dashboard excludes this user from
    /// system-wide aggregates (top series, reads-per-day, DAU/WAU/MAU, content
    /// insights). Default false. Privacy toggle on /settings/activity.
    pub exclude_from_aggregates: bool,

    /// Markers M8: when true, the Bookmarks sidebar row renders a count
    /// badge sourced from `GET /me/markers/count`. Default false so new
    /// accounts get a quiet sidebar; toggled from /settings/account.
    pub show_marker_count: bool,

    /// OPDS sync cleanup M2: when true, `/opds/v1/wtr` + v2 mirror
    /// reorder the user's next-up issue to position 0. When false,
    /// WTR renders in the user-curated drag order. Default true —
    /// the reorder behavior is the right answer for the common case.
    pub opds_wtr_reorder: bool,

    /// OPDS sync cleanup M3: when true, OPDS entry titles carry a
    /// progress glyph (◯ / ◐ / ●) plus `(N / M)` page-count suffix
    /// so clients that ignore PSE attributes still surface "what's
    /// left". Default true.
    pub opds_progress_glyphs: bool,

    /// Per-user override of the home-page rail cap. Defaults to 12;
    /// the DB CHECK + PATCH validation both pin the range to
    /// 1..=50. Replaces the previous hard-coded `MAX_PIN_COUNT`
    /// constant in `saved_views.rs::pin`. The client-side rail
    /// surface lazy-mounts off-screen rails so a higher value does
    /// not load every rail at hydration.
    pub max_rails_per_page: i32,

    /// Reader display preferences that follow the account (audit 2026-11
    /// F6/F7). Account-wide like `markersHidden` in the web store: they are
    /// about the viewer's screen, not a series. NULL means the account has
    /// never set the value, so a browser's existing localStorage value
    /// stands (and is adopted on the first change). Ranges are pinned by
    /// DB CHECK constraints and the PATCH handler.
    ///
    /// CSS `brightness()` multiplier, 0.5..=1.5.
    #[sea_orm(nullable)]
    pub reader_brightness: Option<f64>,
    /// CSS `sepia()` amount, 0..=1.
    #[sea_orm(nullable)]
    pub reader_sepia: Option<f64>,
    /// CSS `grayscale()` percent, 0..=100.
    #[sea_orm(nullable)]
    pub reader_grayscale: Option<i32>,
    /// Night mode: `invert(1)` over the page surface.
    #[sea_orm(nullable)]
    pub reader_invert: Option<bool>,
    /// Colour temperature, -50 (cool) ..= 50 (warm); 0 = neutral.
    #[sea_orm(nullable)]
    pub reader_warmth: Option<i32>,
    /// CSS `contrast()` percent, 50..=150.
    #[sea_orm(nullable)]
    pub reader_contrast: Option<i32>,
    /// "Keep zoom between pages" (WP-4.2).
    #[sea_orm(nullable)]
    pub reader_zoom_persist: Option<bool>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
