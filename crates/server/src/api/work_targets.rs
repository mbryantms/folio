//! Human labels for the ids background-work surfaces carry.
//!
//! Dead jobs (`/admin/queue/dead-jobs`) and audit rows (`/audit`) hand the
//! client a job's `args` / an action's payload as opaque JSON. Those
//! payloads name their targets by id — and an issue id is its BLAKE3
//! content hash, a series id a UUID — so a failed job read as
//! `issue_id: 9f3a…` is not something an admin can act on. This module
//! turns the ids a payload carries into labelled, linkable targets:
//!
//! - `issue_id` / `issue_ids[]` → "Series Name #12 — Title" with the
//!   series + issue slugs for the issue page;
//! - `series_id` → "Series Name (1987)" with its slug;
//! - `library_id` → the library name with its slug;
//! - `run_id` → the metadata run's own scope entity (an issue or a series),
//!   labelled as that entity.
//!
//! One bulk resolution per response: callers collect the refs from every
//! payload ([`collect_refs`]), resolve them in four queries
//! ([`resolve`]), then read each payload's targets back
//! ([`targets_for`]). Ids that no longer resolve (a removed issue) are
//! left out, the raw payload still shows them.

use std::collections::{HashMap, HashSet};

use entity::{issue, library, metadata_run, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

use crate::state::AppState;

/// A labelled entity a background job or audit action targeted.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema, PartialEq, Eq)]
pub struct WorkTargetView {
    /// `issue` | `series` | `library`.
    pub kind: String,
    pub id: String,
    /// Human label: `"The Flash #12 — Learning Curve"`, `"The Flash
    /// (1987)"`, `"DC"`.
    pub label: String,
    /// The owning series' slug (issue and series targets).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_slug: Option<String>,
    /// The issue's slug (issue targets).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_slug: Option<String>,
    /// The library's slug (library targets).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_slug: Option<String>,
}

/// Ids collected from one or more payloads, before resolution.
#[derive(Debug, Default)]
pub struct TargetRefs {
    pub issues: HashSet<String>,
    pub series: HashSet<Uuid>,
    pub libraries: HashSet<Uuid>,
    pub runs: HashSet<Uuid>,
}

/// Payload keys that name a target. `scope_entity_id` is a metadata run's
/// own key and is read with its `scope`.
const ISSUE_KEYS: [&str; 2] = ["issue_id", "scope_entity_id"];

/// Pull every id a payload names into `refs`. Walks one level of nesting
/// (a job wrapped in `{ "job": {…} }`, a bulk payload's `targets: [{…}]`)
/// so the common shapes resolve without per-queue code.
pub fn collect_refs(payload: &serde_json::Value, refs: &mut TargetRefs) {
    collect_depth(payload, refs, 0);
}

fn collect_depth(v: &serde_json::Value, refs: &mut TargetRefs, depth: u8) {
    let Some(obj) = v.as_object() else {
        if let (Some(arr), true) = (v.as_array(), depth < 2) {
            for item in arr {
                collect_depth(item, refs, depth + 1);
            }
        }
        return;
    };
    let scope = obj.get("scope").and_then(|s| s.as_str());
    for key in ISSUE_KEYS {
        if let Some(s) = obj.get(key).and_then(|x| x.as_str()) {
            // `scope_entity_id` names an issue or a series depending on the
            // run's scope; without a scope it is tried as both.
            match (key, scope) {
                ("scope_entity_id", Some("series")) => {
                    if let Ok(u) = Uuid::parse_str(s) {
                        refs.series.insert(u);
                    }
                }
                ("scope_entity_id", None) => {
                    if let Ok(u) = Uuid::parse_str(s) {
                        refs.series.insert(u);
                    } else {
                        refs.issues.insert(s.to_owned());
                    }
                }
                _ => {
                    refs.issues.insert(s.to_owned());
                }
            }
        }
    }
    if let Some(arr) = obj.get("issue_ids").and_then(|x| x.as_array()) {
        for s in arr.iter().filter_map(|x| x.as_str()) {
            refs.issues.insert(s.to_owned());
        }
    }
    for (key, set) in [
        ("series_id", &mut refs.series),
        ("library_id", &mut refs.libraries),
        ("run_id", &mut refs.runs),
    ] {
        if let Some(u) = obj
            .get(key)
            .and_then(|x| x.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            set.insert(u);
        }
    }
    if let Some(u) = obj
        .get("triggering_run_id")
        .and_then(|x| x.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        refs.runs.insert(u);
    }
    if depth < 2 {
        for (k, child) in obj {
            if matches!(k.as_str(), "job" | "args" | "targets" | "payload" | "items") {
                collect_depth(child, refs, depth + 1);
            }
        }
    }
}

/// Labels for the refs, keyed by id. A run resolves to its scope entity's
/// target, so a dead apply job reads as the issue it was applying to.
#[derive(Debug, Default)]
pub struct Resolved {
    pub issues: HashMap<String, WorkTargetView>,
    pub series: HashMap<Uuid, WorkTargetView>,
    pub libraries: HashMap<Uuid, WorkTargetView>,
    pub runs: HashMap<Uuid, WorkTargetView>,
}

pub async fn resolve(app: &AppState, mut refs: TargetRefs) -> Result<Resolved, sea_orm::DbErr> {
    let mut out = Resolved::default();

    // Runs first: they add their scope entities to the issue / series sets.
    let mut run_scope: HashMap<Uuid, (String, String)> = HashMap::new();
    if !refs.runs.is_empty() {
        let rows: Vec<(Uuid, String, Option<String>)> = metadata_run::Entity::find()
            .select_only()
            .column(metadata_run::Column::Id)
            .column(metadata_run::Column::Scope)
            .column(metadata_run::Column::ScopeEntityId)
            .filter(metadata_run::Column::Id.is_in(refs.runs.iter().copied()))
            .into_tuple()
            .all(&app.db)
            .await?;
        for (id, scope, entity) in rows {
            let Some(entity) = entity else { continue };
            match scope.as_str() {
                "issue" => {
                    refs.issues.insert(entity.clone());
                }
                "series" => {
                    if let Ok(u) = Uuid::parse_str(&entity) {
                        refs.series.insert(u);
                    }
                }
                _ => continue,
            }
            run_scope.insert(id, (scope, entity));
        }
    }

    if !refs.issues.is_empty() {
        let rows: Vec<IssueRow> = issue::Entity::find()
            .select_only()
            .column(issue::Column::Id)
            .column(issue::Column::Slug)
            .column(issue::Column::NumberRaw)
            .column(issue::Column::Title)
            .column(issue::Column::SeriesId)
            .filter(issue::Column::Id.is_in(refs.issues.iter().cloned()))
            .into_tuple()
            .all(&app.db)
            .await?;
        for (_, _, _, _, sid) in &rows {
            refs.series.insert(*sid);
        }
        let series_rows = load_series(app, &refs.series).await?;
        for (id, slug, number, title, sid) in rows {
            let (sname, sslug) = series_rows
                .get(&sid)
                .map(|(n, s, _)| (n.as_str(), s.as_str()))
                .unwrap_or(("", ""));
            out.issues.insert(
                id.clone(),
                WorkTargetView {
                    kind: "issue".into(),
                    id,
                    label: issue_label(sname, number.as_deref(), title.as_deref()),
                    series_slug: Some(sslug.to_owned()),
                    issue_slug: Some(slug),
                    library_slug: None,
                },
            );
        }
        for (sid, (name, slug, year)) in series_rows {
            out.series
                .entry(sid)
                .or_insert_with(|| series_target(sid, &name, &slug, year));
        }
    } else if !refs.series.is_empty() {
        for (sid, (name, slug, year)) in load_series(app, &refs.series).await? {
            out.series
                .insert(sid, series_target(sid, &name, &slug, year));
        }
    }

    if !refs.libraries.is_empty() {
        let rows: Vec<(Uuid, String, String)> = library::Entity::find()
            .select_only()
            .column(library::Column::Id)
            .column(library::Column::Name)
            .column(library::Column::Slug)
            .filter(library::Column::Id.is_in(refs.libraries.iter().copied()))
            .into_tuple()
            .all(&app.db)
            .await?;
        for (id, name, slug) in rows {
            out.libraries.insert(
                id,
                WorkTargetView {
                    kind: "library".into(),
                    id: id.to_string(),
                    label: name,
                    series_slug: None,
                    issue_slug: None,
                    library_slug: Some(slug),
                },
            );
        }
    }

    for (run, (scope, entity)) in run_scope {
        let target = match scope.as_str() {
            "issue" => out.issues.get(&entity).cloned(),
            "series" => Uuid::parse_str(&entity)
                .ok()
                .and_then(|u| out.series.get(&u).cloned()),
            _ => None,
        };
        if let Some(t) = target {
            out.runs.insert(run, t);
        }
    }
    Ok(out)
}

/// `(id, slug, number_raw, title, series_id)` projection.
type IssueRow = (String, String, Option<String>, Option<String>, Uuid);
type SeriesRows = HashMap<Uuid, (String, String, Option<i32>)>;

async fn load_series(app: &AppState, ids: &HashSet<Uuid>) -> Result<SeriesRows, sea_orm::DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, String, String, Option<i32>)> = series::Entity::find()
        .select_only()
        .column(series::Column::Id)
        .column(series::Column::Name)
        .column(series::Column::Slug)
        .column(series::Column::Year)
        .filter(series::Column::Id.is_in(ids.iter().copied()))
        .into_tuple()
        .all(&app.db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, slug, year)| (id, (name, slug, year)))
        .collect())
}

fn series_target(id: Uuid, name: &str, slug: &str, year: Option<i32>) -> WorkTargetView {
    WorkTargetView {
        kind: "series".into(),
        id: id.to_string(),
        label: series_label(name, year),
        series_slug: Some(slug.to_owned()),
        issue_slug: None,
        library_slug: None,
    }
}

/// `"The Flash (1987)"`; a series without a year is just its name.
pub fn series_label(name: &str, year: Option<i32>) -> String {
    match year {
        Some(y) => format!("{name} ({y})"),
        None => name.to_owned(),
    }
}

/// `"The Flash #12 — Learning Curve"`: series name, `#number` when the
/// issue has one, the title when it has one.
pub fn issue_label(series_name: &str, number: Option<&str>, title: Option<&str>) -> String {
    let mut out = series_name.trim().to_owned();
    if let Some(n) = number.map(str::trim).filter(|n| !n.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push('#');
        out.push_str(n);
    }
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        if !out.is_empty() {
            out.push_str(" — ");
        }
        out.push_str(t);
    }
    if out.is_empty() {
        "Issue".to_owned()
    } else {
        out
    }
}

/// The targets one payload names, in reading order (issues, then the
/// series, then the library, then what its run points at), deduplicated.
pub fn targets_for(payload: &serde_json::Value, resolved: &Resolved) -> Vec<WorkTargetView> {
    let mut refs = TargetRefs::default();
    collect_refs(payload, &mut refs);
    let mut out: Vec<WorkTargetView> = Vec::new();
    let mut push = |t: &WorkTargetView| {
        if !out.iter().any(|x| x.kind == t.kind && x.id == t.id) {
            out.push(t.clone());
        }
    };
    let mut issues: Vec<&String> = refs.issues.iter().collect();
    issues.sort();
    for id in issues {
        if let Some(t) = resolved.issues.get(id) {
            push(t);
        }
    }
    let mut series: Vec<&Uuid> = refs.series.iter().collect();
    series.sort();
    for id in series {
        if let Some(t) = resolved.series.get(id) {
            push(t);
        }
    }
    let mut libs: Vec<&Uuid> = refs.libraries.iter().collect();
    libs.sort();
    for id in libs {
        if let Some(t) = resolved.libraries.get(id) {
            push(t);
        }
    }
    let mut runs: Vec<&Uuid> = refs.runs.iter().collect();
    runs.sort();
    for id in runs {
        if let Some(t) = resolved.runs.get(id) {
            push(t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labels_read_like_the_series_page() {
        assert_eq!(
            issue_label("The Flash", Some("12"), Some("Learning Curve")),
            "The Flash #12 — Learning Curve"
        );
        assert_eq!(issue_label("The Flash", Some("12"), None), "The Flash #12");
        assert_eq!(
            issue_label("The Flash", None, Some("Annual")),
            "The Flash — Annual"
        );
        assert_eq!(issue_label("", None, None), "Issue");
        assert_eq!(series_label("The Flash", Some(1987)), "The Flash (1987)");
        assert_eq!(series_label("Saga", None), "Saga");
    }

    #[test]
    fn collects_ids_from_the_common_payload_shapes() {
        let sid = Uuid::now_v7();
        let lid = Uuid::now_v7();
        let rid = Uuid::now_v7();
        let mut refs = TargetRefs::default();
        collect_refs(
            &json!({
                "issue_id": "abc", "series_id": sid.to_string(), "library_id": lid.to_string(),
                "run_id": rid.to_string(), "actor_id": Uuid::now_v7().to_string(),
                "job": { "issue_ids": ["def", "ghi"] },
                "targets": [{ "issue_id": "jkl" }],
            }),
            &mut refs,
        );
        assert_eq!(
            refs.issues,
            ["abc", "def", "ghi", "jkl"].map(String::from).into()
        );
        assert_eq!(refs.series, [sid].into());
        assert_eq!(refs.libraries, [lid].into());
        assert_eq!(refs.runs, [rid].into());

        // A metadata run payload: `scope_entity_id` follows `scope`.
        let mut refs = TargetRefs::default();
        collect_refs(
            &json!({ "scope": "series", "scope_entity_id": sid.to_string() }),
            &mut refs,
        );
        assert!(refs.issues.is_empty());
        assert_eq!(refs.series, [sid].into());
        let mut refs = TargetRefs::default();
        collect_refs(
            &json!({ "scope": "issue", "scope_entity_id": "hash" }),
            &mut refs,
        );
        assert_eq!(refs.issues, ["hash".to_owned()].into());
    }

    #[test]
    fn targets_are_ordered_and_deduplicated() {
        let sid = Uuid::now_v7();
        let resolved = Resolved {
            issues: [(
                "abc".to_owned(),
                WorkTargetView {
                    kind: "issue".into(),
                    id: "abc".into(),
                    label: "X #1".into(),
                    series_slug: Some("x".into()),
                    issue_slug: Some("1".into()),
                    library_slug: None,
                },
            )]
            .into(),
            series: [(sid, series_target(sid, "X", "x", Some(2001)))].into(),
            libraries: HashMap::new(),
            runs: HashMap::new(),
        };
        let t = targets_for(
            &json!({ "issue_id": "abc", "series_id": sid.to_string(), "job": { "issue_id": "abc" } }),
            &resolved,
        );
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].label, "X #1");
        assert_eq!(t[1].label, "X (2001)");
    }
}
