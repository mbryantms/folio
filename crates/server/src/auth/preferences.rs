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

/// `width` | `height` | `original`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FitMode {
    Width,
    Height,
    Original,
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
    "fit_mode must be 'width', 'height', or 'original'",
    Width => "width",
    Height => "height",
    Original => "original",
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

/// Convenience: project an `Option<String>` from the entity layer to an
/// `Option<TypedEnum>` for response DTOs. Invalid wire strings parse to
/// `None` — the server validates on write, so a DB row carrying an
/// out-of-range value is treated as "unset" rather than panicking.
pub fn opt_from_db<E: FromStr>(value: Option<&str>) -> Option<E> {
    value.and_then(|s| s.parse().ok())
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
        assert!("zoom".parse::<FitMode>().is_err());
    }
}
