//! HTTP client for Mylar's API, limited to what add-to-Mylar needs.
//!
//! Mylar's API key is all-or-nothing on Mylar's side, so the limit is
//! held here: [`Command`] is a closed enum and the only way to send a
//! request. Adding a variant is a review-time decision, pinned by
//! `crates/server/tests/upcoming_view_only.rs`; nothing that deletes a
//! series, reconfigures Mylar or stops it belongs here.
//!
//! The key travels as a query parameter (Mylar accepts nothing else), so
//! redirects are refused — a bounce would carry it to another host — and
//! transport errors are logged without their URL.

use crate::config::Config;
use reqwest::redirect::Policy;
use std::time::Duration;
use url::Url;

const USER_AGENT: &str = concat!("folio/", env!("CARGO_PKG_VERSION"));
/// `addComic` only queues the import, so Mylar answers quickly; a large
/// `getComic` is the slow one.
const TIMEOUT: Duration = Duration::from_secs(30);
/// `forceSearch` answers only when the whole search is over, which can
/// take many minutes. Long enough to hear a refusal, then stop waiting.
const SEARCH_WAIT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// `getComic` returns every issue of the series.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The Mylar API commands Folio may send. Nothing else is reachable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Connection + key check.
    GetVersion,
    /// Is this ComicVine volume on the watchlist?
    GetComic,
    /// Add a ComicVine volume to the watchlist (Mylar queues the import).
    AddComic,
    /// One issue's status; fails until Mylar has imported its series.
    GetIssueInfo,
    /// Mark one issue Wanted.
    QueueIssue,
    /// Set one Wanted issue back to Skipped.
    UnqueueIssue,
    /// Every issue Mylar currently wants.
    GetWanted,
    /// Search the indexers for the whole wanted list now.
    ForceSearch,
    /// Rescan one series' folder for files Mylar has not noticed.
    RecheckFiles,
    /// Re-read one series from ComicVine (new issues, changed dates).
    RefreshComic,
}

impl Command {
    pub const ALL: [Command; 10] = [
        Command::GetVersion,
        Command::GetComic,
        Command::AddComic,
        Command::GetIssueInfo,
        Command::QueueIssue,
        Command::UnqueueIssue,
        Command::GetWanted,
        Command::ForceSearch,
        Command::RecheckFiles,
        Command::RefreshComic,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Command::GetVersion => "getVersion",
            Command::GetComic => "getComic",
            Command::AddComic => "addComic",
            Command::GetIssueInfo => "getIssueInfo",
            Command::QueueIssue => "queueIssue",
            Command::UnqueueIssue => "unqueueIssue",
            Command::GetWanted => "getWanted",
            Command::ForceSearch => "forceSearch",
            Command::RecheckFiles => "recheckFiles",
            Command::RefreshComic => "refreshComic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MylarError {
    /// No answer: DNS, connect, TLS.
    #[error("could not reach Mylar ({0})")]
    Unreachable(String),
    /// Mylar took the request and did not answer in time. For the
    /// commands Mylar runs to completion before answering, the work may
    /// still be going on.
    #[error("Mylar did not answer in time")]
    TimedOut,
    #[error("Mylar answered HTTP {0}")]
    Http(u16),
    /// Mylar answered and said no (bad key, unknown id, …).
    #[error("Mylar refused: {0}")]
    Rejected(String),
    #[error("Mylar sent an answer Folio could not read")]
    Unexpected,
}

impl MylarError {
    /// Worth trying again later without counting against the request.
    pub fn is_transient(&self) -> bool {
        match self {
            MylarError::Unreachable(_) | MylarError::TimedOut => true,
            MylarError::Http(code) => *code >= 500,
            MylarError::Rejected(_) | MylarError::Unexpected => false,
        }
    }
}

/// Validate an operator-supplied Mylar base URL and return it with a
/// trailing slash. http(s) only, no credentials, query or fragment.
pub fn parse_base_url(raw: &str) -> Result<Url, String> {
    let mut url = Url::parse(raw.trim()).map_err(|_| "not a valid URL".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("must start with http:// or https://".to_owned());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("has no host".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("must not contain a username or password".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("must not contain a query or fragment".to_owned());
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

#[derive(Clone)]
pub struct MylarClient {
    http: reqwest::Client,
    api_url: Url,
    key: String,
}

impl MylarClient {
    /// `None` unless both the URL and the key are stored (and the URL
    /// parses). That pair is what turns add-to-Mylar on.
    pub fn from_cfg(cfg: &Config) -> Option<Self> {
        let url = cfg.integrations_mylar_url.as_deref()?;
        let key = cfg.integrations_mylar_api_key.as_deref()?;
        Self::new(url, key).ok()
    }

    pub fn new(base_url: &str, key: &str) -> Result<Self, String> {
        let base = parse_base_url(base_url)?;
        let api_url = base.join("api").map_err(|_| "not a valid URL".to_owned())?;
        if key.trim().is_empty() {
            return Err("the API key is empty".to_owned());
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::none())
            .build()
            .map_err(|_| "could not build the HTTP client".to_owned())?;
        Ok(Self {
            http,
            api_url,
            key: key.trim().to_owned(),
        })
    }

    async fn call(&self, cmd: Command, id: Option<i64>) -> Result<Answer, MylarError> {
        self.call_within(cmd, id, TIMEOUT).await
    }

    async fn call_within(
        &self,
        cmd: Command,
        id: Option<i64>,
        wait: Duration,
    ) -> Result<Answer, MylarError> {
        let mut query: Vec<(&str, String)> = vec![
            ("apikey", self.key.clone()),
            ("cmd", cmd.as_str().to_owned()),
        ];
        if let Some(id) = id {
            query.push(("id", id.to_string()));
        }
        let resp = self
            .http
            .get(self.api_url.clone())
            .query(&query)
            .timeout(wait)
            .send()
            .await
            .map_err(|e| transport_error(&e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(MylarError::Http(status.as_u16()));
        }
        let mut body: Vec<u8> = Vec::new();
        let mut resp = resp;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > MAX_BODY_BYTES {
                        return Err(MylarError::Unexpected);
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(e) => return Err(transport_error(&e)),
            }
        }
        let text = String::from_utf8_lossy(&body).trim().to_owned();
        Ok(match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(json) => Answer::Json(json),
            Err(_) => Answer::Text(text),
        })
    }

    /// Mylar's version string; proves the URL and the key.
    pub async fn version(&self) -> Result<String, MylarError> {
        let json = self.call(Command::GetVersion, None).await?.into_ok_json()?;
        let data = &json["data"];
        let version = data["current_version"]
            .as_str()
            .or_else(|| data["latest_version"].as_str())
            .or_else(|| data.as_str())
            .unwrap_or("unknown");
        Ok(version.chars().take(40).collect())
    }

    /// Whether the ComicVine volume is already on Mylar's watchlist.
    pub async fn has_series(&self, cv_series_id: i64) -> Result<bool, MylarError> {
        let json = self
            .call(Command::GetComic, Some(cv_series_id))
            .await?
            .into_ok_json()?;
        Ok(json["data"]["comic"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()))
    }

    /// Queue the ComicVine volume for import into the watchlist.
    pub async fn add_series(&self, cv_series_id: i64) -> Result<(), MylarError> {
        self.call(Command::AddComic, Some(cv_series_id))
            .await?
            .into_ok_json()
            .map(|_| ())
    }

    /// The issue's Mylar status (`Wanted`, `Skipped`, …); `None` while
    /// Mylar does not know the issue (its series is not imported yet).
    pub async fn issue_status(&self, cv_issue_id: i64) -> Result<Option<String>, MylarError> {
        let answer = self.call(Command::GetIssueInfo, Some(cv_issue_id)).await?;
        let Answer::Json(json) = answer else {
            return Err(MylarError::Unexpected);
        };
        if !succeeded(&json) {
            return Ok(None);
        }
        Ok(json["data"][0]["status"].as_str().map(str::to_owned))
    }

    /// Mark the issue Wanted. Mylar answers with the plain text `OK`.
    pub async fn queue_issue(&self, cv_issue_id: i64) -> Result<(), MylarError> {
        match self.call(Command::QueueIssue, Some(cv_issue_id)).await? {
            Answer::Text(t) if t == "OK" => Ok(()),
            Answer::Json(json) if succeeded(&json) => Ok(()),
            Answer::Json(json) => Err(MylarError::Rejected(rejection(&json))),
            Answer::Text(_) => Err(MylarError::Unexpected),
        }
    }

    /// Set a Wanted issue back to Skipped. Mylar writes the row whether
    /// or not it knows the issue, so call this only for an issue
    /// [`Self::issue_status`] reports as `Wanted`.
    pub async fn unqueue_issue(&self, cv_issue_id: i64) -> Result<(), MylarError> {
        match self.call(Command::UnqueueIssue, Some(cv_issue_id)).await? {
            Answer::Text(t) if t == "OK" => Ok(()),
            Answer::Json(json) if succeeded(&json) => Ok(()),
            Answer::Json(json) if json.get("error").is_some() => {
                Err(MylarError::Rejected(rejection(&json)))
            }
            _ => Err(MylarError::Unexpected),
        }
    }

    /// Every issue Mylar wants, annuals included. Rows without numeric
    /// ComicVine ids (one-offs Mylar tracks by another key) are left out.
    pub async fn wanted(&self) -> Result<Vec<WantedIssue>, MylarError> {
        let Answer::Json(json) = self.call(Command::GetWanted, None).await? else {
            return Err(MylarError::Unexpected);
        };
        if json.get("success").is_some() && !succeeded(&json) {
            return Err(MylarError::Rejected(rejection(&json)));
        }
        let Some(issues) = json["issues"].as_array() else {
            return Err(MylarError::Unexpected);
        };
        let annuals = json["annuals"].as_array();
        Ok(issues
            .iter()
            .chain(annuals.into_iter().flatten())
            .filter_map(|row| {
                Some(WantedIssue {
                    cv_series_id: id_of(&row["ComicID"])?,
                    cv_issue_id: id_of(&row["IssueID"])?,
                    series: row["ComicName"].as_str().unwrap_or_default().to_owned(),
                    issue_number: row["Issue_Number"].as_str().unwrap_or_default().to_owned(),
                })
            })
            .collect())
    }

    /// Search the indexers for the whole wanted list. Mylar answers only
    /// when the search is over, so not hearing back in time means it is
    /// running, not that it failed.
    pub async fn force_search(&self) -> Result<Progress, MylarError> {
        match self
            .call_within(Command::ForceSearch, None, SEARCH_WAIT)
            .await
        {
            Ok(Answer::Json(json)) if json.get("error").is_some() => {
                Err(MylarError::Rejected(rejection(&json)))
            }
            Ok(_) => Ok(Progress::Done),
            Err(MylarError::TimedOut) => Ok(Progress::Started),
            Err(e) => Err(e),
        }
    }

    /// Rescan one series' folder. Mylar does nothing, silently, for a
    /// series it does not have: check [`Self::has_series`] first. It
    /// answers when the rescan is over (an empty body), so a timeout
    /// means still running.
    pub async fn recheck_files(&self, cv_series_id: i64) -> Result<Progress, MylarError> {
        match self.call(Command::RecheckFiles, Some(cv_series_id)).await {
            Ok(Answer::Json(json)) if json.get("error").is_some() => {
                Err(MylarError::Rejected(rejection(&json)))
            }
            Ok(_) => Ok(Progress::Done),
            Err(MylarError::TimedOut) => Ok(Progress::Started),
            Err(e) => Err(e),
        }
    }

    /// Queue a re-read of one series from ComicVine.
    ///
    /// Check [`Self::has_series`] first. Mylar (as of 2026-11) queues the
    /// refresh and then fails while formatting its own success message,
    /// which reaches us as a 5xx; for a series Mylar is known to have,
    /// that is read as queued.
    pub async fn refresh_series(&self, cv_series_id: i64) -> Result<(), MylarError> {
        match self.call(Command::RefreshComic, Some(cv_series_id)).await {
            Ok(Answer::Json(json)) if succeeded(&json) => Ok(()),
            Ok(Answer::Json(json)) if json.get("error").is_some() => {
                Err(MylarError::Rejected(rejection(&json)))
            }
            Ok(_) => Ok(()),
            Err(MylarError::Http(code)) if code >= 500 => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// One issue on Mylar's wanted list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WantedIssue {
    pub cv_series_id: i64,
    pub cv_issue_id: i64,
    pub series: String,
    pub issue_number: String,
}

/// How far a command that Mylar runs before answering got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Mylar answered: the work is finished (or queued on Mylar's side).
    Done,
    /// Mylar took the request and is still working.
    Started,
}

enum Answer {
    Json(serde_json::Value),
    Text(String),
}

impl Answer {
    /// A JSON answer whose `success` is true.
    fn into_ok_json(self) -> Result<serde_json::Value, MylarError> {
        match self {
            Answer::Json(json) if succeeded(&json) => Ok(json),
            Answer::Json(json) if json.get("success").is_some() || json.get("error").is_some() => {
                Err(MylarError::Rejected(rejection(&json)))
            }
            _ => Err(MylarError::Unexpected),
        }
    }
}

/// Mylar sends `"success": true`; some builds send the string.
fn succeeded(json: &serde_json::Value) -> bool {
    match &json["success"] {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::String(s) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn rejection(json: &serde_json::Value) -> String {
    let err = &json["error"];
    let msg = err["message"]
        .as_str()
        .or_else(|| err.as_str())
        .unwrap_or("no reason given");
    msg.chars().take(200).collect()
}

/// An error with no URL in it: the URL carries the API key.
fn transport_error(e: &reqwest::Error) -> MylarError {
    if e.is_timeout() && !e.is_connect() {
        return MylarError::TimedOut;
    }
    let reason = if e.is_connect() {
        "connection failed"
    } else if e.is_redirect() {
        "redirected"
    } else {
        "request failed"
    };
    MylarError::Unreachable(reason.to_owned())
}

/// Mylar stores its ids as text; some answers carry them as numbers.
fn id_of(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_gets_a_trailing_slash_and_keeps_a_sub_path() {
        assert_eq!(
            parse_base_url("https://mylar.example").unwrap().as_str(),
            "https://mylar.example/"
        );
        let c = MylarClient::new("http://10.0.0.5:8090/mylar", "k").unwrap();
        assert_eq!(c.api_url.as_str(), "http://10.0.0.5:8090/mylar/api");
    }

    #[test]
    fn base_url_refuses_other_schemes_credentials_and_queries() {
        for bad in [
            "ftp://mylar.example/",
            "mylar.example",
            "http://user:pw@mylar.example/",
            "http://mylar.example/?apikey=1",
            "http://mylar.example/#x",
            "",
        ] {
            assert!(parse_base_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn success_reads_bool_and_string() {
        assert!(succeeded(&serde_json::json!({"success": true})));
        assert!(succeeded(&serde_json::json!({"success": "True"})));
        assert!(!succeeded(&serde_json::json!({"success": false})));
        assert!(!succeeded(&serde_json::json!({})));
    }

    #[test]
    fn only_transport_faults_and_5xx_are_transient() {
        assert!(MylarError::Unreachable("connection failed".into()).is_transient());
        assert!(MylarError::TimedOut.is_transient());
        assert!(MylarError::Http(502).is_transient());
        assert!(!MylarError::Http(401).is_transient());
        assert!(!MylarError::Rejected("no".into()).is_transient());
    }
}
