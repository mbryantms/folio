# OCR fixtures

One comic page for the real-model tests in
[`../../ocr_recognizer.rs`](../../ocr_recognizer.rs) and
[`../../issue_ocr.rs`](../../issue_ocr.rs) (ignored by default; they
load the pinned models).

## The page

`strange-terrors-6-p028.jpg`: page index 28 (0-based, front cover = 0) of
*Strange Terrors* #6, January 1953, St. John Comics.

- Source: the Internet Archive item
  [`strange-terrors-6`](https://archive.org/details/strange-terrors-6),
  file `29.jpg` inside the item's `Strange Terrors 6.cbr`.
- Rights: the item's metadata carries the Public Domain Mark 1.0
  (`licenseurl: https://creativecommons.org/publicdomain/mark/1.0/`,
  read from <https://archive.org/metadata/strange-terrors-6>).
- Fetched 2026-11-12 and stored byte for byte, no re-encode: 1280 × 1876
  pixels, 735,487 bytes, SHA-256
  `2d0f9c53784d283489b0d59b45194eb95e47181cb8ceb78413f10d75ddbc8654`.

## The balloons

The tests cut three balloons from the page at the rectangles the page
detector returned for them when the fixture was made, so the recogniser
tests do not depend on the detector (a detector test checks that it
still returns these rectangles, to two pixels):

| Balloon | Rectangle on the page (x, y, w, h) | Text |
|---|---|---|
| 6 | 282, 726, 213, 166 | THAT VOICE --IT SOUNDS LIKE IT'S COMING FROM THE GRAVE! |
| 8 | 687, 660, 292, 190 | BUT WHY SHOULD HE WANT TO KEEP US FROM OPENING THE MINE? IT WOULD GIVE HIM HIS OLD JOB BACK AGAIN. |
| 10 | 24, 1244, 158, 108 | JOHN! LOOK OUT! |

The reference text of the page (13 regions: 11 balloons, 2 captions) was
transcribed from the scan and checked against it for these three; the
test file holds the centre of every region and the three texts.
