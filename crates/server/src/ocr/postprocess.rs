//! Post-recognition text cleanup.
//!
//! Raw engine output from comic lettering carries OCR debris: bubble
//! borders, tails, and halftone dots read back as stray `| ~ ' _`
//! symbols, and Tesseract keeps low-confidence guesses in the text.
//! This module is the single deterministic cleanup pass, run by the
//! pipeline right after recognition (see
//! [`crate::ocr::pipeline::run_ocr`]).
//!
//! Western rule chain, in order (W-numbers referenced from tests):
//!
//! - **W1** word-confidence drop — needs per-word data; skipped when
//!   the recognizer returned `words: None`.
//! - **W2** hyphenated line-break join (`HYPH-\nEN` → `HYPHEN`).
//! - **W3** symbol-only token drop, sparing expressive punctuation
//!   (`!?`, `...`).
//! - **W4** edge junk strip per token (`|HELLO!` → `HELLO!`).
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

/// Output of [`clean`]. `raw_text` is `Some` only when cleaning
/// changed the text — a debugging and golden-fixture-authoring aid.
#[derive(Debug, Clone)]
pub struct Cleaned {
    pub text: String,
    pub confidence: f32,
    pub words: Option<Vec<Word>>,
    pub raw_text: Option<String>,
}

/// Entry point: language-aware cleanup of one recognition result.
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
    let lines = tokenize(rec); // W1 happens in here
    let tokens = join_hyphenated(lines); // W2
    let kept: Vec<Token> = tokens.into_iter().filter_map(scrub_token).collect(); // W3-W5

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

/// W2: a line ending in a hyphenated stem joins the next line's
/// first token (`HYPH-` + `EN` → `HYPHEN`). Requires a stem (a lone
/// `-` is an em-dash-style break, not hyphenation) and a letter on
/// the other side. Flattens the line structure — every rule after
/// this operates on a single token stream.
fn join_hyphenated(lines: Vec<Vec<Token>>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    for line in lines {
        let mut iter = line.into_iter();
        if let Some(first) = iter.next() {
            let joinable = out.last().is_some_and(|prev| {
                prev.text.chars().count() > 1 && prev.text.ends_with(['-', '\u{2010}'])
            }) && first.text.chars().next().is_some_and(char::is_alphabetic);
            if joinable {
                let prev = out.last_mut().expect("checked non-empty above");
                prev.text.pop();
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

/// W3 + W4 + W5 on one token. `None` means the token was debris.
fn scrub_token(mut tok: Token) -> Option<Token> {
    // W3
    let has_alnum = tok.text.chars().any(char::is_alphanumeric);
    if !has_alnum && !tok.text.chars().all(|c| EXPRESSIVE.contains(c)) {
        return None;
    }
    // W4
    let stripped = tok.text.trim_matches(|c| HARD_JUNK.contains(c));
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
    fn joins_hyphenated_line_breaks() {
        assert_eq!(
            clean(rec("SOME HYPH-\nENATED TEXT"), Language::Western).text,
            "SOME HYPHENATED TEXT"
        );
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
        assert_eq!(out.text, "HYPHEN");
        let words = out.words.expect("words");
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "HYPHEN");
        assert!((words[0].confidence - 0.7).abs() < 1e-6);
        assert!((words[0].xmin - 2.0).abs() < 1e-6);
        assert!((words[0].xmax - 20.0).abs() < 1e-6);
        assert!((words[0].ymax - 22.0).abs() < 1e-6);
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
