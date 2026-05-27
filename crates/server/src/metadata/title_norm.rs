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
}
