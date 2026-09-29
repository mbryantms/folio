//! Boot-time Postgres version guard (WP-1.6 ops hygiene).
//!
//! The search migration relies on Postgres 17+ behaviour; on an older
//! server it fails part-way through `migration::Migrator::up`, leaving
//! the operator with a half-applied schema and an opaque SQL error. The
//! guard runs `SHOW server_version_num` right after the pool connects
//! and — when the server is too old — refuses to boot with a message
//! that names the found version and the minimum, *before* any migration
//! touches the schema. There is deliberately no escape hatch: the
//! secrets/pepper guards in [`crate::app::serve`] have none either, and
//! "run anyway" would just move the failure to the migration step.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

/// Minimum supported `server_version_num` — Postgres 17.0. The dev / CI
/// stack pins `postgres:18-alpine`; anything from 17.0 up is accepted.
pub const MIN_SERVER_VERSION_NUM: u32 = 170_000;

/// Parse the text `SHOW server_version_num` returns (e.g. `"180001"`).
///
/// Postgres encodes the version as `major * 10000 + minor` (`170004` is
/// 17.4). The value is plain digits; anything else is a malformed
/// server reply and surfaces as an error rather than a silent pass.
pub fn parse_server_version_num(raw: &str) -> Result<u32, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("server_version_num was empty".to_owned());
    }
    trimmed
        .parse::<u32>()
        .map_err(|e| format!("server_version_num {trimmed:?} is not a number: {e}"))
}

/// Render a `server_version_num` as the operator-facing `major.minor`
/// form (`170004` → `17.4`, `180001` → `18.1`).
pub fn format_version_num(num: u32) -> String {
    format!("{}.{}", num / 10_000, num % 10_000)
}

/// Compare a parsed `server_version_num` against
/// [`MIN_SERVER_VERSION_NUM`]. `Err` carries the full operator-facing
/// message so the caller can `bail!` with it verbatim.
pub fn check_supported(num: u32) -> Result<(), String> {
    if num >= MIN_SERVER_VERSION_NUM {
        return Ok(());
    }
    Err(format!(
        "refusing to boot: Postgres {found} is too old; Folio requires \
         Postgres {min} or newer (server_version_num {num} < \
         {min_num}). The search migration uses 17+ features and would \
         fail part-way through, leaving a half-applied schema. Upgrade \
         the database server (the shipped compose.prod.yml runs \
         postgres:18 — see docs/install/upgrades.md for the dump/restore \
         path when moving a volume across a Postgres major) and restart.",
        found = format_version_num(num),
        min = format_version_num(MIN_SERVER_VERSION_NUM),
        num = num,
        min_num = MIN_SERVER_VERSION_NUM,
    ))
}

/// Query the connected server's version and refuse to proceed when it is
/// below [`MIN_SERVER_VERSION_NUM`]. Call this after the pool connects
/// and before migrations run.
pub async fn assert_supported(db: &DatabaseConnection) -> anyhow::Result<u32> {
    let stmt = Statement::from_string(DbBackend::Postgres, "SHOW server_version_num");
    let row = db
        .query_one_raw(stmt)
        .await?
        .ok_or_else(|| anyhow::anyhow!("SHOW server_version_num returned no row"))?;
    let raw: String = row.try_get_by_index(0)?;
    let num = parse_server_version_num(&raw).map_err(anyhow::Error::msg)?;
    check_supported(num).map_err(anyhow::Error::msg)?;
    tracing::info!(
        server_version_num = num,
        version = %format_version_num(num),
        "postgres version check passed",
    );
    Ok(num)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_digits_and_trims_whitespace() {
        assert_eq!(parse_server_version_num("180001").unwrap(), 180_001);
        assert_eq!(parse_server_version_num(" 170004\n").unwrap(), 170_004);
    }

    #[test]
    fn rejects_empty_and_non_numeric() {
        assert!(parse_server_version_num("").is_err());
        assert!(parse_server_version_num("   ").is_err());
        assert!(parse_server_version_num("17.4").is_err());
        assert!(parse_server_version_num("abc").is_err());
    }

    #[test]
    fn formats_major_minor() {
        assert_eq!(format_version_num(170_004), "17.4");
        assert_eq!(format_version_num(180_000), "18.0");
        assert_eq!(format_version_num(160_009), "16.9");
    }

    #[test]
    fn accepts_17_0_and_above() {
        assert!(check_supported(170_000).is_ok());
        assert!(check_supported(170_004).is_ok());
        assert!(check_supported(180_001).is_ok());
        assert!(check_supported(u32::MAX).is_ok());
    }

    #[test]
    fn rejects_below_17_with_actionable_message() {
        let err = check_supported(160_009).unwrap_err();
        assert!(err.starts_with("refusing to boot"), "{err}");
        assert!(err.contains("Postgres 16.9"), "names found version: {err}");
        assert!(
            err.contains("Postgres 17.0 or newer"),
            "names minimum: {err}"
        );
        assert!(err.contains("160009 < 170000"), "raw numbers: {err}");
        assert!(
            err.contains("docs/install/upgrades.md"),
            "points at docs: {err}"
        );

        // Boundary: one below the minimum is still rejected.
        assert!(check_supported(MIN_SERVER_VERSION_NUM - 1).is_err());
    }
}
