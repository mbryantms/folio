//! Provider-filled profiles for the entity landing pages (characters,
//! teams, locations, story arcs) — the "wiki" block.
//!
//! One fetch per entity, queued lazily by the first page view (or an
//! admin's Refresh) and run by `jobs::entity_enrich`:
//!
//! 1. **Metron** is asked first when configured: a name search picks
//!    the exact-name record, whose detail carries the ComicVine and
//!    GCD ids. That is what makes the ComicVine step a direct fetch.
//! 2. **ComicVine** fills the content: deck, the wiki description
//!    (split into [`Section`]s here), real name, aliases, gender,
//!    birthday, powers, creators, teams / members, first appearance
//!    and the appearance count. Without a Metron id it falls back to
//!    its own exact-name search.
//!
//! The merged result lands in three places: the entity row's short
//! fields (`description`, `image_url`, `aliases`, `real_name`) through
//! [`writers::update_entity_display`], the provider ids through
//! [`writers::set_external_id`], and the `entity_profile` row holding
//! the summary, sections, [`Facts`] and per-provider counts.

use anyhow::Context as _;
use chrono::Utc;
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter, Set, Statement,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::comicvine::{ComicVineClient, CvEntityKind, CvEntityRecord};
use super::identifier::{Identifier, Source};
use super::metron::{MetronClient, MetronEntityKind, MetronEntityRecord};
use super::provider::ProviderError;
use super::writers::{self, SetBy};
use crate::config::Config;
use crate::state::AppState;

/// An entity table a profile can be fetched for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Character,
    Team,
    Location,
    StoryArc,
}

impl ProfileKind {
    /// `external_ids.entity_type` / `entity_profile.entity_type` value
    /// (also the table name).
    pub fn entity_type(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Team => "team",
            Self::Location => "location",
            Self::StoryArc => "story_arc",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "character" => Some(Self::Character),
            "team" => Some(Self::Team),
            "location" => Some(Self::Location),
            "story_arc" => Some(Self::StoryArc),
            _ => None,
        }
    }

    fn cv_kind(self) -> CvEntityKind {
        match self {
            Self::Character => CvEntityKind::Character,
            Self::Team => CvEntityKind::Team,
            Self::Location => CvEntityKind::Location,
            Self::StoryArc => CvEntityKind::StoryArc,
        }
    }

    /// Metron has no location records.
    fn metron_kind(self) -> Option<MetronEntityKind> {
        match self {
            Self::Character => Some(MetronEntityKind::Character),
            Self::Team => Some(MetronEntityKind::Team),
            Self::StoryArc => Some(MetronEntityKind::StoryArc),
            Self::Location => None,
        }
    }
}

/// One wiki section split out of ComicVine's description.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Section {
    /// Empty for the lead text before the first heading.
    pub heading: String,
    /// `0` lead, `2` for `<h2>`, `3` for `<h3>`.
    pub level: u8,
    pub paragraphs: Vec<String>,
}

/// The issue an entity first appeared in, as the provider names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FirstAppearance {
    /// `"Detective Comics #27"` — the provider's label.
    pub label: String,
    /// ComicVine issue id, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comicvine_issue_id: Option<String>,
}

/// Fact sheet. Fields a kind doesn't carry stay empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Facts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub real_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gender: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub birthday: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_appearance: Option<FirstAppearance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub creators: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<String>,
    /// Team members (teams only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub powers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub universes: Vec<String>,
}

/// What a fetch produced, before it is written.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub aliases: Vec<String>,
    pub sections: Vec<Section>,
    pub facts: Facts,
    /// `source → issues the provider lists`.
    pub provider_counts: HashMap<String, u64>,
    pub sources: Vec<Source>,
    pub identifiers: Vec<Identifier>,
}

/// Are any of the providers the profiles use configured?
pub fn providers_configured(cfg: &Config) -> bool {
    comicvine_configured(cfg) || metron_configured(cfg)
}

fn comicvine_configured(cfg: &Config) -> bool {
    cfg.comicvine_enabled
        && cfg
            .comicvine_api_key
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
}

fn metron_configured(cfg: &Config) -> bool {
    cfg.metron_enabled && super::metron::MetronAuth::from_config(cfg).is_some()
}

fn comicvine_client(state: &AppState) -> Option<ComicVineClient> {
    let cfg = state.cfg();
    if !comicvine_configured(&cfg) {
        return None;
    }
    let key = cfg.comicvine_api_key.clone()?;
    Some(match cfg.comicvine_base_url.clone() {
        Some(base) => ComicVineClient::with_base_url(key, base, state.jobs.redis.clone()),
        None => ComicVineClient::new(key, state.jobs.redis.clone()),
    })
}

fn metron_client(state: &AppState) -> Option<MetronClient> {
    let cfg = state.cfg();
    if !metron_configured(&cfg) {
        return None;
    }
    MetronClient::from_config(&cfg, state.jobs.redis.clone())
}

/// Collapse whitespace + case for exact-name matching.
fn name_key(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Fetch and store the profile of one entity. Errors that mean "no record
/// today" (no exact-name match, provider quota) are recorded on the row
/// and returned as `Ok(())`; only infrastructure failures surface as
/// `Err`.
pub async fn enrich(state: &AppState, kind: ProfileKind, entity_id: Uuid) -> anyhow::Result<()> {
    let db = &state.db;
    let entity_type = kind.entity_type();
    #[derive(Debug, FromQueryResult)]
    struct NameRow {
        name: String,
    }
    let Some(row) = NameRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        format!("SELECT name FROM {entity_type} WHERE id = $1"),
        [entity_id.into()],
    ))
    .one(db)
    .await?
    else {
        return Ok(());
    };
    let name = row.name;
    let known = known_ids(db, entity_type, &entity_id.to_string()).await?;

    let mut profile = Profile::default();
    let mut problems: Vec<String> = Vec::new();

    // ── Metron: identity + ids ──
    let mut metron: Option<MetronEntityRecord> = None;
    if let (Some(client), Some(mk)) = (metron_client(state), kind.metron_kind()) {
        match lookup_metron(&client, mk, &name, known.get("metron").map(String::as_str)).await {
            Ok(Some((rec, count))) => {
                if let Some(n) = count {
                    profile.provider_counts.insert("metron".into(), n);
                }
                metron = Some(rec);
            }
            Ok(None) => problems.push(format!("Metron has no record named {name:?}")),
            Err(e) => problems.push(format!("Metron: {e}")),
        }
    }

    // ── ComicVine: content ──
    let mut cv: Option<CvEntityRecord> = None;
    if let Some(client) = comicvine_client(state) {
        let cv_id = known.get("comicvine").cloned().or_else(|| {
            metron
                .as_ref()
                .and_then(|m| m.cv_id)
                .map(|id| id.to_string())
        });
        match lookup_comicvine(&client, kind.cv_kind(), &name, cv_id.as_deref()).await {
            Ok(Some(rec)) => {
                if let Some(n) = rec.issue_appearances {
                    profile
                        .provider_counts
                        .insert("comicvine".into(), n.max(0) as u64);
                }
                cv = Some(rec);
            }
            Ok(None) => problems.push(format!("ComicVine has no record named {name:?}")),
            Err(e) => problems.push(format!("ComicVine: {e}")),
        }
    }

    if metron.is_none() && cv.is_none() {
        let error = if problems.is_empty() {
            "No metadata provider is configured".to_owned()
        } else {
            problems.join("; ")
        };
        write_failure(db, entity_type, entity_id, &error).await?;
        return Ok(());
    }

    merge(&mut profile, entity_type, metron.as_ref(), cv.as_ref());
    write_profile(db, kind, entity_id, &profile, &problems).await?;
    Ok(())
}

/// Provider ids the entity already carries (`source → external_id`).
async fn known_ids<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
) -> Result<HashMap<String, String>, sea_orm::DbErr> {
    let rows = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq(entity_type))
        .filter(entity::external_id::Column::EntityId.eq(entity_id))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.source, r.external_id))
        .collect())
}

async fn lookup_metron(
    client: &MetronClient,
    kind: MetronEntityKind,
    name: &str,
    known_id: Option<&str>,
) -> Result<Option<(MetronEntityRecord, Option<u64>)>, ProviderError> {
    let id = match known_id {
        Some(id) => id.to_owned(),
        None => {
            let key = name_key(name);
            let hits = client.search_entities(kind, name).await?;
            let Some(hit) = hits.iter().find(|h| name_key(&h.name) == key) else {
                return Ok(None);
            };
            hit.id.to_string()
        }
    };
    let rec = client.fetch_entity(kind, &id).await?;
    // The count is one more request; a failure there is not a failure
    // of the record.
    let count = client.count_entity_issues(kind, &id).await.ok().flatten();
    Ok(Some((rec, count)))
}

async fn lookup_comicvine(
    client: &ComicVineClient,
    kind: CvEntityKind,
    name: &str,
    known_id: Option<&str>,
) -> Result<Option<CvEntityRecord>, ProviderError> {
    let id = match known_id {
        Some(id) => id.to_owned(),
        None => {
            let key = name_key(name);
            let hits = client.search_entities(kind, name).await?;
            // Exact name only; among exact matches the one with the most
            // appearances is the main continuity's record.
            let Some(hit) = hits
                .iter()
                .filter(|h| name_key(&h.name) == key)
                .max_by_key(|h| h.issue_appearances.unwrap_or(0))
            else {
                return Ok(None);
            };
            hit.id.to_string()
        }
    };
    Ok(Some(client.fetch_entity(kind, &id).await?))
}

fn dedupe(mut names: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(name_key(n)));
    names
}

/// Merge the two records into one profile. ComicVine wins for content
/// (deck, wiki, facts, image); Metron for the summary, universes and the
/// cross-provider ids.
fn merge(
    profile: &mut Profile,
    entity_type: &str,
    metron: Option<&MetronEntityRecord>,
    cv: Option<&CvEntityRecord>,
) {
    if let Some(m) = metron {
        profile.sources.push(Source::Metron);
        profile.identifiers.push(Identifier::with_canonical_url(
            Source::Metron,
            m.id.to_string(),
            entity_type,
        ));
        if let Some(gcd) = m.gcd_id {
            profile.identifiers.push(Identifier::with_canonical_url(
                Source::Gcd,
                gcd.to_string(),
                entity_type,
            ));
        }
    }
    if let Some(c) = cv {
        profile.sources.push(Source::ComicVine);
        profile.identifiers.push(Identifier::with_canonical_url(
            Source::ComicVine,
            c.id.to_string(),
            entity_type,
        ));
    } else if let Some(cv_id) = metron.and_then(|m| m.cv_id) {
        // Metron named the ComicVine record even though ComicVine isn't
        // configured (or failed): keep the id, it costs nothing.
        profile.identifiers.push(Identifier::with_canonical_url(
            Source::ComicVine,
            cv_id.to_string(),
            entity_type,
        ));
    }

    profile.summary = metron
        .and_then(|m| m.description.clone())
        .or_else(|| cv.and_then(|c| c.deck.clone()));
    profile.description = cv
        .and_then(|c| c.deck.clone())
        .or_else(|| metron.and_then(|m| m.description.clone()));
    profile.image_url = cv
        .and_then(|c| c.image_url.clone())
        .or_else(|| metron.and_then(|m| m.image_url.clone()));
    profile.aliases = cv
        .map(|c| c.aliases.clone())
        .filter(|a| !a.is_empty())
        .or_else(|| metron.map(|m| m.aliases.clone()))
        .unwrap_or_default();
    profile.sections = cv
        .and_then(|c| c.description_html.as_deref())
        .map(sections_from_html)
        .unwrap_or_default();

    let facts = &mut profile.facts;
    if let Some(c) = cv {
        facts.real_name = c.real_name.clone();
        facts.gender = c.gender.clone();
        facts.birthday = c.birth.clone();
        facts.publisher = c.publisher.clone();
        facts.first_appearance = c.first_appearance.as_ref().and_then(|f| {
            let label = match (&f.name, &f.issue_number) {
                (Some(n), Some(num)) => format!("{n} #{num}"),
                (Some(n), None) => n.clone(),
                (None, Some(num)) => format!("#{num}"),
                (None, None) => return None,
            };
            Some(FirstAppearance {
                label,
                comicvine_issue_id: f.issue_id.map(|id| id.to_string()),
            })
        });
        facts.powers = c.powers.clone();
        facts.members = c.members.clone();
    }
    let mut creators: Vec<String> = cv.map(|c| c.creators.clone()).unwrap_or_default();
    creators.extend(metron.map(|m| m.creators.clone()).unwrap_or_default());
    facts.creators = dedupe(creators);
    let mut teams: Vec<String> = metron.map(|m| m.teams.clone()).unwrap_or_default();
    teams.extend(cv.map(|c| c.teams.clone()).unwrap_or_default());
    facts.teams = dedupe(teams);
    facts.universes = metron.map(|m| m.universes.clone()).unwrap_or_default();
}

async fn write_failure<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: Uuid,
    error: &str,
) -> anyhow::Result<()> {
    let now = Utc::now().fixed_offset();
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "INSERT INTO entity_profile (entity_type, entity_id, error, fetched_at, updated_at) \
         VALUES ($1, $2, $3, $4, $4) \
         ON CONFLICT (entity_type, entity_id) DO UPDATE \
            SET error = EXCLUDED.error, fetched_at = EXCLUDED.fetched_at, \
                updated_at = EXCLUDED.updated_at",
        [
            entity_type.into(),
            entity_id.into(),
            error.into(),
            now.into(),
        ],
    ))
    .await
    .context("record profile failure")?;
    Ok(())
}

async fn write_profile(
    db: &sea_orm::DatabaseConnection,
    kind: ProfileKind,
    entity_id: Uuid,
    profile: &Profile,
    problems: &[String],
) -> anyhow::Result<()> {
    let entity_type = kind.entity_type();
    let id_str = entity_id.to_string();
    writers::update_entity_display(
        db,
        entity_type,
        entity_id,
        writers::EntityDisplay {
            description: profile.description.as_deref(),
            image_url: profile.image_url.as_deref(),
            aliases: (!profile.aliases.is_empty()).then_some(profile.aliases.as_slice()),
            real_name: (kind == ProfileKind::Character)
                .then_some(profile.facts.real_name.as_deref())
                .flatten(),
        },
    )
    .await
    .context("update entity display fields")?;
    for ident in &profile.identifiers {
        writers::set_external_id(
            db,
            entity_type,
            &id_str,
            ident,
            SetBy::Provider(ident.source),
        )
        .await
        .context("write entity external id")?;
    }
    let now = Utc::now().fixed_offset();
    let sources: Vec<&str> = profile.sources.iter().map(|s| s.as_str()).collect();
    // A provider that answered nothing today is a note, not a failure.
    let error = (!problems.is_empty()).then(|| problems.join("; "));
    let am = entity::entity_profile::ActiveModel {
        entity_type: Set(entity_type.to_owned()),
        entity_id: Set(entity_id),
        summary: Set(profile.summary.clone()),
        sections: Set(Some(serde_json::to_value(&profile.sections)?)),
        facts: Set(Some(serde_json::to_value(&profile.facts)?)),
        provider_counts: Set(Some(serde_json::to_value(&profile.provider_counts)?)),
        sources: Set(Some(serde_json::to_value(&sources)?)),
        requested_at: sea_orm::ActiveValue::NotSet,
        fetched_at: Set(Some(now)),
        error: Set(error),
        updated_at: Set(now),
    };
    entity::entity_profile::Entity::insert(am)
        .on_conflict(
            sea_orm::sea_query::OnConflict::columns([
                entity::entity_profile::Column::EntityType,
                entity::entity_profile::Column::EntityId,
            ])
            .update_columns([
                entity::entity_profile::Column::Summary,
                entity::entity_profile::Column::Sections,
                entity::entity_profile::Column::Facts,
                entity::entity_profile::Column::ProviderCounts,
                entity::entity_profile::Column::Sources,
                entity::entity_profile::Column::FetchedAt,
                entity::entity_profile::Column::Error,
                entity::entity_profile::Column::UpdatedAt,
            ])
            .to_owned(),
        )
        .exec(db)
        .await
        .context("upsert entity profile")?;
    Ok(())
}

/// Stamp `requested_at = now` (creating the row), so a page view queues
/// one fetch and the detail view reports it as pending.
pub async fn mark_requested<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: Uuid,
    clear_error: bool,
) -> Result<(), sea_orm::DbErr> {
    let now = Utc::now().fixed_offset();
    let error_sql = if clear_error {
        "error = NULL, fetched_at = NULL,"
    } else {
        ""
    };
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        format!(
            "INSERT INTO entity_profile (entity_type, entity_id, requested_at, updated_at) \
             VALUES ($1, $2, $3, $3) \
             ON CONFLICT (entity_type, entity_id) DO UPDATE \
                SET {error_sql} requested_at = EXCLUDED.requested_at, \
                    updated_at = EXCLUDED.updated_at"
        ),
        [entity_type.into(), entity_id.into(), now.into()],
    ))
    .await?;
    Ok(())
}

// ───────── ComicVine wiki HTML → sections ─────────

/// Split ComicVine's wiki description into sections at `<h2>` / `<h3>`
/// headings, each a list of plain-text paragraphs. Tables, figures,
/// scripts and styles are dropped; every other tag is stripped and
/// entities decoded. Pure, so the unit tests pin the shape.
pub fn sections_from_html(html: &str) -> Vec<Section> {
    let html = drop_blocks(html, &["script", "style", "table", "figure"]);
    let mut sections: Vec<Section> = vec![Section::default()];
    let mut text = String::new();
    let mut heading: Option<(u8, String)> = None;
    let flush = |sections: &mut Vec<Section>, text: &mut String| {
        let t = clean_text(text);
        text.clear();
        if !t.is_empty() {
            sections.last_mut().unwrap().paragraphs.push(t);
        }
    };
    let mut rest = html.as_str();
    while let Some(lt) = rest.find('<') {
        let before = &rest[..lt];
        if let Some((_, h)) = heading.as_mut() {
            h.push_str(before);
        } else {
            text.push_str(before);
        }
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        match name.as_str() {
            "h2" | "h3" => {
                let level = if name == "h2" { 2 } else { 3 };
                if closing {
                    if let Some((lvl, h)) = heading.take() {
                        flush(&mut sections, &mut text);
                        sections.push(Section {
                            heading: clean_text(&h),
                            level: lvl,
                            paragraphs: Vec::new(),
                        });
                    }
                } else {
                    flush(&mut sections, &mut text);
                    heading = Some((level, String::new()));
                }
            }
            "p" | "li" | "div" | "ul" | "ol" | "h4" | "h5" | "blockquote" | "pre" => {
                if heading.is_none() {
                    flush(&mut sections, &mut text);
                }
            }
            "br" if heading.is_none() => text.push('\n'),
            _ => {}
        }
    }
    if let Some((_, h)) = heading.take() {
        text.push_str(&h);
    }
    text.push_str(rest);
    flush(&mut sections, &mut text);
    sections.retain(|s| !s.paragraphs.is_empty() || !s.heading.is_empty());
    // A heading with nothing under it (an empty era) adds nothing.
    sections.retain(|s| !s.paragraphs.is_empty());
    sections
}

/// Remove `<tag …>…</tag>` blocks (case-insensitive, non-nested).
fn drop_blocks(html: &str, tags: &[&str]) -> String {
    let mut out = html.to_owned();
    for tag in tags {
        let lower = out.to_lowercase();
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        let mut result = String::with_capacity(out.len());
        let mut pos = 0;
        while let Some(start) = lower[pos..].find(&open) {
            let start = pos + start;
            // `<table` must not swallow `<tablet>` or similar: the next
            // char ends the tag name.
            let after = lower[start + open.len()..].chars().next();
            if !matches!(after, Some(c) if c == '>' || c.is_whitespace() || c == '/') {
                result.push_str(&out[pos..start + open.len()]);
                pos = start + open.len();
                continue;
            }
            result.push_str(&out[pos..start]);
            match lower[start..].find(&close) {
                Some(end) => pos = start + end + close.len(),
                None => {
                    pos = out.len();
                    break;
                }
            }
        }
        result.push_str(&out[pos..]);
        out = result;
    }
    out
}

/// Decode entities and collapse whitespace (keeping `\n` from `<br>`
/// as a single line break).
fn clean_text(raw: &str) -> String {
    let decoded = decode_entities(raw);
    let mut out = String::with_capacity(decoded.len());
    let mut pending_space = false;
    let mut pending_break = false;
    for ch in decoded.chars() {
        if ch == '\n' {
            pending_break = true;
        } else if ch.is_whitespace() {
            pending_space = true;
        } else {
            if pending_break && !out.is_empty() {
                out.push('\n');
            } else if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_break = false;
            pending_space = false;
            out.push(ch);
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..semi];
        let decoded: Option<String> = match ent {
            "amp" => Some("&".into()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" => Some("'".into()),
            "nbsp" => Some(" ".into()),
            "mdash" => Some("—".into()),
            "ndash" => Some("–".into()),
            "hellip" => Some("…".into()),
            "rsquo" => Some("’".into()),
            "lsquo" => Some("‘".into()),
            "rdquo" => Some("”".into()),
            "ldquo" => Some("“".into()),
            _ => ent
                .strip_prefix('#')
                .and_then(|n| {
                    n.strip_prefix(['x', 'X'])
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| n.parse::<u32>().ok())
                })
                .and_then(char::from_u32)
                .map(|c| c.to_string()),
        };
        match decoded {
            Some(d) => {
                out.push_str(&d);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_wiki_html_into_sections() {
        let html = r#"<p>Lead text about <b>Batman</b>.</p>
<h2>Origin</h2><p>Thomas &amp; Martha Wayne&hellip;</p><p>Second&nbsp;paragraph.</p>
<table><tr><td>dropped</td></tr></table>
<h3>Crime Alley</h3><ul><li>Joe Chill</li><li>Park Row</li></ul>
<h2>Empty era</h2>
<h2>Golden Age</h2><figure><img src="x"></figure><p>Line one<br>line two</p>"#;
        let s = sections_from_html(html);
        assert_eq!(s.len(), 4, "{s:#?}");
        assert_eq!(s[0].heading, "");
        assert_eq!(s[0].level, 0);
        assert_eq!(s[0].paragraphs, vec!["Lead text about Batman."]);
        assert_eq!(s[1].heading, "Origin");
        assert_eq!(s[1].level, 2);
        assert_eq!(
            s[1].paragraphs,
            vec!["Thomas & Martha Wayne…", "Second paragraph."]
        );
        assert_eq!(s[2].heading, "Crime Alley");
        assert_eq!(s[2].level, 3);
        assert_eq!(s[2].paragraphs, vec!["Joe Chill", "Park Row"]);
        assert_eq!(s[3].heading, "Golden Age");
        assert_eq!(s[3].paragraphs, vec!["Line one\nline two"]);
    }

    #[test]
    fn plain_text_is_one_lead_paragraph() {
        let s = sections_from_html("Just   a   sentence &#39;quoted&#x27;.");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].paragraphs, vec!["Just a sentence 'quoted'."]);
        assert!(sections_from_html("   ").is_empty());
    }

    #[test]
    fn name_matching_is_exact_modulo_case_and_space() {
        assert_eq!(name_key("  Batman "), name_key("batman"));
        assert_ne!(name_key("Batman (Earth 2)"), name_key("Batman"));
    }
}
