//! Post-recognition text cleanup.
//!
//! This module is the single deterministic cleanup pass, run by the
//! pipeline right after recognition (see
//! [`crate::ocr::pipeline::run_ocr`]). Which rules run depends on the
//! engine that produced the text ([`clean_for`]):
//!
//! - **CTC line recogniser** (Western): only the
//!   rules that do not depend on an engine's mistakes. The recogniser
//!   returns one string per text line; the lines are joined into one
//!   utterance with the line-end hyphen rule (W2 below) and single
//!   spaces (W6). Nothing is dropped, and the only character ever
//!   rewritten is a U+2010 hyphen at a W2 join, which becomes `-`: the
//!   debris rules below were written for what Tesseract makes of balloon
//!   borders, and on this recogniser's output they would delete real
//!   text (a `|`, a low-scored word, a symbol outside the allowlist).
//!   The confidence is the recogniser's.
//! - **manga-ocr** (Japanese): control-character stripping only (see
//!   below).
//! - **Tesseract**: the full W chain ([`clean`] with
//!   [`Language::Western`]). No request reaches it any more; it stays for
//!   `crates/tools/ocr-bench`, whose Tesseract baseline uses it, until the
//!   Tesseract build is removed.
//!
//! Raw Tesseract output from comic lettering carries OCR debris: bubble
//! borders, tails, and halftone dots read back as stray `| ~ ' _`
//! symbols, and Tesseract keeps low-confidence guesses in the text.
//!
//! Tesseract rule chain, in order (W-numbers referenced from tests):
//!
//! - **W1** word-confidence drop — needs per-word data; skipped when
//!   the recognizer returned `words: None`.
//! - **W1b** pronoun `I` read as `|` (Tesseract's usual misreading of
//!   a sans-serif capital I): a token that is exactly `|` with a word
//!   on both sides *on the same line* becomes `I`, and a token that
//!   starts `|'M` / `|'LL` / `|'VE` / `|'D` (straight or curly
//!   apostrophe, any case, optionally followed by punctuation) becomes
//!   `I'M` / … A `|` at a line edge stays debris (that is where
//!   balloon borders sit), and so does any `|` glued to other symbols
//!   (`|~`, `||`, `|_`).
//! - **W2** line-end hyphen join without deleting the hyphen: a line
//!   ending in a word + exactly one hyphen joins the next line's first
//!   token when it starts with a letter (`SPIDER-\nMAN` →
//!   `SPIDER-MAN`). A double dash is an interruption and is kept as
//!   written, with the next word after a space (`WAIT--\nNO` →
//!   `WAIT-- NO`). Accepted cost: a word the letterer really split
//!   across lines keeps its hyphen (`WON-\nDERFUL` → `WON-DERFUL`);
//!   comic lettering rarely splits words, and deleting the hyphen
//!   broke every compound name that wrapped.
//! - **W3** symbol-only token drop, sparing expressive punctuation
//!   (`!?`, `...`) and symbol swearing (grawlix: a run of three or more
//!   of `# @ $ % & * ! ?` with at least two different of `# @ $ % & *`,
//!   e.g. `#@$%!`) whose confidence is at least [`GRAWLIX_MIN_CONF`].
//! - **W4** edge junk strip per token (`|HELLO!` → `HELLO!`); a kept
//!   grawlix token is not stripped.
//! - **W5** per-char allowlist (letters, digits, common punctuation).
//! - **W6** whitespace collapse — one bubble is one utterance.
//! - **W7** empty-out guard — cleaning a non-empty raw down to
//!   nothing reports confidence 0.0 so the client's "couldn't read
//!   text" path fires.
//! - **W8** confidence recompute over the *kept* words.
//!
//! The manga path applies almost none of this: Japanese has no
//! whitespace tokenization, and `。、「」！？ー…` are all legitimate
//! output. It only strips control/zero-width/replacement characters
//! and collapses whitespace runs.
//!
//! Thresholds and charsets are hardcoded consts, mirroring the
//! matching engine's Hamming-ladder precedent: consts guarded by
//! tests now, promotion to the settings registry only if operator
//! demand materializes. A tunable threshold would also have to
//! participate in the OCR result-cache key — bump
//! [`crate::ocr::cache::OCR_RESULT_VERSION`] when changing anything
//! in this module.

use super::recognizer::{Language, Recognition, Word};

/// W1: words below this confidence are dropped outright.
pub const WORD_MIN_CONF: f32 = 0.35;

/// W1: words below this confidence that contain no alphanumeric
/// character are dropped — a low-confidence symbol cluster is
/// almost always a bubble-border artifact, while low-confidence
/// *letters* still deserve the benefit of the doubt up to
/// [`WORD_MIN_CONF`].
pub const SYMBOL_WORD_MIN_CONF: f32 = 0.60;

/// W3: a token with no alphanumerics survives only if every char is
/// in this set — keeps legitimate `!?`, `...`, `!!` bubbles.
const EXPRESSIVE: &str = ".,!?…;:'\"()-‘’“”";

/// W4: stripped from token edges. Interior occurrences are left
/// alone (`don't`, `e-mail`). Quotes are deliberately absent —
/// quoted dialogue is real text.
const HARD_JUNK: &str = "|~_^`´·•\\/{}[]<>=+*#@";

/// W5: non-alphanumeric chars kept by the per-char allowlist.
const KEEP_PUNCT: &str = ".,!?…;:'\"()&%$#@*/+=°–—-‘’“”¢£¥€";

/// W1b: contraction endings after which a leading `|` is the pronoun
/// (`|'M` → `I'M`). Compared case-insensitively.
const I_CONTRACTIONS: [&str; 4] = ["M", "LL", "VE", "D"];

/// W3: characters a grawlix (symbol swearing, `#@$%!`) is made of.
const GRAWLIX: &str = "#@$%&*!?";

/// W3: a grawlix needs at least two different of these, so `!!!` (kept
/// anyway as expressive punctuation) and a run of one repeated symbol
/// (`***`, halftone or border debris) do not count.
const GRAWLIX_CORE: &str = "#@$%&*";

/// W3: shortest grawlix kept.
const GRAWLIX_MIN_LEN: usize = 3;

/// W3: a grawlix is kept only at this confidence or above (the word's
/// own score, or the whole recognition's when the engine returned no
/// per-word data). Same bar W1 sets for any symbol-only word.
pub const GRAWLIX_MIN_CONF: f32 = SYMBOL_WORD_MIN_CONF;

/// Output of [`clean`]. `raw_text` is `Some` only when cleaning
/// changed the text — a debugging and golden-fixture-authoring aid.
#[derive(Debug, Clone)]
pub struct Cleaned {
    pub text: String,
    pub confidence: f32,
    pub words: Option<Vec<Word>>,
    pub raw_text: Option<String>,
}

/// Entry point for the pipeline: the cleanup that fits the engine that
/// read `rec`. Western text (the CTC line recogniser) gets
/// [`clean_lines`]; Japanese gets the manga rules of [`clean`].
pub fn clean_for(rec: Recognition, lang: Language) -> Cleaned {
    match lang {
        Language::Western => clean_lines(rec),
        Language::Manga => clean(rec, lang),
    }
}

/// Cleanup for the CTC line recogniser: join the lines into one
/// utterance. A line ending in a word and one hyphen joins the next
/// line's first word (W2: `SPACE-` / `BUSTERS` → `SPACE-BUSTERS`); every
/// other line break and every run of whitespace becomes one space (W6).
/// No token is dropped, and the confidence is kept. One character can
/// change: at such a join a typographic hyphen (U+2010) ending the line
/// is written as the ASCII `-`, as W2 always did. Nothing else is
/// rewritten. The line entries in `words` keep their own text, with
/// whitespace collapsed.
pub fn clean_lines(rec: Recognition) -> Cleaned {
    let raw = rec.text;
    let lines: Vec<Vec<Token>> = raw
        .lines()
        .map(|l| {
            l.split_whitespace()
                .map(|t| Token {
                    text: t.to_owned(),
                    word: None,
                })
                .collect::<Vec<Token>>()
        })
        .filter(|l| !l.is_empty())
        .collect();
    let text = join_hyphenated(lines)
        .iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let words = rec
        .words
        .map(|lines| {
            lines
                .into_iter()
                .filter_map(|mut line| {
                    line.text = line.text.split_whitespace().collect::<Vec<_>>().join(" ");
                    (!line.text.is_empty()).then_some(line)
                })
                .collect::<Vec<Word>>()
        })
        .filter(|lines| !lines.is_empty());
    let raw_text = (!raw.is_empty() && raw != text).then_some(raw);
    Cleaned {
        text,
        confidence: rec.confidence,
        words,
        raw_text,
    }
}

/// Language-aware cleanup with the Tesseract rule chain for Western text
/// and the manga rules for Japanese. The pipeline calls it for Japanese
/// only ([`clean_for`]); the Tesseract chain is kept for
/// `crates/tools/ocr-bench` until the Tesseract build is removed.
pub fn clean(rec: Recognition, lang: Language) -> Cleaned {
    let raw = rec.text.clone();
    let (text, confidence, words) = match lang {
        Language::Manga => {
            let cleaned = clean_manga(&rec.text);
            // W7 analogue: stripping a non-empty raw to nothing
            // means the output was pure debris.
            let confidence = if cleaned.is_empty() && !raw.trim().is_empty() {
                0.0
            } else {
                rec.confidence
            };
            (cleaned, confidence, None)
        }
        Language::Western => clean_western(&rec),
    };
    let raw_text = (!raw.is_empty() && raw != text).then_some(raw);
    Cleaned {
        text,
        confidence,
        words,
        raw_text,
    }
}

/// One whitespace-delimited unit moving through the rule chain.
/// `word` carries the recognizer's per-word data when available so
/// the cleaned output can still report confidences and bboxes.
struct Token {
    text: String,
    word: Option<Word>,
}

fn clean_western(rec: &Recognition) -> (String, f32, Option<Vec<Word>>) {
    let had_words = rec.words.is_some();
    let mut lines = tokenize(rec); // W1 happens in here
    restore_pronoun_i(&mut lines); // W1b
    let tokens = join_hyphenated(lines); // W2
    let kept: Vec<Token> = tokens
        .into_iter()
        .filter_map(|t| scrub_token(t, rec.confidence))
        .collect(); // W3-W5

    // W6: single spaces, no newlines — one bubble, one utterance.
    let text = kept
        .iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    // W7
    if text.is_empty() {
        let confidence = if rec.text.trim().is_empty() {
            rec.confidence
        } else {
            0.0
        };
        return (text, confidence, None);
    }

    // W8
    let confs: Vec<f32> = kept
        .iter()
        .filter_map(|t| t.word.as_ref().map(|w| w.confidence))
        .collect();
    let confidence = if !confs.is_empty() {
        confs.iter().sum::<f32>() / confs.len() as f32
    } else {
        rec.confidence
    };
    let words = had_words.then(|| {
        kept.iter()
            .filter_map(|t| {
                t.word.clone().map(|mut w| {
                    w.text = t.text.clone();
                    w
                })
            })
            .collect()
    });
    (text, confidence, words)
}

/// Split the recognition into lines of tokens. The word-backed path
/// applies W1 (confidence drops); the text-only fallback can't and
/// skips it — fail-safe, never guessing at confidences.
fn tokenize(rec: &Recognition) -> Vec<Vec<Token>> {
    match &rec.words {
        Some(ws) => {
            let mut lines: Vec<Vec<Token>> = Vec::new();
            let mut cur_line: Option<u32> = None;
            for w in ws {
                if w.confidence < WORD_MIN_CONF {
                    continue;
                }
                if w.confidence < SYMBOL_WORD_MIN_CONF && !w.text.chars().any(char::is_alphanumeric)
                {
                    continue;
                }
                if cur_line != Some(w.line_index) {
                    lines.push(Vec::new());
                    cur_line = Some(w.line_index);
                }
                lines.last_mut().expect("line pushed above").push(Token {
                    text: w.text.clone(),
                    word: Some(w.clone()),
                });
            }
            lines
        }
        None => rec
            .text
            .lines()
            .map(|l| {
                l.split_whitespace()
                    .map(|t| Token {
                        text: t.to_owned(),
                        word: None,
                    })
                    .collect::<Vec<Token>>()
            })
            .filter(|l| !l.is_empty())
            .collect(),
    }
}

/// W1b: read Tesseract's `|` as the pronoun `I` where comic lettering
/// means it (see the module docs). Runs per line, before the hyphen
/// join flattens the lines, because "between words" has to mean
/// "between words on the same line": a `|` at a line edge is where a
/// balloon border sits.
fn restore_pronoun_i(lines: &mut [Vec<Token>]) {
    let has_letter = |t: &Token| t.text.chars().any(char::is_alphabetic);
    for line in lines.iter_mut() {
        for i in 0..line.len() {
            if line[i].text == "|" {
                let between_words = i > 0
                    && i + 1 < line.len()
                    && has_letter(&line[i - 1])
                    && has_letter(&line[i + 1]);
                if between_words {
                    line[i].text = "I".to_owned();
                }
            } else if let Some(fixed) = contraction_i(&line[i].text) {
                line[i].text = fixed;
            }
        }
    }
}

/// `|'M` → `I'M` (also `|'LL`, `|'VE`, `|'D`, a curly apostrophe, any
/// case, and trailing expressive punctuation such as `|'M,`). `None`
/// for anything else, including `|'MA` or `|'M~`.
fn contraction_i(token: &str) -> Option<String> {
    let rest = token.strip_prefix('|')?;
    let apostrophe = rest.chars().next().filter(|c| matches!(c, '\'' | '’'))?;
    let after = &rest[apostrophe.len_utf8()..];
    let letters_end = after
        .char_indices()
        .find(|(_, c)| !c.is_alphabetic())
        .map_or(after.len(), |(i, _)| i);
    let (letters, tail) = after.split_at(letters_end);
    let known = I_CONTRACTIONS
        .iter()
        .any(|c| c.eq_ignore_ascii_case(letters));
    if !known || !tail.chars().all(|c| EXPRESSIVE.contains(c)) {
        return None;
    }
    Some(format!("I{apostrophe}{letters}{tail}"))
}

/// W2: does this token end in a word followed by exactly one hyphen
/// (`SPIDER-`, not `WAIT--`, not a lone `-`)?
fn ends_in_single_hyphen(token: &str) -> bool {
    let mut rev = token.chars().rev();
    matches!(rev.next(), Some('-' | '\u{2010}')) && rev.next().is_some_and(char::is_alphanumeric)
}

/// W2: a line ending in a word + one hyphen joins the next line's
/// first token when that starts with a letter, and the hyphen stays
/// (`SPIDER-` + `MAN` → `SPIDER-MAN`; a U+2010 hyphen is written as
/// `-`, which W5 keeps). A double dash (`WAIT--`) or a lone `-` is not
/// joined: the next word follows after a space. Flattens the line
/// structure — every rule after this operates on a single token
/// stream.
fn join_hyphenated(lines: Vec<Vec<Token>>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    for line in lines {
        let mut iter = line.into_iter();
        if let Some(first) = iter.next() {
            let joinable = out
                .last()
                .is_some_and(|prev| ends_in_single_hyphen(&prev.text))
                && first.text.chars().next().is_some_and(char::is_alphabetic);
            if joinable {
                let prev = out.last_mut().expect("checked non-empty above");
                prev.text.pop();
                prev.text.push('-');
                prev.text.push_str(&first.text);
                prev.word = match (prev.word.take(), first.word) {
                    (Some(a), Some(b)) => Some(Word {
                        text: String::new(), // rewritten from token text at the end
                        confidence: a.confidence.min(b.confidence),
                        xmin: a.xmin.min(b.xmin),
                        ymin: a.ymin.min(b.ymin),
                        xmax: a.xmax.max(b.xmax),
                        ymax: a.ymax.max(b.ymax),
                        line_index: a.line_index,
                    }),
                    (a, _) => a,
                };
            } else {
                out.push(first);
            }
        }
        out.extend(iter);
    }
    out
}

/// W3: symbol swearing (`#@$%!`, `@#$%&!`) as opposed to border or
/// halftone debris. See [`GRAWLIX`] / [`GRAWLIX_CORE`].
fn is_grawlix(token: &str) -> bool {
    if token.chars().count() < GRAWLIX_MIN_LEN || !token.chars().all(|c| GRAWLIX.contains(c)) {
        return false;
    }
    let mut core: Vec<char> = token
        .chars()
        .filter(|c| GRAWLIX_CORE.contains(*c))
        .collect();
    core.sort_unstable();
    core.dedup();
    core.len() >= 2
}

/// W3 + W4 + W5 on one token. `None` means the token was debris.
/// `rec_confidence` stands in for the token's own confidence when the
/// engine returned no per-word data.
fn scrub_token(mut tok: Token, rec_confidence: f32) -> Option<Token> {
    // W3
    let has_alnum = tok.text.chars().any(char::is_alphanumeric);
    let confidence = tok.word.as_ref().map_or(rec_confidence, |w| w.confidence);
    let grawlix = !has_alnum && is_grawlix(&tok.text) && confidence >= GRAWLIX_MIN_CONF;
    if !has_alnum && !grawlix && !tok.text.chars().all(|c| EXPRESSIVE.contains(c)) {
        return None;
    }
    // W4 (a grawlix is made of the very symbols W4 strips)
    let stripped = if grawlix {
        tok.text.as_str()
    } else {
        tok.text.trim_matches(|c| HARD_JUNK.contains(c))
    };
    // W5
    let filtered: String = stripped
        .chars()
        .filter(|c| c.is_alphanumeric() || KEEP_PUNCT.contains(*c))
        .collect();
    if filtered.is_empty() {
        return None;
    }
    tok.text = filtered;
    Some(tok)
}

/// Manga path: strip control/zero-width/replacement chars, collapse
/// whitespace runs. Western rules must NOT fire here — Japanese has
/// no whitespace tokenization and its punctuation is all legitimate.
fn clean_manga(text: &str) -> String {
    let stripped: String = text
        .chars()
        .filter(|&c| {
            !(c == '\u{FFFD}'
                || matches!(c, '\u{200B}'..='\u{200F}' | '\u{FEFF}')
                || (c.is_control() && !c.is_whitespace()))
        })
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(text: &str) -> Recognition {
        Recognition {
            text: text.to_owned(),
            confidence: 0.8,
            words: None,
        }
    }

    fn word(text: &str, confidence: f32, line_index: u32) -> Word {
        Word {
            text: text.to_owned(),
            confidence,
            xmin: 0.0,
            ymin: 0.0,
            xmax: 10.0,
            ymax: 10.0,
            line_index,
        }
    }

    fn rec_with_words(words: Vec<Word>) -> Recognition {
        let text = words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Recognition {
            text,
            confidence: 0.8,
            words: Some(words),
        }
    }

    // ── W3 / W4 / W5 (token rules, text-only path) ──

    #[test]
    fn strips_edge_junk_from_tokens() {
        for (input, want) in [
            ("|HELLO!", "HELLO!"),
            ("~WAIT~", "WAIT"),
            ("_OKAY_", "OKAY"),
            ("//RUN", "RUN"),
            ("[STOP]", "STOP"),
        ] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
    }

    #[test]
    fn keeps_interior_punctuation() {
        for (input, want) in [
            ("DON'T", "DON'T"),
            ("E-MAIL", "E-MAIL"),
            ("\"QUOTED\"", "\"QUOTED\""),
        ] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
    }

    #[test]
    fn drops_symbol_only_tokens() {
        assert_eq!(
            clean(rec("HELLO ~| WORLD"), Language::Western).text,
            "HELLO WORLD"
        );
        assert_eq!(clean(rec("A _ B ^ C"), Language::Western).text, "A B C");
    }

    #[test]
    fn preserves_expressive_punctuation_tokens() {
        for (input, want) in [("WHAT?!", "WHAT?!"), ("!? ...", "!? ..."), ("!!", "!!")] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
    }

    #[test]
    fn charset_filter_drops_exotic_symbols() {
        // U+FFFD and a box-drawing char vanish; letters survive.
        assert_eq!(
            clean(rec("HE\u{FFFD}LLO ─WORLD"), Language::Western).text,
            "HELLO WORLD"
        );
    }

    // ── W2 (hyphen join) + W6 (whitespace) ──

    #[test]
    fn line_end_hyphen_joins_and_is_kept() {
        for (input, want) in [
            ("SPIDER-\nMAN", "SPIDER-MAN"),
            ("THE SPIDER-\nMAN IS HERE", "THE SPIDER-MAN IS HERE"),
            ("Spider-\nman", "Spider-man"),
            // U+2010 is written as an ASCII hyphen, which W5 keeps.
            ("SPIDER\u{2010}\nMAN", "SPIDER-MAN"),
            // The accepted cost: a word split across lines keeps its hyphen.
            ("WON-\nDERFUL", "WON-DERFUL"),
        ] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
    }

    #[test]
    fn double_dash_at_line_end_is_an_interruption() {
        for (input, want) in [
            ("WAIT--\nNO", "WAIT-- NO"),
            ("I CAN'T--\nLOOK OUT!", "I CAN'T-- LOOK OUT!"),
        ] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
    }

    #[test]
    fn lone_hyphen_does_not_join_lines() {
        // A trailing "-" with no stem is an em-dash-style break.
        assert_eq!(
            clean(rec("WAIT -\nNO"), Language::Western).text,
            "WAIT - NO"
        );
    }

    #[test]
    fn hyphen_join_requires_letter_on_next_line() {
        assert_eq!(
            clean(rec("ROUTE-\n66"), Language::Western).text,
            "ROUTE- 66"
        );
    }

    #[test]
    fn collapses_newlines_into_spaces() {
        assert_eq!(
            clean(rec("ONE\nBUBBLE\nUTTERANCE"), Language::Western).text,
            "ONE BUBBLE UTTERANCE"
        );
    }

    // ── W1 / W8 (confidence, word-backed path) ──

    #[test]
    fn drops_low_confidence_words() {
        let r = rec_with_words(vec![
            word("KEEP", 0.9, 0),
            word("NOISE", 0.2, 0), // below WORD_MIN_CONF
            word("ALSO", 0.8, 0),
        ]);
        let out = clean(r, Language::Western);
        assert_eq!(out.text, "KEEP ALSO");
    }

    #[test]
    fn drops_mid_confidence_symbol_words_keeps_letters() {
        let r = rec_with_words(vec![
            word("HELLO", 0.5, 0), // letters: survives at 0.5
            word("~'", 0.5, 0),    // symbols below SYMBOL_WORD_MIN_CONF: dropped
        ]);
        assert_eq!(clean(r, Language::Western).text, "HELLO");
    }

    #[test]
    fn recomputes_confidence_over_kept_words() {
        let r = rec_with_words(vec![
            word("A", 0.9, 0),
            word("JUNK", 0.1, 0), // dropped by W1
            word("B", 0.7, 0),
        ]);
        let out = clean(r, Language::Western);
        assert!((out.confidence - 0.8).abs() < 1e-6);
    }

    #[test]
    fn cleaned_words_carry_cleaned_text() {
        let r = rec_with_words(vec![word("|HELLO!", 0.9, 0)]);
        let out = clean(r, Language::Western);
        let words = out.words.expect("word-backed path keeps words");
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "HELLO!");
    }

    #[test]
    fn word_backed_hyphen_join_merges_bboxes() {
        let mut first = word("HYPH-", 0.9, 0);
        first.xmin = 5.0;
        first.xmax = 20.0;
        let mut second = word("EN", 0.7, 1);
        second.xmin = 2.0;
        second.xmax = 9.0;
        second.ymin = 12.0;
        second.ymax = 22.0;
        let out = clean(rec_with_words(vec![first, second]), Language::Western);
        assert_eq!(out.text, "HYPH-EN");
        let words = out.words.expect("words");
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "HYPH-EN");
        assert!((words[0].confidence - 0.7).abs() < 1e-6);
        assert!((words[0].xmin - 2.0).abs() < 1e-6);
        assert!((words[0].xmax - 20.0).abs() < 1e-6);
        assert!((words[0].ymax - 22.0).abs() < 1e-6);
    }

    // ── W1b (pronoun I) ──

    #[test]
    fn lone_bar_between_words_is_the_pronoun() {
        for (input, want) in [
            ("WHAT DO | DO NOW?", "WHAT DO I DO NOW?"),
            ("SO | SAID NO", "SO I SAID NO"),
            ("and | said", "and I said"),
        ] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
    }

    #[test]
    fn bar_before_a_contraction_is_the_pronoun() {
        for (input, want) in [
            ("|'M FINE", "I'M FINE"),
            ("|'LL GO", "I'LL GO"),
            ("WE KNOW |'VE TRIED", "WE KNOW I'VE TRIED"),
            ("|'D SAY SO", "I'D SAY SO"),
            ("|’M HERE", "I’M HERE"),
            ("|'m fine", "I'm fine"),
            ("YES, |'M!", "YES, I'M!"),
            // At the start of a line too: `|'M` cannot be border debris.
            ("HELLO\n|'M BACK", "HELLO I'M BACK"),
        ] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
    }

    #[test]
    fn bars_that_are_balloon_borders_are_still_dropped() {
        for (input, want) in [
            // At a line edge: the border position.
            ("| HELLO THERE |", "HELLO THERE"),
            ("HELLO\n| THERE", "HELLO THERE"),
            ("HELLO |\nTHERE", "HELLO THERE"),
            // Glued to other symbols.
            ("HELLO |~ THERE", "HELLO THERE"),
            ("HELLO || THERE", "HELLO THERE"),
            ("HELLO |_ THERE", "HELLO THERE"),
            // Not a word on both sides.
            ("HELLO | !!", "HELLO !!"),
            // Not one of the contractions.
            ("|'MA", "'MA"),
            ("|'S", "'S"),
        ] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
    }

    #[test]
    fn low_confidence_bar_is_dropped_before_the_pronoun_rule() {
        let r = rec_with_words(vec![
            word("SO", 0.9, 0),
            word("|", 0.5, 0), // symbol-only below SYMBOL_WORD_MIN_CONF (W1)
            word("SAID", 0.9, 0),
        ]);
        assert_eq!(clean(r, Language::Western).text, "SO SAID");
        let r = rec_with_words(vec![
            word("SO", 0.9, 0),
            word("|", 0.8, 0),
            word("SAID", 0.9, 0),
        ]);
        let out = clean(r, Language::Western);
        assert_eq!(out.text, "SO I SAID");
        assert_eq!(out.words.unwrap()[1].text, "I");
    }

    // ── W3 (grawlix) ──

    #[test]
    fn grawlix_is_kept_when_confident() {
        for (input, want) in [
            ("WHAT THE #@$%!", "WHAT THE #@$%!"),
            ("@#$%&!", "@#$%&!"),
            ("YOU *#@!", "YOU *#@!"),
        ] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
        let r = rec_with_words(vec![word("OH", 0.9, 0), word("#@$%!", 0.7, 0)]);
        assert_eq!(clean(r, Language::Western).text, "OH #@$%!");
    }

    #[test]
    fn grawlix_lookalikes_and_low_confidence_runs_are_dropped() {
        let low = Recognition {
            text: "OH #@$%!".to_owned(),
            confidence: 0.4,
            words: None,
        };
        assert_eq!(clean(low, Language::Western).text, "OH");
        for (input, want) in [
            // Too short.
            ("OH #@", "OH"),
            // One repeated symbol is not swearing.
            ("OH @@@", "OH"),
            ("OH ***", "OH"),
            // Border debris is not grawlix.
            ("OH ~|'_", "OH"),
            ("OH ~~~", "OH"),
            // Grawlix glued to border junk is not a pure grawlix run.
            ("OH |#@$", "OH"),
        ] {
            assert_eq!(clean(rec(input), Language::Western).text, want, "{input}");
        }
    }

    // ── W7 (empty-out guard) + raw_text ──

    #[test]
    fn pure_debris_empties_out_with_zero_confidence() {
        let out = clean(rec("~| ^^ //"), Language::Western);
        assert_eq!(out.text, "");
        assert_eq!(out.confidence, 0.0);
        assert_eq!(out.raw_text.as_deref(), Some("~| ^^ //"));
    }

    #[test]
    fn raw_text_none_when_cleaning_is_identity() {
        let out = clean(rec("CLEAN TEXT"), Language::Western);
        assert_eq!(out.text, "CLEAN TEXT");
        assert!(out.raw_text.is_none());
    }

    #[test]
    fn empty_input_stays_empty_without_penalty() {
        let out = clean(rec(""), Language::Western);
        assert_eq!(out.text, "");
        assert!((out.confidence - 0.8).abs() < 1e-6);
        assert!(out.raw_text.is_none());
    }

    // ── CTC line recogniser: engine-independent rules only ──

    fn line_rec(lines: &[(&str, f32)]) -> Recognition {
        Recognition {
            text: lines.iter().map(|(t, _)| *t).collect::<Vec<_>>().join("\n"),
            confidence: 0.9,
            words: Some(
                lines
                    .iter()
                    .enumerate()
                    .map(|(i, (t, c))| word(t, *c, i as u32))
                    .collect(),
            ),
        }
    }

    #[test]
    fn lines_are_joined_into_one_utterance() {
        let out = clean_lines(line_rec(&[
            ("WHAT A WORKING-", 0.95),
            ("OVER! IT'LL BE A", 0.9),
            ("PLEASURE  TO BE SHOT!", 0.85),
        ]));
        assert_eq!(
            out.text,
            "WHAT A WORKING-OVER! IT'LL BE A PLEASURE TO BE SHOT!"
        );
        assert_eq!(
            out.raw_text.as_deref(),
            Some("WHAT A WORKING-\nOVER! IT'LL BE A\nPLEASURE  TO BE SHOT!")
        );
        // The recogniser's own confidence, untouched.
        assert!((out.confidence - 0.9).abs() < 1e-6);
        // One entry per line, each with its own text.
        let lines = out.words.expect("line entries");
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "WHAT A WORKING-",
                "OVER! IT'LL BE A",
                "PLEASURE TO BE SHOT!"
            ]
        );
    }

    #[test]
    fn the_line_end_hyphen_rule_is_the_same_one() {
        for (input, want) in [
            ("SPIDER-\nMAN", "SPIDER-MAN"),
            // The one rewrite: a U+2010 hyphen at the join becomes `-`.
            // Anywhere else it stays as read.
            ("SPIDER\u{2010}\nMAN", "SPIDER-MAN"),
            ("A\u{2010}B C\u{2010}", "A\u{2010}B C\u{2010}"),
            ("WAIT--\nNO", "WAIT-- NO"),
            ("WAIT -\nNO", "WAIT - NO"),
            ("ROUTE-\n66", "ROUTE- 66"),
        ] {
            assert_eq!(clean_lines(rec(input)).text, want, "{input}");
        }
    }

    /// What the Tesseract chain would drop or rewrite is kept as read.
    #[test]
    fn nothing_the_line_recogniser_read_is_dropped_or_rewritten() {
        for input in [
            // W1b / W3 / W4: a bar, symbol-only tokens, edge symbols.
            "WHAT DO | DO",
            "| HELLO THERE |",
            "HELLO ~| WORLD",
            "|HELLO!",
            "[STOP]",
            "OH @@@ ***",
            "OH #@",
            // W5: characters outside the Tesseract allowlist.
            "50% OFF ● TODAY ©",
            "A < B > C",
        ] {
            let out = clean_lines(rec(input));
            assert_eq!(out.text, input, "{input}");
            assert!(out.raw_text.is_none(), "{input}");
        }
        // W1: low-scored lines stay, in the text and as entries.
        let out = clean_lines(line_rec(&[("KEEP", 0.9), ("FAINT", 0.2), ("~'", 0.5)]));
        assert_eq!(out.text, "KEEP FAINT ~'");
        assert_eq!(out.words.expect("entries").len(), 3);
        // The same input through the Tesseract chain loses them.
        let out = clean(
            line_rec(&[("KEEP", 0.9), ("FAINT", 0.2), ("~'", 0.5)]),
            Language::Western,
        );
        assert_eq!(out.text, "KEEP");
    }

    #[test]
    fn an_empty_reading_stays_empty_with_its_confidence() {
        let out = clean_lines(Recognition {
            text: String::new(),
            confidence: 0.0,
            words: None,
        });
        assert_eq!(out.text, "");
        assert_eq!(out.confidence, 0.0);
        assert!(out.words.is_none() && out.raw_text.is_none());
        // Whitespace only: nothing was read.
        let out = clean_lines(rec(" \n  "));
        assert_eq!(out.text, "");
        assert_eq!(out.raw_text.as_deref(), Some(" \n  "));
    }

    #[test]
    fn the_language_picks_the_rule_chain() {
        let bar = || rec("| HELLO |");
        // Western: the line rules, never the Tesseract chain.
        assert_eq!(clean_for(bar(), Language::Western).text, "| HELLO |");
        assert_eq!(clean(bar(), Language::Western).text, "HELLO");
        // Japanese: the manga rules.
        let out = clean_for(rec("す\u{200B}ごい | ！"), Language::Manga);
        assert_eq!(out.text, "すごい | ！");
    }

    // ── Manga path: western rules must not fire ──

    #[test]
    fn manga_passes_japanese_punctuation_untouched() {
        for input in ["「すごい！」", "なに……？", "そうだ。ね、行こう！"] {
            assert_eq!(clean(rec(input), Language::Manga).text, input, "{input}");
        }
    }

    #[test]
    fn manga_strips_control_and_zero_width_chars() {
        let out = clean(rec("す\u{200B}ごい\u{FFFD}！"), Language::Manga);
        assert_eq!(out.text, "すごい！");
        assert_eq!(out.raw_text.as_deref(), Some("す\u{200B}ごい\u{FFFD}！"));
    }

    #[test]
    fn manga_debris_only_input_zeroes_confidence() {
        let out = clean(rec("\u{FFFD}\u{200B}"), Language::Manga);
        assert_eq!(out.text, "");
        assert_eq!(out.confidence, 0.0);
    }

    #[test]
    fn western_symbol_rules_do_not_apply_to_manga() {
        // Tokens that the western chain would drop survive in manga.
        let input = "ーー！？";
        assert_eq!(clean(rec(input), Language::Manga).text, input);
    }
}
