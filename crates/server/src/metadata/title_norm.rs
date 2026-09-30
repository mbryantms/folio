//! Title sanitization for the matcher (matching-accuracy-1.0 M2).
//!
//! Ports ComicTagger's `IssueIdentifier`-style title pipeline so the
//! same input strings produce the same comparison keys both tools
//! would. Pipeline:
//!
//! 1. **NFKD normalize** — decomposes accented chars + ligatures so
//!    `Pokémon` and `Pokemon` collapse to the same base letters.
//! 2. **Casefold** — `to_lowercase()`. Handles ß / Σ / capital-Σ
//!    correctly enough for our population; full Unicode `casefold`
//!    is overkill for comic titles and would pull a second crate.
//! 3. **Quote strip** — apostrophes + curly + straight quotes are
//!    discarded entirely (not replaced) so `Spider-Man's` and
//!    `Spider-Mans` compare equal.
//! 4. **Punctuation → hyphen** — every other punct mark (colon,
//!    em-dash, period, brackets, …) becomes a single hyphen. Mirrors
//!    ComicTagger's `_sanitize_title_for_matching` which does the
//!    same so `X-Men: First Class` and `X-Men First Class` agree.
//! 5. **Article strip** — drops `&, a, am, an, and, as, at, be, but,
//!    by, for, if, is, issue, it, it's, its, itself, of, or, so,
//!    the, with` (verbatim from ComicTagger). `it's` becomes `its`
//!    after the quote-strip pass + then gets dropped by the article
//!    filter, matching the source's intent.
//! 6. **Whitespace collapse** — multiple spaces/hyphens collapse to
//!    single spaces so the comparator can split-on-whitespace safely.
//!
//! Returns a deterministic key — same input always produces the
//! same output regardless of locale.

use unicode_normalization::UnicodeNormalization;

/// Article words stripped from sanitized titles before comparison.
/// Lifted **verbatim** from ComicTagger's `IssueIdentifier` defaults.
/// Plan decision Q6: adopt as-is for M2; per-language tuning is M11
/// (skipped per user directive).
const ARTICLES: &[&str] = &[
    "&", "a", "am", "an", "and", "as", "at", "be", "but", "by", "for", "if", "is", "issue", "it",
    "it's", "its", "itself", "of", "or", "so", "the", "with",
];

/// Quote characters to discard outright (not replaced with hyphens
/// like other punct). Both straight + curly forms; the NFKD pass
/// upstream doesn't touch these because they're already canonical
/// code points.
const QUOTES: &[char] = &['\'', '"', '\u{2019}', '\u{2018}', '\u{201C}', '\u{201D}'];

/// Sanitize a comic-series / issue title down to its match key.
///
/// Output is suitable for direct equality compare OR for feeding into
/// [`crate::metadata::ratcliff::ratio`] for fuzzy similarity.
/// Idempotent: `sanitize_title(sanitize_title(x)) == sanitize_title(x)`.
pub fn sanitize_title(input: &str) -> String {
    // 1. NFKD — decomposes "é" → "e + ̀" so subsequent steps drop the
    //    combining mark naturally (it falls into the punct → hyphen
    //    branch and then dedupes with the surrounding whitespace).
    let nfkd: String = input.nfkd().collect();

    // 2. casefold via to_lowercase.
    let folded = nfkd.to_lowercase();

    // 3. discard quotes outright. Do this BEFORE the punct→hyphen
    //    pass so `Spider-Man's` becomes `spider-mans` rather than
    //    `spider-man-s`.
    let dequoted: String = folded.chars().filter(|c| !QUOTES.contains(c)).collect();

    // 4. punctuation → hyphen, plus drop combining marks the NFKD
    //    pass exposed. Keep ASCII alphanumerics, whitespace, and
    //    hyphens; everything else becomes a hyphen if punct, dropped
    //    otherwise. Unicode letters/digits outside ASCII pass through.
    let mapped: String = dequoted
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() || c == '-' {
                Some(c)
            } else if is_punct(c) {
                Some(' ')
            } else {
                // Combining marks + symbols: drop. Keeps "Pokémon" →
                // "pokemon" after NFKD strips the accent.
                None
            }
        })
        .collect();

    // 5+6. Split on whitespace + hyphens, drop articles, rejoin with
    //      a single space. Hyphens are conceptually punctuation — we
    //      want `x-men` to compare equal to `x men` since some
    //      providers use one form and some the other.
    let words: Vec<&str> = mapped
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|w| !w.is_empty())
        .filter(|w| !ARTICLES.contains(w))
        .collect();
    words.join(" ")
}

/// Heuristic punctuation predicate. `char::is_punctuation` doesn't
/// exist in std, so we approximate with the Unicode general
/// categories most relevant for English titles. Hyphens are
/// intentionally NOT classified as punct here — the calling pipeline
/// treats them as soft word boundaries.
fn is_punct(c: char) -> bool {
    matches!(
        c,
        '!' | '?'
        | '.' | ','
        | ':' | ';'
        | '/' | '\\'
        | '(' | ')'
        | '[' | ']'
        | '{' | '}'
        | '<' | '>'
        | '@' | '#' | '$' | '%' | '^' | '*'
        | '+' | '='
        | '|' | '~' | '`'
        | '_'
        // En/em dashes, ellipsis, middle dot — common in titles.
        | '\u{2013}' | '\u{2014}' | '\u{2026}' | '\u{00B7}'
    )
}

// ───────── publication format (WP-5.6) ─────────
//
// Providers and local files describe the *kind* of publication in
// different vocabularies: Metron's `series_type` ("Ongoing Series",
// "Trade Paperback", "Annual Series"), ComicInfo's free-text `Format`
// ("TPB", "Hardcover", "Annual", "Limited Series"), the scanner's
// `special_type` ("TPB", "Annual"), and — for ComicVine, which has no
// volume-type field at all — nothing but the volume's name and deck.
// The matcher only needs a coarse class to decide "is a collected
// edition being matched against a single issue?", so every vocabulary
// folds onto [`FormatClass`]. Anything unrecognised (manga volumes,
// magazines, digital chapters, specials) is `None` — unknown never
// penalises.

/// Coarse publication-format class compared by the matcher's soft
/// format penalty ([`crate::metadata::matcher::FORMAT_MISMATCH_PENALTY`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FormatClass {
    /// A periodical issue: ongoing, limited, or one-shot.
    Single,
    /// An annual (lives in its own provider series on CV and Metron).
    Annual,
    /// A collected edition: TPB, hardcover, omnibus, graphic novel.
    Collected,
}

/// Collapse a format string to lowercase words separated by single
/// spaces (hyphens / underscores / punctuation are word boundaries).
fn format_words(raw: &str) -> String {
    raw.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Map a format / series-type string from any vocabulary we ingest
/// (Metron `series_type`, ComicInfo `Format`, scanner `special_type`,
/// local `series.series_type`) onto a [`FormatClass`]. Returns `None`
/// for values that don't say single vs collected — including manga
/// (`"Manga"`, `"Tankobon"`), which is deliberately compatible with
/// both since a manga "issue" *is* a collected volume.
pub fn classify_format(raw: &str) -> Option<FormatClass> {
    match format_words(raw).as_str() {
        "series" | "ongoing" | "ongoing series" | "cancelled series" | "canceled series"
        | "limited" | "limited series" | "mini series" | "miniseries" | "maxi series"
        | "maxiseries" | "single" | "single issue" | "one shot" | "oneshot" => {
            Some(FormatClass::Single)
        }
        "annual" | "annuals" | "annual series" => Some(FormatClass::Annual),
        "tpb" | "tp" | "trade" | "trade paperback" | "hc" | "hardcover" | "hard cover"
        | "omnibus" | "graphic novel" | "gn" | "ogn" | "collected edition" | "collection"
        | "compendium" | "deluxe edition" | "library edition" => Some(FormatClass::Collected),
        _ => None,
    }
}

/// ComicInfo-vocabulary `Format` label for a Metron `series_type`
/// name, used to populate `GenericMetadata.format` on Metron details.
///
/// Ongoing / cancelled series deliberately map to `None`: that is the
/// default shape of a periodical, and writing `"Series"` into every
/// applied issue's ComicInfo would churn files for no information
/// gain. Unknown types also map to `None`.
pub fn metron_series_type_format(series_type: &str) -> Option<&'static str> {
    match format_words(series_type).as_str() {
        "limited series" => Some("Limited Series"),
        "one shot" => Some("One-Shot"),
        "annual series" | "annual" => Some("Annual"),
        "trade paperback" => Some("TPB"),
        "hard cover" | "hardcover" => Some("Hardcover"),
        "omnibus" => Some("Omnibus"),
        "graphic novel" => Some("Graphic Novel"),
        _ => None,
    }
}

/// Infer a format label (ComicInfo vocabulary) from a provider series
/// name and optional deck. ComicVine exposes no volume-type field, so
/// this is the only format signal a CV candidate carries; Metron list
/// responses omit `series_type`, so it is the fallback there too.
///
/// Conservative on purpose — only markers that essentially never
/// appear in an ongoing series' name are recognised:
/// - name words `Omnibus` / `HC` / `OGN` / `TPB` / `TP` / `Compendium`,
///   or the phrases `hardcover` / `hard cover` / `graphic novel` /
///   `trade paperback` / `collected edition` / `deluxe edition` /
///   `library edition`;
/// - a deck that opens with `Collects` / `Collecting` / `Collected`
///   (ComicVine's usual phrasing for a trade's deck);
/// - a trailing name word `Annual` → `"Annual"`.
///
/// `None` when nothing matches — which the matcher treats as unknown.
pub fn infer_format_from_title(name: &str, deck: Option<&str>) -> Option<&'static str> {
    let w = format_words(name);
    let words: Vec<&str> = w.split(' ').filter(|s| !s.is_empty()).collect();
    let has = |needle: &str| words.contains(&needle);
    let padded = format!(" {w} ");
    let phrase = |p: &str| padded.contains(&format!(" {p} "));

    if has("omnibus") {
        return Some("Omnibus");
    }
    if has("hc") || phrase("hardcover") || phrase("hard cover") {
        return Some("Hardcover");
    }
    if phrase("graphic novel") || has("ogn") {
        return Some("Graphic Novel");
    }
    if has("tpb")
        || has("tp")
        || has("compendium")
        || phrase("trade paperback")
        || phrase("collected edition")
        || phrase("deluxe edition")
        || phrase("library edition")
    {
        return Some("TPB");
    }
    if let Some(d) = deck {
        let dw = format_words(d);
        if ["collects ", "collecting ", "collected "]
            .iter()
            .any(|p| dw.starts_with(p))
        {
            return Some("TPB");
        }
    }
    if words.last() == Some(&"annual") {
        return Some("Annual");
    }
    None
}

/// True when `name` carries the word "annual" (case-insensitive, word
/// boundary). ComicVine and Metron file annuals under a separate
/// series named `"<Series> Annual"`.
pub fn has_annual_token(name: &str) -> bool {
    format_words(name).split(' ').any(|w| w == "annual")
}

/// Drop every `annual` word from a series name so `"X-Men Annual"`
/// compares equal to a local `"X-Men"` whose issue is `"Annual 1"`.
pub fn strip_annual_token(name: &str) -> String {
    name.split_whitespace()
        .filter(|w| !w.eq_ignore_ascii_case("annual"))
        .collect::<Vec<_>>()
        .join(" ")
}

// ───────── issue-number comparison key (WP-5.6) ─────────

/// Structured comparison key for an issue number. Mirrors the useful
/// part of ComicTagger's `IssueString`: a numeric value (so `"1"`,
/// `"01"`, `"1.0"` agree and `"½"` equals `"0.5"`), an alphanumeric
/// suffix kept distinct (`"14AU"` ≠ `"14"`), and an `annual` flag for
/// `"Annual 1"`-style numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct IssueNumberKey {
    /// `"Annual 1"` / `"Ann. 1"` / `"Annual #01"`.
    pub annual: bool,
    /// Numeric part, when there is one.
    pub value: Option<f64>,
    /// Uppercased alphanumeric remainder after the number (`"AU"` for
    /// `"14AU"` / `"14.AU"` / `"14 au"`); the whole uppercased token
    /// when there is no number (`"ALPHA"`).
    pub suffix: String,
}

impl IssueNumberKey {
    /// Same issue? Annual flag and suffix must agree; the numeric part
    /// must agree when both sides carry one. A key with neither number
    /// nor suffix (empty input) never matches.
    pub fn same_issue(&self, other: &Self) -> bool {
        if self.annual != other.annual || self.suffix != other.suffix {
            return false;
        }
        match (self.value, other.value) {
            (Some(a), Some(b)) => (a - b).abs() < 1e-9,
            (None, None) => !self.suffix.is_empty(),
            _ => false,
        }
    }
}

/// Strip a leading `Annual` marker (`"Annual 1"`, `"annual #01"`,
/// `"Ann. 1"`, `"Annual1"`), returning the trimmed remainder (possibly
/// empty) when present.
pub fn strip_annual_prefix(raw: &str) -> Option<&str> {
    let t = raw.trim();
    let lower = t.to_ascii_lowercase();
    let rest_at = if lower.starts_with("annual") {
        "annual".len()
    } else if lower.starts_with("ann.") {
        "ann.".len()
    } else if lower.starts_with("ann ") {
        "ann".len()
    } else {
        return None;
    };
    let after = &t[rest_at..];
    // "Annually" / "Annuals Special" are words, not a marker.
    if after
        .chars()
        .next()
        .is_some_and(|c| !(c.is_whitespace() || c == '#' || c == '.' || c.is_ascii_digit()))
    {
        return None;
    }
    let rest = after.trim_start_matches(|c: char| c.is_whitespace() || c == '#' || c == '.');
    Some(rest.trim())
}

/// Strip a leading volume marker used for manga / collected volumes:
/// `"Vol. 3"`, `"vol 03"`, `"Volume 3"`, `"v03"`. Only strips when a
/// digit follows, so words that merely start with `v` are untouched.
pub fn strip_volume_prefix(raw: &str) -> Option<&str> {
    let t = raw.trim();
    let lower = t.to_ascii_lowercase();
    for p in ["volume", "vol.", "vol", "v"] {
        if let Some(rest) = lower.strip_prefix(p) {
            let rest_trimmed =
                rest.trim_start_matches(|c: char| c.is_whitespace() || c == '#' || c == '.');
            if rest_trimmed.starts_with(|c: char| c.is_ascii_digit()) {
                // ASCII-lowercasing preserves byte offsets.
                let offset = t.len() - rest_trimmed.len();
                return Some(t[offset..].trim());
            }
        }
    }
    None
}

fn vulgar_fraction(c: char) -> Option<f64> {
    match c {
        '½' => Some(0.5),
        '¼' => Some(0.25),
        '¾' => Some(0.75),
        _ => None,
    }
}

/// Parse an issue number into its [`IssueNumberKey`]. Handles leading
/// `#`, `Annual` and volume (`Vol. 3`, `v03`) markers, zero padding,
/// decimals, negative numbers (`-1`), `1/2`, and vulgar fractions
/// (`½`, `1½`).
pub fn issue_number_key(raw: &str) -> IssueNumberKey {
    let mut s = raw.trim().trim_start_matches('#').trim();
    let mut annual = false;
    if let Some(rest) = strip_annual_prefix(s) {
        annual = true;
        s = rest;
    }
    if let Some(rest) = strip_volume_prefix(s) {
        s = rest;
    }
    let chars: Vec<char> = s.trim_start_matches('#').trim().chars().collect();

    let mut i = 0;
    let negative = chars.first() == Some(&'-') && chars.get(1).is_some_and(char::is_ascii_digit);
    if negative {
        i = 1;
    }
    let int_start = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    let mut value: Option<f64> = if i > int_start {
        chars[int_start..i].iter().collect::<String>().parse().ok()
    } else {
        None
    };
    // Decimal part ("1.5"); a dot followed by letters ("14.AU") is a
    // separator, left for the suffix pass.
    if value.is_some() && chars.get(i) == Some(&'.') {
        let start = i + 1;
        let mut j = start;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            let frac: String = chars[start..j].iter().collect();
            if let Ok(f) = format!("0.{frac}").parse::<f64>() {
                value = value.map(|v| v + f);
            }
            i = j;
        }
    }
    // Simple fraction ("1/2").
    if let Some(num) = value
        && chars.get(i) == Some(&'/')
    {
        let start = i + 1;
        let mut j = start;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        if let Ok(d) = chars[start..j].iter().collect::<String>().parse::<f64>()
            && d > 0.0
        {
            value = Some(num / d);
            i = j;
        }
    }
    // Vulgar fraction, alone ("½") or after the integer ("1½").
    if let Some(f) = chars.get(i).copied().and_then(vulgar_fraction) {
        value = Some(value.unwrap_or(0.0) + f);
        i += 1;
    }
    if negative {
        value = value.map(|v| -v);
    }

    let suffix: String = chars[i..]
        .iter()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect();
    IssueNumberKey {
        annual,
        value,
        suffix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ───────── 20 known-equivalent pairs ─────────

    #[test]
    fn equivalent_pairs_sanitize_to_same_key() {
        let pairs = [
            ("The X-Men", "X-Men"),
            ("Spider-Man", "spider man"),
            ("Saga", "SAGA"),
            ("Y: The Last Man", "Y Last Man"),
            ("X-Men: First Class", "X Men First Class"),
            ("Batman, Inc.", "Batman Inc"),
            ("Pokémon", "Pokemon"),
            // `Æ` (U+00C6) is its own letter in Unicode — NFKD has no
            // decomposition mapping for it (only compat-ligatures like
            // `ﬁ` decompose), so we don't promise equivalence between
            // `Æon Flux` and `AEon Flux`. That's a known asymmetry +
            // matches Python's `unicodedata.normalize('NFKD', 'Æ')`.
            ("Sandman: Overture", "Sandman Overture"),
            // `in` is intentionally NOT on the ComicTagger article
            // list (only 23 specific words; this pair is here to lock
            // that asymmetry — `in` matters for disambiguation).
            ("Hellboy & Friends", "Hellboy Friends"),
            ("A Game of Thrones", "Game of Thrones"),
            ("An American Tail", "American Tail"),
            ("Spider-Man's Daily Bugle", "spider mans daily bugle"),
            ("DC: The New Frontier", "DC New Frontier"),
            (
                "Star Wars: Knights of the Old Republic",
                "Star Wars Knights Old Republic",
            ),
            ("100 Bullets", "100 bullets"),
            // `issue` article stripped from both sides — the number
            // stays so this only equates when both carry one.
            ("Issue 1", "1"),
            ("Wonder Woman", "wonder-woman"),
            ("Spider-Man (2099)", "Spider-Man 2099"),
            ("Daredevil — Yellow", "Daredevil Yellow"),
        ];
        for (a, b) in pairs {
            let sa = sanitize_title(a);
            let sb = sanitize_title(b);
            assert_eq!(sa, sb, "expected {a:?} ≡ {b:?}; got {sa:?} vs {sb:?}");
        }
    }

    // ───────── 20 known-distinct pairs ─────────

    #[test]
    fn distinct_pairs_sanitize_to_different_keys() {
        let pairs = [
            ("Aquaman", "Aquaman: The Becoming"),
            ("Batman", "Batman Beyond"),
            ("Superman", "Super Sons"),
            ("X-Men", "X-Force"),
            ("Avengers", "New Avengers"),
            ("Saga", "Saga of the Swamp Thing"),
            ("Sandman", "Sandman Mystery Theatre"),
            ("Robin", "Tim Drake Robin"),
            ("Spider-Man", "Spider-Gwen"),
            ("Flash", "Flash Forward"),
            ("Detective Comics", "Action Comics"),
            ("Hellboy", "BPRD"),
            ("Watchmen", "Doomsday Clock"),
            ("Daredevil", "Echo"),
            ("Wonder Woman", "Wonder Girl"),
            ("Catwoman", "Cat Woman"),
            ("Iron Man", "Iron Fist"),
            ("Captain America", "Captain Marvel"),
            ("Justice League", "Justice Society"),
            ("Thor", "Mighty Thor"),
        ];
        for (a, b) in pairs {
            let sa = sanitize_title(a);
            let sb = sanitize_title(b);
            assert_ne!(
                sa, sb,
                "expected {a:?} to differ from {b:?}; both sanitized to {sa:?}"
            );
        }
    }

    // ───────── NFKD edge cases ─────────

    #[test]
    fn nfkd_strips_accents_and_decomposes_ligatures() {
        assert_eq!(sanitize_title("Café"), "cafe");
        assert_eq!(sanitize_title("naïve"), "naive");
        assert_eq!(sanitize_title("résumé"), "resume");
        assert_eq!(sanitize_title("Pokémon Pikachu"), "pokemon pikachu");
        // ﬁ ligature (U+FB01) → "fi"
        assert_eq!(sanitize_title("ﬁre"), "fire");
    }

    // ───────── article-strip edge cases ─────────

    #[test]
    fn article_strip_drops_full_list() {
        assert_eq!(sanitize_title("The Saga"), "saga");
        assert_eq!(sanitize_title("A New Hope"), "new hope");
        assert_eq!(sanitize_title("An American Werewolf"), "american werewolf");
        // Issue word stripped — same intent as ComicTagger.
        assert_eq!(sanitize_title("Issue 1"), "1");
        // Internal article also stripped (matches ComicTagger).
        assert_eq!(sanitize_title("Lord of the Rings"), "lord rings");
        // "It's" → quotes stripped → "its" → article-stripped to empty.
        // Word should disappear; rest of title stays.
        assert_eq!(sanitize_title("It's a Wonderful Life"), "wonderful life",);
    }

    #[test]
    fn quotes_are_discarded_not_replaced_with_hyphens() {
        // Curly + straight forms both → empty.
        assert_eq!(sanitize_title("Spider-Man\u{2019}s Web"), "spider mans web");
        assert_eq!(sanitize_title("\"Quoted\" Hero"), "quoted hero");
        assert_eq!(sanitize_title("\u{201C}Heavy\u{201D} Metal"), "heavy metal");
    }

    #[test]
    fn punctuation_becomes_word_boundary() {
        assert_eq!(sanitize_title("X-Men: First Class"), "x men first class");
        assert_eq!(sanitize_title("Batman/Superman"), "batman superman");
        assert_eq!(sanitize_title("Spider-Man (2099)"), "spider man 2099");
        assert_eq!(sanitize_title("Daredevil — Yellow"), "daredevil yellow");
    }

    #[test]
    fn idempotent_on_already_sanitized_input() {
        let inputs = [
            "saga",
            "x men first class",
            "spider man 2099",
            "wonder woman",
        ];
        for s in inputs {
            assert_eq!(sanitize_title(s), s);
            assert_eq!(sanitize_title(&sanitize_title(s)), sanitize_title(s));
        }
    }

    #[test]
    fn empty_and_pure_punct_inputs_yield_empty() {
        assert_eq!(sanitize_title(""), "");
        assert_eq!(sanitize_title("   "), "");
        assert_eq!(sanitize_title("!!!"), "");
        assert_eq!(sanitize_title("the of and"), "");
    }

    // ───────── WP-5.6: format classification ─────────

    #[test]
    fn classify_format_folds_every_vocabulary() {
        use FormatClass::*;
        // Metron series_type names.
        assert_eq!(classify_format("Ongoing Series"), Some(Single));
        assert_eq!(classify_format("Limited Series"), Some(Single));
        assert_eq!(classify_format("One-Shot"), Some(Single));
        assert_eq!(classify_format("Cancelled Series"), Some(Single));
        assert_eq!(classify_format("Annual Series"), Some(Annual));
        assert_eq!(classify_format("Trade Paperback"), Some(Collected));
        assert_eq!(classify_format("Hard Cover"), Some(Collected));
        assert_eq!(classify_format("Omnibus"), Some(Collected));
        assert_eq!(classify_format("Graphic Novel"), Some(Collected));
        // ComicInfo Format / scanner special_type.
        assert_eq!(classify_format("TPB"), Some(Collected));
        assert_eq!(classify_format("hardcover"), Some(Collected));
        assert_eq!(classify_format("Series"), Some(Single));
        assert_eq!(classify_format("ongoing"), Some(Single));
        assert_eq!(classify_format("Annual"), Some(Annual));
        // Unknown / deliberately neutral.
        assert_eq!(classify_format("Manga"), None);
        assert_eq!(classify_format("Digital Chapters"), None);
        assert_eq!(classify_format("Magazine"), None);
        assert_eq!(classify_format("Special"), None);
        assert_eq!(classify_format(""), None);
    }

    #[test]
    fn metron_series_type_maps_to_comicinfo_format() {
        assert_eq!(metron_series_type_format("Trade Paperback"), Some("TPB"));
        assert_eq!(metron_series_type_format("Hard Cover"), Some("Hardcover"));
        assert_eq!(metron_series_type_format("Annual Series"), Some("Annual"));
        assert_eq!(metron_series_type_format("One-Shot"), Some("One-Shot"));
        assert_eq!(
            metron_series_type_format("Limited Series"),
            Some("Limited Series")
        );
        // Ongoing is the default periodical shape — not written.
        assert_eq!(metron_series_type_format("Ongoing Series"), None);
        assert_eq!(metron_series_type_format("Cancelled Series"), None);
        assert_eq!(metron_series_type_format("Something New"), None);
    }

    #[test]
    fn infer_format_from_title_recognises_collected_markers_only() {
        assert_eq!(infer_format_from_title("Saga TPB", None), Some("TPB"));
        assert_eq!(
            infer_format_from_title("Batman: Hush (HC)", None),
            Some("Hardcover")
        );
        assert_eq!(
            infer_format_from_title("Absolute Carnage Omnibus", None),
            Some("Omnibus")
        );
        assert_eq!(
            infer_format_from_title("Maus: A Survivor's Tale Graphic Novel", None),
            Some("Graphic Novel")
        );
        assert_eq!(
            infer_format_from_title("Saga", Some("Collects issues #1-6 of the series.")),
            Some("TPB")
        );
        assert_eq!(
            infer_format_from_title("X-Men Annual", None),
            Some("Annual")
        );
        // Ongoing names stay unknown — no false positives on words that
        // merely contain a marker ("Hawkeye", "Top 10", "Absolute Batman").
        assert_eq!(infer_format_from_title("Saga", None), None);
        assert_eq!(infer_format_from_title("Hawkeye", None), None);
        assert_eq!(infer_format_from_title("Top 10", None), None);
        assert_eq!(infer_format_from_title("Absolute Batman", None), None);
        assert_eq!(infer_format_from_title("Annual Report Comics", None), None);
        assert_eq!(
            infer_format_from_title("Saga", Some("A sci-fi epic about a family.")),
            None
        );
    }

    #[test]
    fn annual_token_helpers() {
        assert!(has_annual_token("X-Men Annual"));
        assert!(has_annual_token("Amazing Spider-Man Annual (2018)"));
        assert!(!has_annual_token("X-Men"));
        assert!(!has_annual_token("Annually Yours"));
        assert_eq!(strip_annual_token("X-Men Annual"), "X-Men");
        assert_eq!(strip_annual_token("X-Men"), "X-Men");
    }

    // ───────── WP-5.6: issue-number key ─────────

    fn key(raw: &str) -> IssueNumberKey {
        issue_number_key(raw)
    }

    #[test]
    fn issue_number_key_equates_padding_decimals_and_fractions() {
        assert!(key("1").same_issue(&key("01")));
        assert!(key("1").same_issue(&key("1.0")));
        assert!(key("#1").same_issue(&key("1")));
        assert!(key("½").same_issue(&key("0.5")));
        assert!(key("½").same_issue(&key("1/2")));
        assert!(key("1½").same_issue(&key("1.5")));
        assert!(key("-1").same_issue(&key("-1")));
        assert!(!key("-1").same_issue(&key("1")));
        assert!(!key("1").same_issue(&key("2")));
        assert!(!key("1.5").same_issue(&key("1")));
    }

    #[test]
    fn issue_number_key_keeps_suffix_distinct() {
        assert!(key("14AU").same_issue(&key("14.AU")));
        assert!(key("14AU").same_issue(&key("014au")));
        assert!(key("14AU").same_issue(&key("14 AU")));
        assert!(!key("14AU").same_issue(&key("14")));
        assert!(!key("1A").same_issue(&key("1B")));
        assert!(key("Alpha").same_issue(&key("ALPHA")));
        assert!(!key("").same_issue(&key("")));
    }

    #[test]
    fn issue_number_key_parses_annual_and_volume_markers() {
        let a = key("Annual 1");
        assert!(a.annual);
        assert_eq!(a.value, Some(1.0));
        assert!(a.same_issue(&key("annual #01")));
        assert!(a.same_issue(&key("Ann. 1")));
        assert!(a.same_issue(&key("Annual1")));
        // Annual 1 is not regular #1.
        assert!(!a.same_issue(&key("1")));
        // Year-numbered annuals.
        assert!(key("Annual 2019").same_issue(&key("Annual 2019")));
        // Manga / collected volume markers.
        assert!(key("Vol. 3").same_issue(&key("3")));
        assert!(key("v03").same_issue(&key("3")));
        assert!(key("Volume 03").same_issue(&key("3")));
        assert!(key("vol 3").same_issue(&key("3")));
        // A word that starts with v is not a volume marker.
        assert_eq!(key("Venom").suffix, "VENOM");
        assert_eq!(key("Venom").value, None);
    }
}
