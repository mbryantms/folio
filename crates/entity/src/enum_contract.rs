//! Test helper for typed discriminator columns (AR-7).
//!
//! A `DeriveActiveEnum` over a `TEXT` column is a storage contract with
//! every existing row: renaming a `string_value` would orphan them, and the
//! JSON wire form must stay the same string. [`assert_exact_strings`] pins
//! both directions for every variant, and that the table lists them all.

use sea_orm::{ActiveEnum, Iterable};
use serde::{Serialize, de::DeserializeOwned};
use std::fmt::Debug;

/// Each `(variant, s)` round-trips `s` through the DB value, `as_str`,
/// `parse` and serde; `pairs` covers every variant; and each string in
/// `unknown` is rejected by the DB decoder.
pub(crate) fn assert_exact_strings<E>(
    pairs: &[(E, &str)],
    as_str: fn(E) -> &'static str,
    parse: fn(&str) -> Option<E>,
    unknown: &[&str],
) where
    E: ActiveEnum<Value = String>
        + Iterable
        + Serialize
        + DeserializeOwned
        + Copy
        + PartialEq
        + Debug,
{
    for &(variant, s) in pairs {
        assert_eq!(variant.to_value(), s);
        assert_eq!(as_str(variant), s);
        assert_eq!(parse(s), Some(variant));
        assert_eq!(E::try_from_value(&s.to_owned()).unwrap(), variant);
        assert_eq!(serde_json::to_value(variant).unwrap(), s);
        assert_eq!(
            serde_json::from_value::<E>(serde_json::json!(s)).unwrap(),
            variant
        );
    }
    assert_eq!(E::iter().count(), pairs.len(), "a variant is missing a pin");
    for &s in unknown {
        assert!(
            E::try_from_value(&s.to_owned()).is_err(),
            "unknown stored string {s:?} must fail to decode"
        );
        assert_eq!(parse(s), None);
        assert!(serde_json::from_value::<E>(serde_json::json!(s)).is_err());
    }
}
