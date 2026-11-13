//! Japanese recogniser: `mayocream/manga-ocr-onnx`, a ViT encoder and a
//! BERT decoder, run on two ONNX Runtime sessions Folio builds itself.
//!
//! Licence: this file is derived from the `manga-ocr` crate 0.5.1 (part
//! of the koharu project, <https://github.com/mayocream/koharu>), by Mayo
//! Takanashi, whose licence is the GNU General Public License version 3.
//! This file is licensed under the GNU General Public License version 3,
//! as that crate is. It is combined with the rest of Folio (licensed
//! AGPL-3.0-or-later) as section 13 of GPLv3 and of AGPLv3 permit; the
//! licence of the rest of Folio is unaffected. See
//! `LICENSE-THIRD-PARTY.md`.
//!
//! Modified by the Folio project, November 2026: sessions built by
//! Folio ([`crate::ocr::session`], explicit thread count), model files
//! from pinned, digest-checked copies
//! ([`crate::ocr::models::MANGA_FILES`]), bounds and checks added (below).
//! The preprocessing, the greedy decode and the vocabulary handling
//! follow the crate.
//!
//! One read:
//!
//! 1. **Preprocess** ([`preprocess`]): the crop in grey, as RGB, resized
//!    to 224 × 224 (Lanczos3, aspect ratio not kept), each value
//!    `(x / 255 − 0.5) / 0.5`, shape `[1, 3, 224, 224]`.
//! 2. **Encoder**, once: `pixel_values` → the hidden states.
//! 3. **Decoder**, greedily ([`greedy_decode`]): from the start token
//!    [`START_TOKEN`], one run per token over the whole sequence so far
//!    (`encoder_hidden_states`, `input_ids` → `logits`), taking the most
//!    probable class of the last position, until the end token
//!    [`END_TOKEN`] or [`MAX_STEPS`] runs.
//! 4. **Text** ([`tokens_to_text`]): tokens below [`FIRST_TEXT_TOKEN`]
//!    (padding, unknown, start, end, mask) are dropped, the rest mapped
//!    through `vocab.txt` and joined without separators.
//!
//! Confidence is reported as `1.0` for non-empty output and `0.0` for
//! empty: the greedy decode keeps no per-token probability.
//!
//! Bounds: the vocabulary must have exactly as many entries as the
//! decoder has output classes (checked when the engine loads; on a
//! mismatch the engine does not start), so every class maps to an entry;
//! every run's output shape is checked; a class score that is not a
//! number never wins; the decode stops after [`MAX_STEPS`] runs.
//!
//! Both sessions sit behind one [`std::sync::Mutex`] (a read needs both
//! in turn), taken for the encoder and decoder runs only: preprocessing
//! and the text mapping run outside it. Only call
//! [`MangaOcr::recognize`] from the blocking pool.

use std::path::Path;
use std::sync::Mutex;

use image::{DynamicImage, imageops::FilterType};
use ndarray::{Array2, Array4, ArrayD};
use ort::session::Session;
use ort::value::TensorRef;
use tokio::sync::OnceCell;

use super::{Recognition, Recognizer};
use crate::ocr::{models, session};

/// Side of the square the crop is resized to.
pub const IMAGE_SIZE: u32 = 224;
/// The token the decode starts from (`[CLS]`).
pub const START_TOKEN: i64 = 2;
/// The token that ends the decode (`[SEP]`).
pub const END_TOKEN: i64 = 3;
/// Tokens below this are special (padding, unknown, start, end, mask)
/// and produce no text.
pub const FIRST_TEXT_TOKEN: i64 = 5;
/// Most decoder runs one read makes (so at most this many tokens after
/// the start token).
pub const MAX_STEPS: usize = 300;

const ENCODER_INPUT: &str = "pixel_values";
const DECODER_HIDDEN: &str = "encoder_hidden_states";
const DECODER_IDS: &str = "input_ids";
const DECODER_LOGITS: &str = "logits";

/// The encoder and decoder sessions.
struct Sessions {
    encoder: Session,
    decoder: Session,
}

pub struct MangaOcr {
    sessions: Mutex<Sessions>,
    /// Name of the encoder's output (its first).
    encoder_output: String,
    vocab: Vec<String>,
    first_use: crate::ocr::FirstUse,
}

impl MangaOcr {
    /// Returns the shared singleton, initializing it on first call.
    /// Cold init waits on the verified download of the pinned files
    /// ([`crate::ocr::models`], ~440 MB) and builds the two sessions.
    pub async fn shared() -> anyhow::Result<&'static Self> {
        static CELL: OnceCell<MangaOcr> = OnceCell::const_new();
        CELL.get_or_try_init(|| async {
            let started = std::time::Instant::now();
            let ocr = Self::load().await?;
            crate::ocr::record_model_load("manga", started.elapsed());
            Ok(ocr)
        })
        .await
    }

    /// A recogniser of its own (tools; the server uses [`Self::shared`]).
    pub async fn load() -> anyhow::Result<Self> {
        // Before any download: on a CPU that cannot run ONNX Runtime the
        // engine fails here, not after fetching its models.
        session::environment()?;
        models::ensure_pinned(models::MANGA_FILES).await?;
        let pin = |name: &str| {
            models::MANGA_FILES
                .iter()
                .find(|f| f.file == name)
                .ok_or_else(|| anyhow::anyhow!("manga-ocr: no pinned file named {name}"))
                .and_then(models::cached_path)
        };
        let encoder = pin("encoder_model.onnx")?;
        let decoder = pin("decoder_model.onnx")?;
        let vocab = pin("vocab.txt")?;
        let threads = session::intra_threads(session::MAX_INTRA_THREADS);
        tokio::task::spawn_blocking(move || Self::from_files(&encoder, &decoder, &vocab, threads))
            .await
            .map_err(|e| anyhow::anyhow!("manga-ocr init task panicked: {e}"))?
    }

    fn from_files(
        encoder: &Path,
        decoder: &Path,
        vocab: &Path,
        intra_threads: usize,
    ) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(vocab)
            .map_err(|e| anyhow::anyhow!("manga-ocr: could not read the vocabulary: {e}"))?;
        let vocab = read_vocab(&raw)?;
        let encoder = session::build(encoder, intra_threads)?;
        let decoder = session::build(decoder, intra_threads)?;

        let enc_inputs = session::input_names(&encoder);
        let Some(&encoder_output) = session::output_names(&encoder).first() else {
            anyhow::bail!("manga-ocr: the encoder has no output");
        };
        let encoder_output = encoder_output.to_owned();
        if !enc_inputs.contains(&ENCODER_INPUT) {
            anyhow::bail!(
                "manga-ocr: the encoder has inputs {enc_inputs:?}; expected {ENCODER_INPUT}"
            );
        }
        let (dec_inputs, dec_outputs) = (
            session::input_names(&decoder),
            session::output_names(&decoder),
        );
        if !(dec_inputs.contains(&DECODER_HIDDEN)
            && dec_inputs.contains(&DECODER_IDS)
            && dec_outputs.contains(&DECODER_LOGITS))
        {
            anyhow::bail!(
                "manga-ocr: the decoder has inputs {dec_inputs:?} and outputs {dec_outputs:?}; \
                 expected {DECODER_HIDDEN}, {DECODER_IDS} and {DECODER_LOGITS}"
            );
        }
        let mut sessions = Sessions { encoder, decoder };
        let classes = decoder_classes(&mut sessions, &encoder_output)?;
        check_vocab(vocab.len(), classes)?;
        Ok(MangaOcr {
            sessions: Mutex::new(sessions),
            encoder_output,
            vocab,
            first_use: crate::ocr::FirstUse::new(),
        })
    }

    /// Entries in the vocabulary (as many as the decoder has classes).
    pub fn vocab_len(&self) -> usize {
        self.vocab.len()
    }

    /// The token ids one read of `image` decodes, the start token first.
    fn tokens(&self, image: &DynamicImage) -> anyhow::Result<Vec<i64>> {
        let pixels = preprocess(image);
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("manga-ocr mutex poisoned"))?;
        let Sessions { encoder, decoder } = &mut *guard;
        let hidden = encode(encoder, &self.encoder_output, &pixels)?;
        let classes = self.vocab.len();
        greedy_decode(|ids| decoder_step(decoder, &hidden, ids, classes))
    }
}

impl Recognizer for MangaOcr {
    fn recognize(&self, image: &DynamicImage) -> anyhow::Result<Recognition> {
        let started = std::time::Instant::now();
        let tokens = self.tokens(image)?;
        self.first_use.record("manga", started.elapsed());
        let text = tokens_to_text(&tokens, &self.vocab);
        let trimmed = text.trim().to_owned();
        let confidence = if trimmed.is_empty() { 0.0 } else { 1.0 };
        Ok(Recognition {
            text: trimmed,
            confidence,
            words: None,
        })
    }
}

/// The crop as the encoder's `pixel_values`: grey, as RGB, resized to
/// [`IMAGE_SIZE`] × [`IMAGE_SIZE`] (Lanczos3), `(x / 255 − 0.5) / 0.5`,
/// shape `[1, 3, 224, 224]`.
pub(crate) fn preprocess(image: &DynamicImage) -> Array4<f32> {
    let rgb = image.grayscale().to_rgb8();
    let resized = image::imageops::resize(&rgb, IMAGE_SIZE, IMAGE_SIZE, FilterType::Lanczos3);
    let side = IMAGE_SIZE as usize;
    let mut x = Array4::<f32>::zeros((1, 3, side, side));
    for (px, py, pixel) in resized.enumerate_pixels() {
        let (px, py) = (px as usize, py as usize);
        for c in 0..3 {
            x[[0, c, py, px]] = (f32::from(pixel[c]) / 255.0 - 0.5) / 0.5;
        }
    }
    x
}

/// One encoder run: the hidden states the decoder attends to.
fn encode(
    encoder: &mut Session,
    output: &str,
    pixels: &Array4<f32>,
) -> anyhow::Result<ArrayD<f32>> {
    let outputs = encoder.run(ort::inputs![
        ENCODER_INPUT => TensorRef::from_array_view(pixels.view())?
    ])?;
    Ok(outputs[output].try_extract_array::<f32>()?.to_owned())
}

/// One decoder run over the sequence `ids`: the class scores of its last
/// position. The output must be `[1, ids.len(), classes]`.
fn decoder_step(
    decoder: &mut Session,
    hidden: &ArrayD<f32>,
    ids: &[i64],
    classes: usize,
) -> anyhow::Result<Vec<f32>> {
    let input = Array2::from_shape_vec((1, ids.len()), ids.to_vec())?;
    let outputs = decoder.run(ort::inputs![
        DECODER_HIDDEN => TensorRef::from_array_view(hidden.view())?,
        DECODER_IDS => TensorRef::from_array_view(input.view())?,
    ])?;
    let logits = outputs[DECODER_LOGITS].try_extract_array::<f32>()?;
    let shape = logits.shape();
    if shape != [1, ids.len(), classes] {
        anyhow::bail!(
            "manga-ocr: unexpected decoder output shape {shape:?} for {} tokens and {classes} classes",
            ids.len()
        );
    }
    Ok(logits
        .index_axis(ndarray::Axis(1), ids.len() - 1)
        .iter()
        .copied()
        .collect())
}

/// The decoder's class count: the last dimension of `logits` when the
/// model declares it, else read from one run on a blank image.
fn decoder_classes(sessions: &mut Sessions, encoder_output: &str) -> anyhow::Result<usize> {
    let declared = sessions
        .decoder
        .outputs()
        .iter()
        .find(|o| o.name() == DECODER_LOGITS)
        .and_then(|o| o.dtype().tensor_shape())
        .and_then(|shape| shape.last().copied());
    if let Some(classes) = declared
        && classes > 0
    {
        return Ok(classes as usize);
    }
    let side = IMAGE_SIZE as usize;
    let hidden = encode(
        &mut sessions.encoder,
        encoder_output,
        &Array4::zeros((1, 3, side, side)),
    )?;
    let input = Array2::from_shape_vec((1, 1), vec![START_TOKEN])?;
    let outputs = sessions.decoder.run(ort::inputs![
        DECODER_HIDDEN => TensorRef::from_array_view(hidden.view())?,
        DECODER_IDS => TensorRef::from_array_view(input.view())?,
    ])?;
    let logits = outputs[DECODER_LOGITS].try_extract_array::<f32>()?;
    match logits.shape() {
        [1, 1, classes] => Ok(*classes),
        other => anyhow::bail!("manga-ocr: unexpected decoder output shape {other:?}"),
    }
}

/// The vocabulary must name every class of the decoder, and nothing more.
pub(crate) fn check_vocab(vocab_len: usize, classes: usize) -> anyhow::Result<()> {
    if vocab_len == classes {
        Ok(())
    } else {
        anyhow::bail!(
            "manga-ocr: the decoder has {classes} classes but the vocabulary has {vocab_len} entries"
        )
    }
}

/// `vocab.txt`: one token per line, the line number is the token id.
/// Line ends are `\n` (a trailing `\r` is dropped). An empty file is an
/// error.
pub(crate) fn read_vocab(raw: &str) -> anyhow::Result<Vec<String>> {
    let vocab: Vec<String> = raw.lines().map(str::to_owned).collect();
    if vocab.len() <= FIRST_TEXT_TOKEN as usize {
        anyhow::bail!(
            "manga-ocr: the vocabulary has {} entries, fewer than the special tokens",
            vocab.len()
        );
    }
    Ok(vocab)
}

/// The class with the largest score: the last of equal scores (as the
/// upstream crate chose), never a score that is not a number. `None` when
/// no score is a number.
pub(crate) fn best_token(scores: &[f32]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (class, &s) in scores.iter().enumerate() {
        if s.is_nan() {
            continue;
        }
        if best.is_none_or(|(_, b)| s >= b) {
            best = Some((class, s));
        }
    }
    best.map(|(class, _)| class)
}

/// Greedy decode: from [`START_TOKEN`], ask `step` for the class scores
/// after the sequence so far and append the best class, until it is
/// [`END_TOKEN`] (appended too) or after [`MAX_STEPS`] steps. A step whose
/// scores hold no number ends the decode with an error.
pub(crate) fn greedy_decode(
    mut step: impl FnMut(&[i64]) -> anyhow::Result<Vec<f32>>,
) -> anyhow::Result<Vec<i64>> {
    let mut ids = vec![START_TOKEN];
    for _ in 0..MAX_STEPS {
        let scores = step(&ids)?;
        let Some(best) = best_token(&scores) else {
            anyhow::bail!("manga-ocr: the decoder returned no usable score");
        };
        let token = i64::try_from(best)?;
        ids.push(token);
        if token == END_TOKEN {
            break;
        }
    }
    Ok(ids)
}

/// The text of a decoded sequence: special tokens (below
/// [`FIRST_TEXT_TOKEN`]) dropped, the rest mapped through the
/// vocabulary and joined; an id outside the vocabulary is dropped.
pub(crate) fn tokens_to_text(ids: &[i64], vocab: &[String]) -> String {
    ids.iter()
        .filter(|&&id| id >= FIRST_TEXT_TOKEN)
        .filter_map(|&id| usize::try_from(id).ok().and_then(|i| vocab.get(i)))
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_is_one_224_square_in_minus_one_to_one() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(31, 77, |x, y| {
            image::Rgb([(x * 8) as u8, (y * 3) as u8, 200])
        }));
        let x = preprocess(&img);
        assert_eq!(x.shape(), &[1, 3, 224, 224]);
        assert!(x.iter().all(|v| (-1.0..=1.0).contains(v)));
        // Grey in, so the three planes are equal.
        for (a, b) in x
            .index_axis(ndarray::Axis(1), 0)
            .iter()
            .zip(x.index_axis(ndarray::Axis(1), 2).iter())
        {
            assert_eq!(a, b);
        }
        let white =
            DynamicImage::ImageLuma8(image::GrayImage::from_pixel(5, 5, image::Luma([255])));
        assert!(preprocess(&white).iter().all(|&v| v == 1.0));
        let black = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(5, 5, image::Luma([0])));
        assert!(preprocess(&black).iter().all(|&v| v == -1.0));
    }

    /// Scores of `classes` classes whose best is `token`.
    fn scores_for(token: i64, classes: usize) -> Vec<f32> {
        let mut s = vec![0.0; classes];
        s[token as usize] = 5.0;
        s
    }

    #[test]
    fn decode_starts_at_start_and_stops_at_end() {
        let script = [7, 9, END_TOKEN, 11];
        let mut calls = Vec::new();
        let ids = greedy_decode(|seq| {
            calls.push(seq.to_vec());
            Ok(scores_for(script[seq.len() - 1], 12))
        })
        .unwrap();
        assert_eq!(ids, vec![START_TOKEN, 7, 9, END_TOKEN]);
        assert_eq!(
            calls[0],
            vec![START_TOKEN],
            "the first run sees the start token only"
        );
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn decode_stops_after_the_step_cap() {
        let mut runs = 0;
        let ids = greedy_decode(|_| {
            runs += 1;
            Ok(scores_for(6, 8))
        })
        .unwrap();
        assert_eq!(runs, MAX_STEPS);
        assert_eq!(ids.len(), MAX_STEPS + 1);
    }

    #[test]
    fn decode_refuses_a_step_with_no_number() {
        assert!(greedy_decode(|_| Ok(vec![f32::NAN; 4])).is_err());
        assert!(greedy_decode(|_| Ok(Vec::new())).is_err());
        let err = greedy_decode(|_| anyhow::bail!("run failed"));
        assert!(err.is_err());
    }

    #[test]
    fn best_token_skips_nan_and_takes_the_last_of_equals() {
        assert_eq!(best_token(&[f32::NAN, 1.0, f32::NAN, 0.5]), Some(1));
        assert_eq!(best_token(&[2.0, 1.0, 2.0, 0.0]), Some(2));
        assert_eq!(best_token(&[f32::NAN]), None);
        assert_eq!(best_token(&[f32::NEG_INFINITY, f32::NEG_INFINITY]), Some(1));
    }

    #[test]
    fn text_drops_special_and_unknown_ids() {
        let vocab: Vec<String> = [
            "[PAD]", "[UNK]", "[CLS]", "[SEP]", "[MASK]", "あ", "い", "。",
        ]
        .map(str::to_owned)
        .to_vec();
        let ids = [START_TOKEN, 5, 1, 6, 4, 7, 99, -1, END_TOKEN];
        assert_eq!(tokens_to_text(&ids, &vocab), "あい。");
        assert_eq!(tokens_to_text(&[START_TOKEN, END_TOKEN], &vocab), "");
    }

    #[test]
    fn vocab_reader_and_class_check() {
        let v = read_vocab("[PAD]\n[UNK]\n[CLS]\n[SEP]\n[MASK]\nあ\r\nい\n").unwrap();
        assert_eq!(v.len(), 7);
        assert_eq!(v[5], "あ", "a trailing CR is dropped");
        assert_eq!(v[6], "い");
        assert!(read_vocab("").is_err());
        assert!(read_vocab("a\nb\n").is_err());
        assert!(check_vocab(7, 7).is_ok());
        assert!(check_vocab(7, 8).is_err());
        assert!(check_vocab(8, 7).is_err());
    }
}
