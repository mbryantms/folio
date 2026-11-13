//! Post-recognition text cleanup.
//!
//! This module is the single deterministic cleanup pass, run by the
//! pipeline right after recognition (see
//! [`crate::ocr::pipeline::run_ocr`]). Which rules run depends on the
//! engine that produced the text ([`clean_for`]):
//!
//! - **CTC line recogniser** (Western, [`clean_lines`]): only rules that
//!   do not depend on an engine's mistakes. The recogniser returns one
//!   string per text line; the lines are joined into one utterance with
//!   the line-end hyphen rule and single spaces:
//!   - a line ending in a word + exactly one hyphen joins the next
//!     line's first token when it starts with a letter, and the hyphen
//!     stays (`SPIDER-\nMAN` → `SPIDER-MAN`). A double dash is an
//!     interruption and is kept as written, with the next word after a
//!     space (`WAIT--\nNO` → `WAIT-- NO`). Accepted cost: a word the
//!     letterer really split across lines keeps its hyphen
//!     (`WON-\nDERFUL` → `WON-DERFUL`); comic lettering rarely splits
//!     words, and deleting the hyphen broke every compound name that
//!     wrapped.
//!   - every other line break and every whitespace run becomes one
//!     space: one bubble is one utterance.
//!
//!   Nothing is dropped, and the only character ever rewritten is a
//!   U+2010 hyphen at such a join, which becomes `-`. The confidence is
//!   the recogniser's.
//! - **manga-ocr** (Japanese, [`clean_manga`]): strips control,
//!   zero-width and replacement characters and collapses whitespace
//!   runs. Japanese has no whitespace tokenization, and `。、「」！？ー…`
//!   are all legitimate output. Stripping a non-empty reading down to
//!   nothing reports confidence 0.0, so the client's "couldn't read
//!   text" path fires.
//!
//! The rules are hardcoded, mirroring the matching engine's
//! Hamming-ladder precedent. Anything tunable would also have to
//! participate in the OCR result-cache key: bump
//! [`crate::ocr::cache::OCR_RESULT_VERSION`] when changing anything in
//! this module.

use super::recognizer::{Language, Recognition, Word};

/// Output of [`clean_for`]. `raw_text` is `Some` only when cleaning
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
/// [`clean_lines`]; Japanese gets [`clean_manga`].
pub fn clean_for(rec: Recognition, lang: Language) -> Cleaned {
    match lang {
        Language::Western => clean_lines(rec),
        Language::Manga => clean_manga(rec),
    }
}

/// Cleanup for the CTC line recogniser: join the lines into one
/// utterance. A line ending in a word and one hyphen joins the next
/// line's first word (`SPACE-` / `BUSTERS` → `SPACE-BUSTERS`); every
/// other line break and every run of whitespace becomes one space.
/// No token is dropped, and the confidence is kept. One character can
/// change: at such a join a typographic hyphen (U+2010) ending the line
/// is written as the ASCII `-`. Nothing else is rewritten. The line
/// entries in `words` keep their own text, with whitespace collapsed.
pub fn clean_lines(rec: Recognition) -> Cleaned {
    let raw = rec.text;
    let lines: Vec<Vec<String>> = raw
        .lines()
        .map(|l| {
            l.split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<String>>()
        })
        .filter(|l| !l.is_empty())
        .collect();
    let text = join_hyphenated(lines).join(" ");
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

/// Cleanup for manga-ocr: strip control/zero-width/replacement chars and
/// collapse whitespace runs. Stripping a non-empty reading to nothing
/// means the output was pure debris, reported with confidence 0.0.
pub fn clean_manga(rec: Recognition) -> Cleaned {
    let raw = rec.text;
    let stripped: String = raw
        .chars()
        .filter(|&c| {
            !(c == '\u{FFFD}'
                || matches!(c, '\u{200B}'..='\u{200F}' | '\u{FEFF}')
                || (c.is_control() && !c.is_whitespace()))
        })
        .collect();
    let text = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    let confidence = if text.is_empty() && !raw.trim().is_empty() {
        0.0
    } else {
        rec.confidence
    };
    let raw_text = (!raw.is_empty() && raw != text).then_some(raw);
    Cleaned {
        text,
        confidence,
        words: None,
        raw_text,
    }
}

/// Does this token end in a word followed by exactly one hyphen
/// (`SPIDER-`, not `WAIT--`, not a lone `-`)?
fn ends_in_single_hyphen(token: &str) -> bool {
    let mut rev = token.chars().rev();
    matches!(rev.next(), Some('-' | '\u{2010}')) && rev.next().is_some_and(char::is_alphanumeric)
}

/// A line ending in a word + one hyphen joins the next line's first
/// token when that starts with a letter, and the hyphen stays
/// (`SPIDER-` + `MAN` → `SPIDER-MAN`; a U+2010 hyphen is written as
/// `-`). A double dash (`WAIT--`) or a lone `-` is not joined: the next
/// word follows after a space. Flattens the lines into one token stream.
fn join_hyphenated(lines: Vec<Vec<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        let mut iter = line.into_iter();
        if let Some(first) = iter.next() {
            let joinable = out.last().is_some_and(|prev| ends_in_single_hyphen(prev))
                && first.chars().next().is_some_and(char::is_alphabetic);
            if joinable {
                let prev = out.last_mut().expect("checked non-empty above");
                prev.pop();
                prev.push('-');
                prev.push_str(&first);
            } else {
                out.push(first);
            }
        }
        out.extend(iter);
    }
    out
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
    fn line_end_hyphen_joins_and_is_kept() {
        for (input, want) in [
            ("SPIDER-\nMAN", "SPIDER-MAN"),
            ("THE SPIDER-\nMAN IS HERE", "THE SPIDER-MAN IS HERE"),
            ("Spider-\nman", "Spider-man"),
            // The one rewrite: a U+2010 hyphen at the join becomes `-`.
            // Anywhere else it stays as read.
            ("SPIDER\u{2010}\nMAN", "SPIDER-MAN"),
            ("A\u{2010}B C\u{2010}", "A\u{2010}B C\u{2010}"),
            // The accepted cost: a word split across lines keeps its hyphen.
            ("WON-\nDERFUL", "WON-DERFUL"),
        ] {
            assert_eq!(clean_lines(rec(input)).text, want, "{input}");
        }
    }

    #[test]
    fn double_dash_at_line_end_is_an_interruption() {
        for (input, want) in [
            ("WAIT--\nNO", "WAIT-- NO"),
            ("I CAN'T--\nLOOK OUT!", "I CAN'T-- LOOK OUT!"),
        ] {
            assert_eq!(clean_lines(rec(input)).text, want, "{input}");
        }
    }

    #[test]
    fn lone_hyphen_does_not_join_lines() {
        // A trailing "-" with no stem is an em-dash-style break.
        assert_eq!(clean_lines(rec("WAIT -\nNO")).text, "WAIT - NO");
    }

    #[test]
    fn hyphen_join_requires_letter_on_next_line() {
        assert_eq!(clean_lines(rec("ROUTE-\n66")).text, "ROUTE- 66");
    }

    #[test]
    fn collapses_newlines_into_spaces() {
        assert_eq!(
            clean_lines(rec("ONE\nBUBBLE\nUTTERANCE")).text,
            "ONE BUBBLE UTTERANCE"
        );
    }

    #[test]
    fn nothing_the_line_recogniser_read_is_dropped_or_rewritten() {
        for input in [
            // A bar, symbol-only tokens, edge symbols.
            "WHAT DO | DO",
            "| HELLO THERE |",
            "HELLO ~| WORLD",
            "|HELLO!",
            "[STOP]",
            "OH @@@ ***",
            "OH #@",
            // Symbols and characters outside plain punctuation.
            "50% OFF ● TODAY ©",
            "A < B > C",
        ] {
            let out = clean_lines(rec(input));
            assert_eq!(out.text, input, "{input}");
            assert!(out.raw_text.is_none(), "{input}");
        }
        // Low-scored lines stay, in the text and as entries.
        let out = clean_lines(line_rec(&[("KEEP", 0.9), ("FAINT", 0.2), ("~'", 0.5)]));
        assert_eq!(out.text, "KEEP FAINT ~'");
        assert_eq!(out.words.expect("entries").len(), 3);
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
    fn the_language_picks_the_rules() {
        // Western: the line rules, nothing dropped.
        assert_eq!(
            clean_for(rec("| HELLO |"), Language::Western).text,
            "| HELLO |"
        );
        // Japanese: the manga rules.
        let out = clean_for(rec("す\u{200B}ごい | ！"), Language::Manga);
        assert_eq!(out.text, "すごい | ！");
    }

    // ── Manga path ──

    #[test]
    fn manga_passes_japanese_punctuation_untouched() {
        for input in [
            "「すごい！」",
            "なに……？",
            "そうだ。ね、行こう！",
            "ーー！？",
        ] {
            assert_eq!(clean_manga(rec(input)).text, input, "{input}");
        }
    }

    #[test]
    fn manga_strips_control_and_zero_width_chars() {
        let out = clean_manga(rec("す\u{200B}ごい\u{FFFD}！"));
        assert_eq!(out.text, "すごい！");
        assert_eq!(out.raw_text.as_deref(), Some("す\u{200B}ごい\u{FFFD}！"));
    }

    #[test]
    fn manga_debris_only_input_zeroes_confidence() {
        let out = clean_manga(rec("\u{FFFD}\u{200B}"));
        assert_eq!(out.text, "");
        assert_eq!(out.confidence, 0.0);
        // An empty reading keeps its confidence.
        let out = clean_manga(rec(""));
        assert!((out.confidence - 0.8).abs() < 1e-6);
        assert!(out.raw_text.is_none());
    }
}
