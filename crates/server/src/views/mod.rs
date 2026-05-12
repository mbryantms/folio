//! Saved smart views (M3+).
//!
//! Three pieces:
//!
//!   - `dsl` — wire-format types for the filter DSL (`Condition`, `Op`,
//!     `Field`, `FilterDsl`). Stored verbatim in `saved_views.conditions`
//!     and round-tripped through the OpenAPI surface.
//!   - `registry` — per-field metadata (kind, allowed ops, column
//!     mapping). Single source of truth for both server validation and
//!     client field-picker labels (mirrored to TS via OpenAPI in M5).
//!   - `compile` — compiles a validated DSL into a
//!     `sea_query::SelectStatement` that the results endpoint runs.
//!
//! The CBL kind doesn't compile through this module — it dispatches to
//! M4's CBL list reader directly. The discriminator lives on
//! `saved_views.kind`.

pub mod compile;
pub mod dsl;
pub mod registry;
