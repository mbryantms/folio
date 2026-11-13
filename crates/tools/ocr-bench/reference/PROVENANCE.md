# Provenance of the reference pages and candidate models

Nothing listed here is stored in the repository. `fetch.py` downloads each
file from the pinned address below and checks its size and SHA-256 against
[`pages.json`](pages.json) or [`models.json`](models.json). The
transcriptions in [`truth/`](truth/) are the only derived material that is
committed.

## Reference pages

12 pages from 11 comic books published between 1940 and 1953, taken from the
Internet Archive (archive.org) on 2026-11-12, one page per request.

### Rights

- Every book is an Internet Archive item whose own metadata carries
  `licenseurl` = Public Domain Mark 1.0 (table below; read from
  `https://archive.org/metadata/<identifier>` on 2026-11-12).
- That field is set by the uploader and is sometimes wrong on archive.org.
  As a second filter only United States books were kept, from publishers
  whose 1940s and 1950s comics are generally understood to have entered the
  public domain through non-renewal. That filter rests on general knowledge;
  individual copyright renewal records were **not** checked.
- No item needs an account to read.
- For three books the publisher is not stated on the item and is given from
  general knowledge: Cowgirl Romances (Fiction House), Western Love Trails
  (Ace Magazines), Space Busters (Ziff-Davis).

| identifier | title | year | publisher | genre | `licenseurl` |
|---|---|---|---|---|---|
| `1949-11-western-love-trails-v001n007` | Western Love Trails v1 #7 | 1949 | Ace Magazines | western | `http://creativecommons.org/publicdomain/mark/1.0/` |
| `AtomicWar003195302` | ATOMIC WAR! No. 3 | 1953 | Junior Books, Inc. (Ace) | war / Cold War | `http://creativecommons.org/publicdomain/mark/1.0/` |
| `CooCooComics35` | Coo Coo Comics #35 | 1947 | Animated Cartoons, Inc. (Nedor/Standard group) | funny animal | `http://creativecommons.org/publicdomain/mark/1.0/` |
| `cowgirl-romances-007` | Cowgirl Romances #7 | 1951 | Fiction House | western romance | `https://creativecommons.org/publicdomain/mark/1.0/` |
| `daredevilcomics01c2clevgleason1941julytitanfan` | Daredevil Comics #01 ("Daredevil Battles Hitler") | 1941 | Lev Gleason | superhero | `http://creativecommons.org/publicdomain/mark/1.0/` |
| `flaming-love-01-1949-12.-quality-hp-darwination-ia` | Flaming Love #1 | 1949 | Quality Comics | romance | `https://creativecommons.org/publicdomain/mark/1.0/` |
| `gangsters-and-gun-molls-04-1952-06.-realistic-jvj-2by-nation-ia` | Gangsters and Gun Molls #4 | 1952 | Avon / Realistic | crime | `https://creativecommons.org/publicdomain/mark/1.0/` |
| `jumbo-comics-105-november-1947` | Jumbo Comics #105 | 1947 | Fiction House | jungle adventure / adventure | `https://creativecommons.org/publicdomain/mark/1.0/` |
| `space-busters-no.-1-spring-1952` | Space Busters #1 | 1952 | Ziff-Davis | science fiction | `http://creativecommons.org/publicdomain/mark/1.0/` |
| `strange-terrors-6` | Strange Terrors #6 | 1953 | St. John Comics | horror | `https://creativecommons.org/publicdomain/mark/1.0/` |
| `superworld-comics-001` | Superworld Comics #1 | 1940 | Komos Publications | science fiction / superhero | `https://creativecommons.org/publicdomain/mark/1.0/` |

Item page: `https://archive.org/details/<identifier>`.

### Files

- Layout: `<identifier>/p<NNN>.jpg`. `NNN` is the item's BookReader page
  index (0-based, `n0` = front cover), the page shown at
  `https://archive.org/details/<identifier>/page/n<NNN>`. Coo Coo Comics has
  no BookReader derivative; there it is the 0-based position of the image in
  the item's CBZ, which equals the number in its file name.
- Ten pages are the **original scan JPEG** inside the uploader's CBZ or CBR,
  fetched as one archive member and kept byte for byte.
- Two pages exist only as the item's JPEG 2000 derivative. `fetch.py`
  downloads that file, checks it against its own digest (`download.sha256` in
  `pages.json`), decodes it with Pillow and saves it as JPEG quality 95:
  - `daredevilcomics01c2clevgleason1941julytitanfan/p004.jpg` (1300 px wide;
    the archive member request for the original CBZ page kept failing on
    2026-11-12, so the derivative was used; the original may be larger);
  - `1949-11-western-love-trails-v001n007/p030.jpg` (the item is a PDF
    upload whose only page images are 795 x 1100 px derivatives: a
    deliberately kept low-resolution case).

  The digest of the converted JPEG was reproduced from the kept download
  with Pillow 13.0.0, OpenJPEG 2.5.4 and libjpeg-turbo 3.2.0. Another build
  of those libraries may write different bytes; see `fetch.py
  --accept-reencode`.
- Page identity: each page except Coo Coo's was compared with the item's
  BookReader preview of the same index and its two neighbours; the Coo Coo
  pages were checked by eye.
- Sizes run from 795 to 2886 px wide. They are what the uploaders scanned.

| file | pixels | bytes | SHA-256 | stored |
|---|---|---:|---|---|
| `1949-11-western-love-trails-v001n007/p030.jpg` | 795x1100 | 446,638 | `75bd5526b5517406f4818759fee7b96b552420911c4897bc936fbe19aaee2436` | converted from JPEG 2000 |
| `AtomicWar003195302/p010.jpg` | 1312x1947 | 1,106,010 | `c7cc6784eec0945fdc517df1231c5999e77ed1b5d31179d06a539ad5683c6bb3` | as served |
| `CooCooComics35/p012.jpg` | 948x1406 | 391,028 | `42b3da812ce28dfe2d7936dcc193c790c61e1364e8069bd3b07e0f28dd7ad835` | as served |
| `CooCooComics35/p038.jpg` | 957x1413 | 441,588 | `9f3f651395281e1286d0b850d21600383c993817e818fad3e11ab498e748258b` | as served |
| `cowgirl-romances-007/p015.jpg` | 1178x1738 | 754,170 | `b7215250fc856dd5480a99747e1752bbdc937518daeebf77f8faafff7e896869` | as served |
| `daredevilcomics01c2clevgleason1941julytitanfan/p004.jpg` | 1300x1830 | 1,122,230 | `46d05fa3ac9b1ed265a73f8ff08d147e5f0e33da79e0573f275b4059738d100f` | converted from JPEG 2000 |
| `flaming-love-01-1949-12.-quality-hp-darwination-ia/p005.jpg` | 1680x2334 | 1,443,021 | `c604ac48d01a68fd5d26d3eddb61f2e12a230992e40ab4b05a1957522a385850` | as served |
| `gangsters-and-gun-molls-04-1952-06.-realistic-jvj-2by-nation-ia/p004.jpg` | 1979x2851 | 1,269,829 | `5138c865ccdd286dcf72ed896566b54866c59d6b72124ee6faf595fbfef9b7ab` | as served |
| `jumbo-comics-105-november-1947/p008.jpg` | 1280x1890 | 970,816 | `254ef7f75f63d8583e53db854ef90f5a5d6ae04e893153ab3b4261a78bb8cf4b` | as served |
| `space-busters-no.-1-spring-1952/p013.jpg` | 1440x1963 | 1,289,508 | `8a877762476243f692ccec2d84480cbc9b4aa4ccbc9287865bc9792f5a424024` | as served |
| `strange-terrors-6/p028.jpg` | 1280x1876 | 735,487 | `2d0f9c53784d283489b0d59b45194eb95e47181cb8ceb78413f10d75ddbc8654` | as served |
| `superworld-comics-001/p020.jpg` | 2886x3950 | 2,137,246 | `c67eedc8ca94d75711c991da75dc397c18ee6c621939b2f04572c36e51d35029` | as served |

The download address of each page is in `pages.json` (`url`).

## Candidate models

14 files from 7 Hugging Face repositories, 178.2 MB in total, each pinned to
the repository commit that was current on 2026-11-12. Address of a file:
`https://huggingface.co/<repository>/resolve/<commit>/<file>`.

- **Licence:** every repository's model card metadata declares
  `license: apache-2.0` at the pinned commit. That is the repository's own
  statement; no separate licence review was done.
- **Digests:** for the `.onnx` files the hub's own file listing reported the
  same SHA-256 that was computed from the download. The hub lists no
  SHA-256 for the small `.yml` files; their digest is of the file as
  downloaded from the pinned commit (`sha256_matches_hub_listing` in
  `models.json` records which is which).
- The files are data: ONNX graphs run through ONNX Runtime, and YAML files
  from which the tool reads a character list or three thresholds with its
  own small reader (`src/cand.rs`). Nothing in them is executed.

| repository | commit | file | bytes | SHA-256 | used as |
|---|---|---|---:|---|---|
| `ogkalu/comic-text-and-bubble-detector` | `16e8a622f91fabc6b5b65c96d32d1183f8843546` | `detector-v4-s_int8.onnx` | 11,120,765 | `5fe9e4f576e49d4e7e8b0e029d6d3cdc252abd4694113e1cae120e62c931ea79` | detector `ogk-s` |
| | | `detector_int8.onnx` | 43,838,857 | `b5022ad46416b6fe4f88b0cc082cfd2ff5b1cfc624088c2f19879485493f5913` | detector `ogk-m` |
| `PaddlePaddle/PP-OCRv5_mobile_det_onnx` | `e6f4fa85f00e168c862bc462aebca69eef9b3d3d` | `inference.onnx` | 4,826,518 | `a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d` | line finder (splits `ppdet`, `ppdet2x`, `page-lines`; detectors `ppdetN`) |
| | | `inference.yml` | 903 | `98069072e1b6b37d727fd9d9f11725faa46d6ea0de012f2ed26caea011c37699` | its thresholds |
| `PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx` | `3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b` | `inference.onnx` | 7,848,423 | `b5f833dfc5d0eb71da397b4efa06ebeee9b431b690a47d6af40d77d8eabc557f` | recogniser `pp-v5-en` |
| | | `inference.yml` | 3,964 | `27e91d0582f40168aa218303c76e184bc78fa7a5d105aad0cfbad8458b441067` | its character list |
| `PaddlePaddle/latin_PP-OCRv5_mobile_rec_onnx` | `89d3a50e2c27e2e7cceeab0e944c25c807d5db4f` | `inference.onnx` | 8,042,023 | `7888113072263cb471b93f66dd5e2ad70548dc526fa1ace760d0d973dd121498` | recogniser `pp-v5-latin` |
| | | `inference.yml` | 6,817 | `0bbe984570f597af3638e50bdf2e8276f3ab26a61966096538b3b0d1849f5c84` | its character list |
| `PaddlePaddle/PP-OCRv6_tiny_rec_onnx` | `2612ab37152ae0a677521bae4e1e3d4fb4cf7c30` | `inference.onnx` | 4,462,639 | `9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6` | recogniser `pp-v6-tiny` |
| | | `inference.yml` | 55,571 | `66170210bad538e83fff3c4a3867e547d6bf20b50d64b20347c4b913f3034ea1` | its character list |
| `PaddlePaddle/PP-OCRv6_small_rec_onnx` | `b8f84f0b80c529de40b4fbb3544b84fa7233a513` | `inference.onnx` | 21,159,378 | `5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634` | recogniser `pp-v6-small` |
| | | `inference.yml` | 150,579 | `ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1` | its character list |
| `PaddlePaddle/PP-OCRv6_medium_rec_onnx` | `50c7eacafc52fa7bcf4194e8cd08e46f8558504b` | `inference.onnx` | 76,554,979 | `9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba` | recogniser `pp-v6-medium` |
| | | `inference.yml` | 150,580 | `991b700facf5b50a7de193468207d5f4255b538dde0d312ae3b7c7a9b6873129` | its character list |

### Models that are not in `models.json`

The engines the application ships are not fetched by `fetch.py`:

- **comic-text-detector** (the previous detector, `ctd`) is pinned in
  `crates/tools/ocr-bench/src/engines.rs` (`CTD_MODEL`: repository,
  revision, size, SHA-256). The server no longer fetches it. `ocr-bench`
  loads it only when it is already in the Hugging Face cache with the pinned
  digest, and otherwise stops.
- **Tesseract language data** comes from the `tesseract-rs` build
  (`docs/dev/ocr.md`, "Tesseract data").
