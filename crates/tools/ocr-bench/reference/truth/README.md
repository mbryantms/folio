# Reference transcriptions

12 pages, 166 text regions, 1,732 words, transcribed from 12 public-domain
comic pages. The page images are not in the repository: `../fetch.py pages
DEST` downloads them, and `../PROVENANCE.md` says where each comes from.

**Who made it:** an AI vision model reading the scans, in one transcription
pass plus one full re-reading pass. No person has checked it. Treat a
disagreement between an OCR engine and this reference as "look at the image",
not as an automatic engine error, until a person has reviewed these files.

## How it was made

1. Each page was cut into overlapping tiles (15 % overlap, each tile at most
   1100 px on its long side, so every tile was read at full resolution) plus
   a whole-page overview at about 900 px. Three low-resolution sources were
   enlarged before tiling (Lanczos; it adds no detail, it only stops the
   viewer shrinking them): Western Love Trails p030 x1.6 (795 px wide
   source), Coo Coo Comics p012 and p038 x1.3 (about 950 px wide). The
   `tiles` field of each file records the grid; the tiles themselves are not
   kept.
2. Pass 1: the overview was read to get the layout and reading order, then
   every tile was read and the text written into `<identifier>-p<NNN>.json`.
3. Pass 2: every tile was read again against the written file, region by
   region. Corrections are recorded per file in `second_pass` (count and what
   changed).
4. A script validated the files (ids, kinds, colour cases, bbox keys,
   whitespace) and wrote the `counts` block into each file.

## File format

```json
{
 "book": "<archive.org identifier>", "page_index": 4,
 "image": "pages/<identifier>/p004.jpg", "image_size": [w, h],
 "excluded": "what was deliberately left out", "page_notes": "...",
 "regions": [
  {"id": 1, "kind": "balloon", "text": "...",
   "bbox_pct": {"x": 6.2, "y": 4.0, "w": 85.5, "h": 5.0},
   "colour_case": "dark_on_light", "notes": "..."}
 ],
 "second_pass": {"corrections": 0, "details": []},
 "counts": {"regions": 18, "words": 225, "by_kind": {}, "by_colour_case": {}, "illegible_tokens": 0}
}
```

`ocr-bench` reads `regions` (`id`, `kind`, `text`, `bbox_pct`, `colour_case`,
`notes`) and ignores the other fields. A file may also be a bare list of
regions, and `bbox_pct` may be a four-number list.

- `kind`: `balloon` (speech and thought), `caption`, `sound_effect`,
  `sign_or_label` (signs, newspaper headlines, the newsreel screen),
  `title` (story title banners and running headers). The benchmark reports
  balloons and captions together as "tap targets" and the rest as display
  lettering.
- `colour_case`: `dark_on_light` (dark text on white or near-white, including
  unboxed captions on the paper), `light_on_dark`, `on_colour` (text on a
  tinted caption box or a coloured panel, or coloured display lettering).
- `bbox_pct`: **approximate**. Estimated by eye from the tiles, in percent
  of the page (x, y = top-left corner, w, h = size). Good enough to pair a
  detected region with a reference region by overlap or centre distance; do
  not use them to score detection accuracy. Some boxes may be off by several
  percent.
- `id` order is the natural reading order (left to right, top to bottom by
  panel tier; inside a panel, balloons in the order a reader takes them).

## Transcription conventions

- Text exactly as lettered, in the page's own capitalisation (almost all
  upper case; Flaming Love p005's narration caption is mixed-case script and
  Superworld's title has a lower-case "the"; both kept).
- Line breaks inside a region become single spaces. No leading/trailing or
  double spaces.
- A word broken across lines with a hyphen is written whole (`DES-` /
  `TROYED` -> `DESTROYED`). A hyphen that belongs to the word is kept
  (`SPACE-BUSTERS`, `WORKING-OVER`, `SLEEP-WALKER`, `NINE-THIRTY`,
  `REAL-LIFE`, stammers like `B-BUT`, `TH-THOSE`). Where that call was a
  judgement it is in `notes`.
- Punctuation as drawn: `..`, `...` and `....` are kept as drawn; double dashes
  are written `--` (also when lettered as one long stroke); a single dash used
  as punctuation stays `-`. Spacing **around** punctuation is normalised:
  a space after a sentence-ending mark or comma, none before it, and no
  spaces around `--` unless the dash ends a line and the next line starts a
  new phrase in a way a reader would space (rare; noted). The letterers'
  quirks (`RAT.'S`, `AROUND ,RESET`, `PAPERS ?`, `FOOL!MASS`) are therefore
  **not** reproduced; each is mentioned in `notes`. A scorer should normalise
  whitespace around punctuation on both sides before comparing (`ocr-bench`
  does, see `src/score.rs`).
- Apostrophes are written as `'` whatever their drawn shape (several 1940
  books draw them as ticks above the letters).
- Specks and print noise next to letters are ignored and noted.
- Bold/italic emphasis is not marked in `text`; it is mentioned in `notes`.
- Illegible words would be written `[?]`; none were needed in this set
  (`illegible_tokens` = 0 everywhere). Text hidden behind artwork is not
  guessed: Superworld p020 region 11 is the visible part of a headline
  (`PLANE CRAS 5 MEN KILLE`), and Coo Coo p038 region 16 stops at `BEAR`
  where the sign is cut by the panel border.
- Excluded on purpose (stated per file in `excluded`): printed page
  numbers; tiny background lettering under about 12 px (a licence plate,
  small stacked "Z" sleep marks on Coo Coo p038, newspaper body text, a
  vertical gutter line on Superworld p020).
- Word count = whitespace-separated tokens of `text`.

## Pages and cases covered

| page | genre / lettering | regions | words | 2nd-pass corrections | cases |
|---|---|---|---|---|---|
| `daredevilcomics01c2clevgleason1941julytitanfan-p004` | 1941 superhero, hand italic | 18 | 225 | 0 | many regions (13 balloons + 5 captions), captions on green/yellow/red bars |
| `superworld-comics-001-p020` | 1940 SF hero, very high-res scan | 13 | 109 | 0 | title banner, BOOM! SFX, partly occluded newspaper headline |
| `strange-terrors-6-p028` | 1953 horror | 13 | 140 | 0 | white balloons on black panels, yellow scroll captions, scream SFX inside a balloon |
| `gangsters-and-gun-molls-04-1952-06.-realistic-jvj-2by-nation-ia-p004` | 1952 crime | 20 | 161 | 0 | most regions on a page (13 balloons, 4 captions, 3 signs), tinted captions, small sign lettering |
| `flaming-love-01-1949-12.-quality-hp-darwination-ia-p005` | 1949 romance | 16 | 217 | 0 | mixed-case script caption, print-damaged letters, POW! SFX on colour |
| `cowgirl-romances-007-p015` | 1951 western romance | 13 | 162 | 1 | steeply slanted unboxed caption, yellow captions, page curvature |
| `1949-11-western-love-trails-v001n007-p030` | 1949 western romance | 15 | 193 | 0 | **low resolution** (795 px wide), balloons on black panels |
| `jumbo-comics-105-november-1947-p008` | 1947 jungle | 12 | 141 | 0 | thought balloons, italic lettering on dark panels |
| `AtomicWar003195302-p010` | 1953 war | 13 | 143 | 0 | rotated display SFX (YAAAR!), pink caption bars, 3 SFX |
| `space-busters-no.-1-spring-1952-p013` | 1952 SF | 6 | 92 | 0 | **few balloons** (6), clean lettering |
| `CooCooComics35-p012` | 1947 funny animal | 10 | 85 | 0 | **light-on-dark** newsreel screen, 2 SFX, scroll caption |
| `CooCooComics35-p038` | 1947 funny animal | 17 | 64 | 0 | **sound-effect heavy** (10 SFX), park signs |
| **total** | | **166** | **1,732** | **1** | |

By kind: 113 balloons, 26 captions, 17 sound effects, 8 signs/labels,
2 titles. By colour case: 140 dark_on_light, 25 on_colour, 1 light_on_dark.

## Known weaknesses

- **Made by an AI model, not a person.** The second pass was done by the same
  model that did the first, so systematic misreadings (a letter shape it
  consistently misreads) would survive it. One correction in 166 regions is
  low; it may mean the first pass was careful or that the second pass was not
  independent enough. A human spot-check of at least the uncertain regions
  listed below is recommended before the scores are trusted.
- **One period, one language.** Every page is a 1940 to 1953 scan with
  upper-case hand lettering in English. There is no modern digital lettering,
  no lower-case font, no manga and no other language.
- **No page reaches 15 balloons.** The densest pages have 13 balloons
  (Daredevil p004, Gangsters p004, Flaming Love p005) and 18-20 text regions
  in total.
- **Only one light-on-dark region** (the Coo Coo newsreel screen). Many
  regions sit on dark panels, but the balloons themselves are light.
- Spacing around punctuation is normalised (see conventions), so the
  reference cannot score whether an engine reproduces the letterer's spacing.
- Region boundaries are a judgement: the Coo Coo newsreel screen and the two
  park signs are each one region although they are several stacked lines in
  different sizes; the Superworld title banner is one region in three styles.
  A detector that splits them will need many-to-one matching.
- `bbox_pct` are rough.

### Specific doubts

- Daredevil p004 r1: comma vs full stop after PROPAGANDA (low contrast on green).
- Strange Terrors p028 r1: the display scream read as `AAIIEE!`; letter count
  uncertain.
- Atomic War p010 r3: `YAAAR!` letter count (overlapping display letters).
- Flaming Love p005 r9-r11: several print-damaged letters (R, E, H) transcribed
  as the intended letters; `VIRLO` rests on a slightly broken R.
- Cowgirl Romances p015 r1: surname read as `ANDREY`.
- Western Love Trails p030 r4: decorative initial read as `I` (looks like a
  slashed 0); r5: comma vs full stop after TANA at low resolution.
- Coo Coo p012 r4: `POW!?` (fused !? glyph); an engine reading `POW!` is
  arguably right.
- Coo Coo p038 r1: `SLEEP-WALKER` hyphen kept by judgement; r17: `SWISH` is
  lettered inside a motion trail at an angle and its orientation is unusual.
- Superworld p020 r11: only the visible part of the headline is recorded.

## Adding a reference set

Any directory of `<book>-p<NNN>.json` files in this shape works as `--truth`,
next to a `--pages` directory holding `<book>/p<NNN>.jpg` (png and webp are
read too). Keep page images that may not be redistributed out of the
repository; commit only what can be: the transcriptions, and a manifest of
pinned URLs and digests like `../pages.json`.
