//! Typed enums for the seven reader/UI preference tokens that used to be
//! stored as bare `Option<String>` on `MeResp` / `PreferencesReq`.
//!
//! The DB column type is still `TEXT` (matched against [`Self::FromStr`] /
//! [`Self::as_str`] at the DTO boundary). Codegen emits each variant as a
//! string literal in the OpenAPI spec, so `theme === "amber"` narrows on
//! the frontend the way readers expect.
//!
//! **Adding a variant:** add it to the enum, extend `FromStr` + `as_str`,
//! re-run `just openapi`. Existing DB rows storing the new value will
//! deserialize correctly. Rows that stored a value never in the enum
//! (which shouldn't exist because the server validates on write) are
//! silently dropped to `None` at projection time — see
//! [`opt_from_db`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// `ltr` | `rtl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReadingDirection {
    Ltr,
    Rtl,
}

/// `width` | `height` | `original` | `contain`. `contain` fits the whole
/// page on screen (both axes) — the reader's "Fit screen" mode (WP-4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FitMode {
    Width,
    Height,
    Original,
    Contain,
}

/// `single` | `double` | `webtoon`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    Single,
    Double,
    Webtoon,
}

/// `off` | `slide` | `fade`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PageAnimation {
    Off,
    Slide,
    Fade,
}

/// `system` | `dark` | `light` | `amber`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Dark,
    Light,
    Amber,
}

/// `amber` | `blue` | `emerald` | `rose`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccentColor {
    Amber,
    Blue,
    Emerald,
    Rose,
}

/// `comfortable` | `compact`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Comfortable,
    Compact,
}

/// `sharp` | `default` | `round` — the `--radius` token every corner in
/// the UI derives from (shadcn plan PR 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CornerRadius {
    Sharp,
    Default,
    Round,
}

/// `system` | `reduced` — `reduced` applies the reduced-motion rules
/// regardless of the OS setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Motion {
    System,
    Reduced,
}

// Shared `FromStr` impl shape — `serde_plain` would do this for us, but we
// avoid the extra dep. Manual `FromStr` keeps the parser tied to the
// canonical wire form one place.

macro_rules! str_enum {
    ($enum:ident, $err:literal, $( $variant:ident => $wire:literal ),+ $(,)?) => {
        impl $enum {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $wire, )+
                }
            }
        }

        impl fmt::Display for $enum {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $enum {
            type Err = &'static str;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $( $wire => Ok(Self::$variant), )+
                    _ => Err($err),
                }
            }
        }
    };
}

str_enum!(
    ReadingDirection,
    "reading_direction must be 'ltr' or 'rtl'",
    Ltr => "ltr",
    Rtl => "rtl",
);
str_enum!(
    FitMode,
    "fit_mode must be 'width', 'height', 'original', or 'contain'",
    Width => "width",
    Height => "height",
    Original => "original",
    Contain => "contain",
);
str_enum!(
    ViewMode,
    "view_mode must be 'single', 'double', or 'webtoon'",
    Single => "single",
    Double => "double",
    Webtoon => "webtoon",
);
str_enum!(
    PageAnimation,
    "page_animation must be 'off', 'slide', or 'fade'",
    Off => "off",
    Slide => "slide",
    Fade => "fade",
);
str_enum!(
    Theme,
    "theme must be 'system', 'dark', 'light', or 'amber'",
    System => "system",
    Dark => "dark",
    Light => "light",
    Amber => "amber",
);
str_enum!(
    AccentColor,
    "accent_color must be 'amber', 'blue', 'emerald', or 'rose'",
    Amber => "amber",
    Blue => "blue",
    Emerald => "emerald",
    Rose => "rose",
);
str_enum!(
    Density,
    "density must be 'comfortable' or 'compact'",
    Comfortable => "comfortable",
    Compact => "compact",
);
str_enum!(
    CornerRadius,
    "corner_radius must be 'sharp', 'default', or 'round'",
    Sharp => "sharp",
    Default => "default",
    Round => "round",
);
str_enum!(
    Motion,
    "motion must be 'system' or 'reduced'",
    System => "system",
    Reduced => "reduced",
);

/// Convenience: project an `Option<String>` from the entity layer to an
/// `Option<TypedEnum>` for response DTOs. Invalid wire strings parse to
/// `None` — the server validates on write, so a DB row carrying an
/// out-of-range value is treated as "unset" rather than panicking.
pub fn opt_from_db<E: FromStr>(value: Option<&str>) -> Option<E> {
    value.and_then(|s| s.parse().ok())
}

/// Per-account on/off switches for decorative effects (folio-flair-1.0).
///
/// Stored as the `users.effects` JSON object. Every field is optional: an
/// absent key means "use the web's built-in default" (the defaults live in
/// `web/lib/effects.ts`, so they can change without a migration). Unknown
/// keys are rejected so a typo can't be stored silently.
///
/// On `PATCH /me/preferences` the object is **merged** into the stored one:
/// a key set to `true`/`false` overwrites, a key set to `null` clears it back
/// to the default, and absent keys are left alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectPrefs {
    /// Library cards tilt toward the pointer with a light sheen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_tilt: Option<bool>,
    /// Moving across a cover flips through a few of its pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_scrub: Option<bool>,
    /// Series cards show stacked page edges that grow with the issue count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spine_stack: Option<bool>,
    /// Series and issue pages get a blurred, cover-tinted backdrop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero_backdrop: Option<bool>,
    /// Creator and character chips open a preview card on hover / long-press.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover_cards: Option<bool>,
    /// The reader's background glows with the current page's edge colours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient_glow: Option<bool>,
    /// Paged-mode page turns animate as a page curl (fine pointer only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_curl: Option<bool>,
}

impl EffectPrefs {
    /// Read the stored JSON. A row that somehow holds a malformed object
    /// projects to all-defaults rather than failing `/me`.
    #[must_use]
    pub fn from_db(v: &serde_json::Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }
}

/// `PATCH` body for [`EffectPrefs`]: same keys, but each one distinguishes
/// absent (leave alone) from `null` (clear to default).
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectPrefsPatch {
    #[serde(default, deserialize_with = "some_value")]
    pub cover_tilt: Option<Option<bool>>,
    #[serde(default, deserialize_with = "some_value")]
    pub page_scrub: Option<Option<bool>>,
    #[serde(default, deserialize_with = "some_value")]
    pub spine_stack: Option<Option<bool>>,
    #[serde(default, deserialize_with = "some_value")]
    pub hero_backdrop: Option<Option<bool>>,
    #[serde(default, deserialize_with = "some_value")]
    pub hover_cards: Option<Option<bool>>,
    #[serde(default, deserialize_with = "some_value")]
    pub ambient_glow: Option<Option<bool>>,
    #[serde(default, deserialize_with = "some_value")]
    pub page_curl: Option<Option<bool>>,
}

impl EffectPrefsPatch {
    /// Apply this patch on top of `base` and return the merged value.
    #[must_use]
    pub fn apply(&self, mut base: EffectPrefs) -> EffectPrefs {
        macro_rules! merge {
            ($($f:ident),*) => { $( if let Some(v) = self.$f { base.$f = v; } )* };
        }
        merge!(
            cover_tilt,
            page_scrub,
            spine_stack,
            hero_backdrop,
            hover_cards,
            ambient_glow,
            page_curl
        );
        base
    }
}

fn some_value<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_serde() {
        for v in [Theme::System, Theme::Dark, Theme::Light, Theme::Amber] {
            let json = serde_json::to_string(&v).unwrap();
            let back: Theme = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
        }
    }

    #[test]
    fn serde_emits_wire_form() {
        let s = serde_json::to_string(&Theme::Amber).unwrap();
        assert_eq!(s, "\"amber\"");
    }

    #[test]
    fn serde_rejects_unknown() {
        let result: Result<Theme, _> = serde_json::from_str("\"midnight\"");
        assert!(result.is_err());
    }

    #[test]
    fn opt_from_db_drops_invalid() {
        let valid: Option<Theme> = opt_from_db(Some("dark"));
        assert_eq!(valid, Some(Theme::Dark));
        let invalid: Option<Theme> = opt_from_db(Some("midnight"));
        assert_eq!(invalid, None);
        let absent: Option<Theme> = opt_from_db(None);
        assert_eq!(absent, None);
    }

    #[test]
    fn fit_mode_parses_all() {
        assert_eq!("width".parse(), Ok(FitMode::Width));
        assert_eq!("height".parse(), Ok(FitMode::Height));
        assert_eq!("original".parse(), Ok(FitMode::Original));
        assert_eq!("contain".parse(), Ok(FitMode::Contain));
        assert!("zoom".parse::<FitMode>().is_err());
    }

    #[test]
    fn effects_patch_merges_and_clears() {
        let base = EffectPrefs {
            cover_tilt: Some(true),
            page_curl: Some(false),
            ..Default::default()
        };
        let patch: EffectPrefsPatch =
            serde_json::from_str(r#"{"cover_tilt":null,"ambient_glow":true}"#).unwrap();
        let merged = patch.apply(base);
        assert_eq!(merged.cover_tilt, None);
        assert_eq!(merged.ambient_glow, Some(true));
        assert_eq!(merged.page_curl, Some(false), "absent keys are left alone");
        assert_eq!(
            serde_json::to_value(&merged).unwrap(),
            serde_json::json!({"ambient_glow": true, "page_curl": false})
        );
    }

    #[test]
    fn effects_reject_unknown_keys_and_bad_types() {
        assert!(serde_json::from_str::<EffectPrefsPatch>(r#"{"sparkles":true}"#).is_err());
        assert!(serde_json::from_str::<EffectPrefsPatch>(r#"{"cover_tilt":"yes"}"#).is_err());
    }

    #[test]
    fn effects_from_db_tolerates_garbage() {
        assert_eq!(
            EffectPrefs::from_db(&serde_json::json!({"nope": 1})),
            EffectPrefs::default()
        );
        assert_eq!(
            EffectPrefs::from_db(&serde_json::json!({"page_scrub": true})).page_scrub,
            Some(true)
        );
    }
}
