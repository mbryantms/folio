# Changelog

All notable changes to Folio are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (pre-1.0:
minor = features, patch = fixes/polish).

Versioning note: the crate/package manifests stay at `0.0.0` on purpose —
**the git tag is the version**. The running build reports it via
`COMIC_BUILD_TAG` (set from the tag at image-build time). See
[docs/dev/releasing.md](docs/dev/releasing.md) for the release ritual.

Releases before v0.7.2 are recorded only as Git tags + GitHub Releases;
this file starts at the first release that ships with a curated changelog.

## [0.26.10](https://github.com/mbryantms/folio/compare/v0.26.9...v0.26.10) (2026-07-08)


### Dependencies

* lock file maintenance ([#408](https://github.com/mbryantms/folio/issues/408)) ([df9be20](https://github.com/mbryantms/folio/commit/df9be20b5c18a66b0f6e5d9165eb337d4d992620))
* update dependency react-hook-form to v7.81.0 ([#417](https://github.com/mbryantms/folio/issues/417)) ([32c42be](https://github.com/mbryantms/folio/commit/32c42bee7520c3496840e201ba990efe50c5ebd7))
* update dependency recharts to v3.9.2 ([#414](https://github.com/mbryantms/folio/issues/414)) ([01f5e23](https://github.com/mbryantms/folio/commit/01f5e237c1f4c09efead5fa5da911f44d5f30273))
* update pnpm to v11.10.0 ([#416](https://github.com/mbryantms/folio/issues/416)) ([5147d71](https://github.com/mbryantms/folio/commit/5147d7199e32d834b34cb8444e0010ea96c61515))
* update taiki-e/install-action digest to 5041467 ([#413](https://github.com/mbryantms/folio/issues/413)) ([a9f1de9](https://github.com/mbryantms/folio/commit/a9f1de93ad8e08ec87f220794f57828d721d9361))

## [0.26.9](https://github.com/mbryantms/folio/compare/v0.26.8...v0.26.9) (2026-07-06)


### Fixed

* **compose:** mount postgres 18 volumes at /var/lib/postgresql ([#412](https://github.com/mbryantms/folio/issues/412)) ([0c25bde](https://github.com/mbryantms/folio/commit/0c25bdee1cd4ac19e846b4fdbbe86683c0cf9ec5))
* **web:** unify series-card title size with issue-card family ([#410](https://github.com/mbryantms/folio/issues/410)) ([78b53a2](https://github.com/mbryantms/folio/commit/78b53a25a304a61ac0d4ab11322b3ab8e7dc920c))


### Dependencies

* update dependency @scalar/api-reference-react to v0.9.53 ([#409](https://github.com/mbryantms/folio/issues/409)) ([05a7541](https://github.com/mbryantms/folio/commit/05a754111a8646088acd80e42a132fc2bce7a812))

## [0.26.8](https://github.com/mbryantms/folio/compare/v0.26.7...v0.26.8) (2026-07-06)


### Fixed

* **metadata:** series-scope apply persists series-row scalars on the writeback path ([#406](https://github.com/mbryantms/folio/issues/406)) ([0e53527](https://github.com/mbryantms/folio/commit/0e535277440f70048698c8b0a55c0251d1eb355b))

## [0.26.7](https://github.com/mbryantms/folio/compare/v0.26.6...v0.26.7) (2026-07-06)


### Fixed

* **cbl:** Up Next walks entry pages before scrolling on long lists ([#404](https://github.com/mbryantms/folio/issues/404)) ([1fa9fe2](https://github.com/mbryantms/folio/commit/1fa9fe2fea9d2168395b7a702c86a48db8402a01))


### Dependencies

* update dependency @scalar/api-reference-react to v0.9.52 ([#405](https://github.com/mbryantms/folio/issues/405)) ([5555b4b](https://github.com/mbryantms/folio/commit/5555b4b3663c2d47ba53fdced770808a4a9856a8))
* update taiki-e/install-action digest to 4684b84 ([#402](https://github.com/mbryantms/folio/issues/402)) ([e302581](https://github.com/mbryantms/folio/commit/e3025811072def1547770d0832404e62937f2e66))

## [0.26.6](https://github.com/mbryantms/folio/compare/v0.26.5...v0.26.6) (2026-07-05)


### Fixed

* **pwa:** per-open stamp so editor tiles defeat not-yet-activated workers ([#399](https://github.com/mbryantms/folio/issues/399)) ([f4ed93a](https://github.com/mbryantms/folio/commit/f4ed93afa1cd74fefcde75f57753ceca7df16be5))


### Dependencies

* update nextjs monorepo to v16.2.10 ([#400](https://github.com/mbryantms/folio/issues/400)) ([247d576](https://github.com/mbryantms/folio/commit/247d5762d4e411a132615bc2bae1a4b1fe0c2ec5))

## [0.26.5](https://github.com/mbryantms/folio/compare/v0.26.4...v0.26.5) (2026-07-04)


### Fixed

* **archive-edit:** apply EXIF orientation at decode + stop dropping busy edits ([#396](https://github.com/mbryantms/folio/issues/396)) ([b7a4a49](https://github.com/mbryantms/folio/commit/b7a4a4900a43ffda213ea063e836950fede45314))
* **pwa:** editor tiles bypass the service worker instead of nonce-busting ([#398](https://github.com/mbryantms/folio/issues/398)) ([6f978da](https://github.com/mbryantms/folio/commit/6f978daf7583f7ffef8f61788c4ec24247e8c9ca))

## [0.26.4](https://github.com/mbryantms/folio/compare/v0.26.3...v0.26.4) (2026-07-04)


### Fixed

* **pwa:** stop the service worker from re-serving pre-fix stale thumbnails ([#395](https://github.com/mbryantms/folio/issues/395)) ([d1528a8](https://github.com/mbryantms/folio/commit/d1528a8129ced1100a19ba278edc1ade28df9608))


### Dependencies

* update dependency lucide-react to v1.23.0 ([#393](https://github.com/mbryantms/folio/issues/393)) ([ebf128e](https://github.com/mbryantms/folio/commit/ebf128e908ed9b3f9798791773682f89dbc4adf3))

## [0.26.3](https://github.com/mbryantms/folio/compare/v0.26.2...v0.26.3) (2026-07-04)


### Fixed

* **archive-edit:** correct stale page dims on rescan + version page/thumb URLs ([#391](https://github.com/mbryantms/folio/issues/391)) ([2dcfcf5](https://github.com/mbryantms/folio/commit/2dcfcf55eef4a3b20eb46027e051157151cee3a8))

## [0.26.2](https://github.com/mbryantms/folio/compare/v0.26.1...v0.26.2) (2026-07-04)


### Fixed

* **archive-edit:** stale thumbnails after page edits (server + HTTP caching) ([#390](https://github.com/mbryantms/folio/issues/390)) ([47b351e](https://github.com/mbryantms/folio/commit/47b351ea262651b7c3e1d69b6f3302f0c3666d9a))


### Dependencies

* update radix-ui ([#387](https://github.com/mbryantms/folio/issues/387)) ([b05f95b](https://github.com/mbryantms/folio/commit/b05f95beac4071dfb036f64ec9506d5b99d160da))
* update taiki-e/install-action digest to c93ccc0 ([#389](https://github.com/mbryantms/folio/issues/389)) ([cee86ab](https://github.com/mbryantms/folio/commit/cee86ab5957986afc6730972d20cff788fbf0969))

## [0.26.1](https://github.com/mbryantms/folio/compare/v0.26.0...v0.26.1) (2026-07-03)


### Dependencies

* lock file maintenance ([#386](https://github.com/mbryantms/folio/issues/386)) ([a88bd60](https://github.com/mbryantms/folio/commit/a88bd60bb43a0450bfbdda909b44b5f1388f7d02))
* pin dependencies ([#371](https://github.com/mbryantms/folio/issues/371)) ([854c318](https://github.com/mbryantms/folio/commit/854c3182b3c5f013ca695615f54bc34b7e1a59dd))
* update actions/cache action to v6 ([#375](https://github.com/mbryantms/folio/issues/375)) ([d573b6c](https://github.com/mbryantms/folio/commit/d573b6c6d7fb84260fad2841d60187a043d3e9cf))
* update actions/checkout action to v7 ([#376](https://github.com/mbryantms/folio/issues/376)) ([76852c0](https://github.com/mbryantms/folio/commit/76852c097ab186f19b672f359321d2e6115d1734))
* update dependency @playwright/test to v1.61.1 ([#363](https://github.com/mbryantms/folio/issues/363)) ([492780b](https://github.com/mbryantms/folio/commit/492780b3c05e08a7874fae6f8ca30b1b499f218e))
* update dependency @scalar/api-reference-react to v0.9.50 ([#355](https://github.com/mbryantms/folio/issues/355)) ([ad347c0](https://github.com/mbryantms/folio/commit/ad347c0f2211160d18cb6b396da3f068f943248e))
* update dependency cronstrue to v3.24.0 ([#364](https://github.com/mbryantms/folio/issues/364)) ([1ee00db](https://github.com/mbryantms/folio/commit/1ee00dbfa0c79632cb287f35e1903a2c9c4705a5))
* update dependency js-yaml@&gt;=4.0.0 &lt;4.1.1 to v4.3.0 ([#365](https://github.com/mbryantms/folio/issues/365)) ([4be89ce](https://github.com/mbryantms/folio/commit/4be89cecf3981a7eb061da12e5acd74d63279beb))
* update dependency lucide-react to v1.22.0 ([#366](https://github.com/mbryantms/folio/issues/366)) ([944e36e](https://github.com/mbryantms/folio/commit/944e36e7680b84e1aad8b448644813a0c5c0401a))
* update dependency next-intl to v4.13.1 ([#385](https://github.com/mbryantms/folio/issues/385)) ([a20bbf9](https://github.com/mbryantms/folio/commit/a20bbf9c1831a19e8275cf1432e903d988e32595))
* update dependency postcss to v8.5.16 ([#356](https://github.com/mbryantms/folio/issues/356)) ([cde8401](https://github.com/mbryantms/folio/commit/cde8401b20997406e638ebec45e895df0dc45b5f))
* update dependency postcss@&lt;8.5.10 to v8.5.16 ([#357](https://github.com/mbryantms/folio/issues/357)) ([4d1322b](https://github.com/mbryantms/folio/commit/4d1322bf103b1cf4cf31447ee81499ee9f75c900))
* update dependency prettier to v3.9.4 ([#367](https://github.com/mbryantms/folio/issues/367)) ([54ef3bb](https://github.com/mbryantms/folio/commit/54ef3bb3b4f70a56ad1203f98878d9fc296434d3))
* update dependency recharts to v3.9.1 ([#382](https://github.com/mbryantms/folio/issues/382)) ([97f1cd9](https://github.com/mbryantms/folio/commit/97f1cd951192a69146ecc2240574518d42a3810c))
* update dependency vite to v8.1.1 ([#381](https://github.com/mbryantms/folio/issues/381)) ([e9a94d9](https://github.com/mbryantms/folio/commit/e9a94d9ca64b33fb40ceef5a8d51f73f4ab35572))
* update docker/login-action digest to af1e73f ([#384](https://github.com/mbryantms/folio/issues/384)) ([17c4619](https://github.com/mbryantms/folio/commit/17c4619db686de5ef51b425ad967342585aca727))
* update pnpm to v11.9.0 ([#369](https://github.com/mbryantms/folio/issues/369)) ([9c95327](https://github.com/mbryantms/folio/commit/9c953270f7355d03630c98fb39dca76c2b57999f))
* update rust crate arc-swap to v1.9.2 ([#358](https://github.com/mbryantms/folio/issues/358)) ([baa09c2](https://github.com/mbryantms/folio/commit/baa09c2c24a4f1d0f4079e876ff565f2a75551c3))
* update rust crate log to v0.4.33 ([#359](https://github.com/mbryantms/folio/issues/359)) ([dc02270](https://github.com/mbryantms/folio/commit/dc02270ade6be37721d02c550ce3a76805ce5468))
* update rust crate tower-http to 0.7 ([#372](https://github.com/mbryantms/folio/issues/372)) ([93b02d0](https://github.com/mbryantms/folio/commit/93b02d0513d9770292f92dc221607db50984f904))
* update rust crate zeroize to v1.9.0 ([#373](https://github.com/mbryantms/folio/issues/373)) ([dd6e993](https://github.com/mbryantms/folio/commit/dd6e99386b7320e8c5c832b3cf8f0d3386855e06))
* update rust to v1.96.1 ([#360](https://github.com/mbryantms/folio/issues/360)) ([2a43f0b](https://github.com/mbryantms/folio/commit/2a43f0ba6fe0f6da1585be9c92761be3143fc7b6))
* update tailwindcss monorepo to v4.3.2 ([#361](https://github.com/mbryantms/folio/issues/361)) ([6c880eb](https://github.com/mbryantms/folio/commit/6c880ebaf8504efaf5b0f654f7e8e4597ac095c7))
* update tanstack ([#362](https://github.com/mbryantms/folio/issues/362)) ([e67c392](https://github.com/mbryantms/folio/commit/e67c392c6c738bec5f7c976d49d8fa3a79da3256))

## [0.26.0](https://github.com/mbryantms/folio/compare/v0.25.0...v0.26.0) (2026-07-02)


### Added

* **observability:** wave 1 audit remediation (request-id correlation, panic capture, handler spans, slow-query + pool metrics) ([#332](https://github.com/mbryantms/folio/issues/332)) ([65e512a](https://github.com/mbryantms/folio/commit/65e512a7e9749c7f9c632279ffb8832a2081b522))
* **ux:** mobile polish — dialog bottom sheets + instant reader tap (UX-7, UX-8) ([#342](https://github.com/mbryantms/folio/issues/342)) ([79bcb4f](https://github.com/mbryantms/folio/commit/79bcb4fd26b83f6e16ff72c9b8c0ee6e9f8fdfe1))
* **ux:** wave 4 audit remediation — onboarding, curator workflows, OPDS search paging ([#338](https://github.com/mbryantms/folio/issues/338)) ([749962d](https://github.com/mbryantms/folio/commit/749962d3514a266e47c8dd02637ff29ad995ba8a))


### Fixed

* **observability:** stamp scan events with their batch id + backfill history ([#346](https://github.com/mbryantms/folio/issues/346)) ([8b3c625](https://github.com/mbryantms/folio/commit/8b3c62571eb62736c001bdaa306376f57cf39eef))
* **security:** wave 0 audit remediation (decode caps, session revocation, error hygiene, deps) ([#331](https://github.com/mbryantms/folio/issues/331)) ([990d5e4](https://github.com/mbryantms/folio/commit/990d5e42fb6b6d5f51aa122e230a7471551aed31))


### Changed

* **backend:** wave 2a audit remediation (audit_log index, batched collection add, concurrent issue-detail lookups) ([#334](https://github.com/mbryantms/folio/issues/334)) ([903e47b](https://github.com/mbryantms/folio/commit/903e47b06a197efd8e456740822fab1914d62025))
* **metadata:** single-flight the provider cache (PERF-4) ([#336](https://github.com/mbryantms/folio/issues/336)) ([0cfeae1](https://github.com/mbryantms/folio/commit/0cfeae1fb27be8433a50df9cd36653eb6d6853ec))
* **reader:** reserve page dimensions + race /auth/me (FEP-2, FEP-4) ([#337](https://github.com/mbryantms/folio/issues/337)) ([c14f7b7](https://github.com/mbryantms/folio/commit/c14f7b719f7d3616580a82c5d9e32fa53c2a9613))
* **reader:** width-negotiated page variants with on-disk cache (FEP-1) ([#343](https://github.com/mbryantms/folio/issues/343)) ([17eb602](https://github.com/mbryantms/folio/commit/17eb6024638d2a691f3627bc49e93f0897832c0c))
* **scanner:** gate the post-folder metadata rollup on actual ingest (PERF-2) ([#341](https://github.com/mbryantms/folio/issues/341)) ([69c1f1a](https://github.com/mbryantms/folio/commit/69c1f1a65e08963b0db3d58c57a56bd021238c19))
* **scanner:** wave 2b — batch reconcile queries (PERF-7, PERF-9) ([#335](https://github.com/mbryantms/folio/issues/335)) ([c03ce60](https://github.com/mbryantms/folio/commit/c03ce60a8f9456c7835a1afe6b164fa70b45a787))
* wave 2/3 residue — SW thumb cache, scan-manifest projection, query-count guards (FEP-3, PERF-6, PERF-12) ([#340](https://github.com/mbryantms/folio/issues/340)) ([55414d0](https://github.com/mbryantms/folio/commit/55414d01bfd71e7c2a68517b2b6ccd73f43dc929))

## [0.25.0](https://github.com/mbryantms/folio/compare/v0.24.0...v0.25.0) (2026-06-28)


### Added

* only show the "Appears in" tab when there are appearances ([#327](https://github.com/mbryantms/folio/issues/327)) ([2f05930](https://github.com/mbryantms/folio/commit/2f059305e5bcb108576f917e04ac48ba8403013a))


### Fixed

* make CBL "Search this list" matching case-insensitive and broader ([#328](https://github.com/mbryantms/folio/issues/328)) ([ec6b524](https://github.com/mbryantms/folio/commit/ec6b524930afb2d9e0b5ce1251b3b586f23858f1))
* make user-facing searches case-insensitive and multi-term ([#329](https://github.com/mbryantms/folio/issues/329)) ([af80ba1](https://github.com/mbryantms/folio/commit/af80ba1d9160cb0e3acc85996b6d45f0d2df0fe8))

## [0.24.0](https://github.com/mbryantms/folio/compare/v0.23.2...v0.24.0) (2026-06-28)


### Added

* add "Appears in" tab to issue & series pages ([#324](https://github.com/mbryantms/folio/issues/324)) ([00e87b1](https://github.com/mbryantms/folio/commit/00e87b102ec879b76d8fdaa27daf2551eae4651a))
* reading-list refresh dashboard with review-before-apply ([#325](https://github.com/mbryantms/folio/issues/325)) ([cebc2d4](https://github.com/mbryantms/folio/commit/cebc2d400d7e6e062d138a659ca5233839452aa1))


### Fixed

* keep page-strip thumbs mounted through chrome-hide slide on Tailwind v4 ([#322](https://github.com/mbryantms/folio/issues/322)) ([d69e78d](https://github.com/mbryantms/folio/commit/d69e78da927e0a3f6cf9821c6d882903eb26a38c))
* stop dialog scroll body from clipping focus rings ([#323](https://github.com/mbryantms/folio/issues/323)) ([428f73c](https://github.com/mbryantms/folio/commit/428f73c4e53e0545eb2f931bbac97fae63c03b00))

## [0.23.2](https://github.com/mbryantms/folio/compare/v0.23.1...v0.23.2) (2026-06-28)


### Dependencies

* update dependency cron-parser to v5.6.1 ([#312](https://github.com/mbryantms/folio/issues/312)) ([b720004](https://github.com/mbryantms/folio/commit/b7200043605e25b1a1a152b002ed791e291c0928))

## [0.23.1](https://github.com/mbryantms/folio/compare/v0.23.0...v0.23.1) (2026-06-24)


### Fixed

* **deps:** bump quinn-proto to 0.11.15 (RUSTSEC-2026-0185) ([#302](https://github.com/mbryantms/folio/issues/302)) ([0a3b0f2](https://github.com/mbryantms/folio/commit/0a3b0f23cb55159912282650bcda5fd77f4af0e1))


### Dependencies

* update dependency @axe-core/playwright to v4.12.1 ([#308](https://github.com/mbryantms/folio/issues/308)) ([de9a050](https://github.com/mbryantms/folio/commit/de9a050e06b261143220c72fda4d632e3d38b7eb))
* update dependency @vitejs/plugin-react to v6.0.3 ([#299](https://github.com/mbryantms/folio/issues/299)) ([3c87f74](https://github.com/mbryantms/folio/commit/3c87f743cc3fa5f8914df724e4929755626ab679))
* update dependency cron-parser to v5.6.0 ([#310](https://github.com/mbryantms/folio/issues/310)) ([27d4ead](https://github.com/mbryantms/folio/commit/27d4eadab04d0f7159f099db95b256abe01c44c3))
* update dependency cronstrue to v3.21.0 ([#311](https://github.com/mbryantms/folio/issues/311)) ([12ce676](https://github.com/mbryantms/folio/commit/12ce6764722ceb6f2fb2d10c4c3609ed15238b08))
* update dependency react-hook-form to v7.80.0 ([#313](https://github.com/mbryantms/folio/issues/313)) ([8315f23](https://github.com/mbryantms/folio/commit/8315f2325cd0079d32a002ef8d7b40d7c830d1fe))
* update dependency recharts to v3.9.0 ([#314](https://github.com/mbryantms/folio/issues/314)) ([6b288ca](https://github.com/mbryantms/folio/commit/6b288cac71a4e8c59179f3ab6796e6280351ac44))
* update dependency vite to v8.1.0 ([#315](https://github.com/mbryantms/folio/issues/315)) ([c19243e](https://github.com/mbryantms/folio/commit/c19243e207f5c5d0e5c866fde69af0b6e375b790))
* update docker/dockerfile docker tag to v1.25 ([#318](https://github.com/mbryantms/folio/issues/318)) ([762377f](https://github.com/mbryantms/folio/commit/762377f66ded0269d33b593325f83862026bac12))
* update rust crate quote to v1.0.46 ([#300](https://github.com/mbryantms/folio/issues/300)) ([c49aaae](https://github.com/mbryantms/folio/commit/c49aaaefff76888c810b6abd26b0ec8ef144effa))
* update rust crate syn to v2.0.118 ([#303](https://github.com/mbryantms/folio/issues/303)) ([59931fd](https://github.com/mbryantms/folio/commit/59931fd9f9330b17d480f8a9a763f70393df9daa))
* update rust crate time to v0.3.51 ([#304](https://github.com/mbryantms/folio/issues/304)) ([8fccd6f](https://github.com/mbryantms/folio/commit/8fccd6f75940a6850093385653c1eeec3c6e8ed2))
* update rust crate uuid to v1.23.4 ([#317](https://github.com/mbryantms/folio/issues/317)) ([2165498](https://github.com/mbryantms/folio/commit/216549848e5fd5a7bec98a5887dca15e6fe27211))
* update scalar monorepo ([#305](https://github.com/mbryantms/folio/issues/305)) ([a8108f4](https://github.com/mbryantms/folio/commit/a8108f4790acd388fb4cba0094c30416c8e6bc93))
* update tanstack ([#306](https://github.com/mbryantms/folio/issues/306)) ([f80c53d](https://github.com/mbryantms/folio/commit/f80c53d34f5ea9fcdad0531bc6f2186567b69efe))
* update vitest monorepo ([#307](https://github.com/mbryantms/folio/issues/307)) ([af1a9e4](https://github.com/mbryantms/folio/commit/af1a9e47c4b9fbb5b77a7987acb072bf91859931))

## [0.23.0](https://github.com/mbryantms/folio/compare/v0.22.6...v0.23.0) (2026-06-23)


### Added

* **metadata:** provider series-boundary divergence (split/merged series) ([#296](https://github.com/mbryantms/folio/issues/296)) ([dd9bde8](https://github.com/mbryantms/folio/commit/dd9bde88e8866c2168f160596ff4879c417de13a))

## [0.22.6](https://github.com/mbryantms/folio/compare/v0.22.5...v0.22.6) (2026-06-22)


### Changed

* **rails:** project slim columns for On Deck hydration ([#295](https://github.com/mbryantms/folio/issues/295)) ([5a8c094](https://github.com/mbryantms/folio/commit/5a8c09433dee2da53ca9af02e01e9097bb06665f))


### Dependencies

* update dependency serialize-javascript@&lt;7.0.5 to v7.0.6 ([#292](https://github.com/mbryantms/folio/issues/292)) ([a58d5ec](https://github.com/mbryantms/folio/commit/a58d5ec5209a0c5fe28ce7c0c758f26a3cda6aed))
* update radix-ui ([#293](https://github.com/mbryantms/folio/issues/293)) ([d179f48](https://github.com/mbryantms/folio/commit/d179f484a2c750ceb84686f67d8cd501982355b3))

## [0.22.5](https://github.com/mbryantms/folio/compare/v0.22.4...v0.22.5) (2026-06-21)


### Changed

* **rails:** reduce On Deck rail work ([#290](https://github.com/mbryantms/folio/issues/290)) ([8cb5fcc](https://github.com/mbryantms/folio/commit/8cb5fcc07249a772231758d3b3ef78720f1eb03b))

## [0.22.4](https://github.com/mbryantms/folio/compare/v0.22.3...v0.22.4) (2026-06-21)


### Fixed

* **ui:** flatten dropdown submenus on touch so they aren't off-screen ([#288](https://github.com/mbryantms/folio/issues/288)) ([a16cd25](https://github.com/mbryantms/folio/commit/a16cd2568f8ca0647ac2202ec443dc88d06964f0))

## [0.22.3](https://github.com/mbryantms/folio/compare/v0.22.2...v0.22.3) (2026-06-20)


### Fixed

* **ui:** keep dropdown menus + submenus inside the safe area ([#285](https://github.com/mbryantms/folio/issues/285)) ([5c13a86](https://github.com/mbryantms/folio/commit/5c13a861e5bcaabb6978561435d6f683d10163a8))
* **ui:** keep popover/select/hover-card/tooltip inside the safe area ([#286](https://github.com/mbryantms/folio/issues/286)) ([cabed53](https://github.com/mbryantms/folio/commit/cabed53a602b314d493982403c86f661db8bb79c))

## [0.22.2](https://github.com/mbryantms/folio/compare/v0.22.1...v0.22.2) (2026-06-20)


### Changed

* **rails:** batch On Deck next-up picks (O(1) round-trips) ([#283](https://github.com/mbryantms/folio/issues/283)) ([790136c](https://github.com/mbryantms/folio/commit/790136c795dbcad41dfde93f312ab43597a075cb))

## [0.22.1](https://github.com/mbryantms/folio/compare/v0.22.0...v0.22.1) (2026-06-20)


### Fixed

* **web:** reliably anchor CBL home rails on the Up Next issue ([#281](https://github.com/mbryantms/folio/issues/281)) ([96be135](https://github.com/mbryantms/folio/commit/96be135eb8b678415c1e66acbfd3787f7c5a0df8))

## [0.22.0](https://github.com/mbryantms/folio/compare/v0.21.0...v0.22.0) (2026-06-18)


### Added

* **admin:** create user with a one-time temp password (3.8 / D9) ([#280](https://github.com/mbryantms/folio/issues/280)) ([2bfa3d9](https://github.com/mbryantms/folio/commit/2bfa3d91db23d1bdd7b815cb275f5239457dc86d))
* **collections:** recently-used collections in the add-to-collection picker (3.7) ([#276](https://github.com/mbryantms/folio/issues/276)) ([3efdbb7](https://github.com/mbryantms/folio/commit/3efdbb7d257f924d775d15d6106662e5f50c2ebf))
* **collections:** undo collection deletes (B6) ([#274](https://github.com/mbryantms/folio/issues/274)) ([a824a8d](https://github.com/mbryantms/folio/commit/a824a8d516318e5aadf66de930a5978fd93e31e2))
* **discovery:** card hover previews + surprise-me on the Recently Added rail (3.7) ([#278](https://github.com/mbryantms/folio/issues/278)) ([d74a3c2](https://github.com/mbryantms/folio/commit/d74a3c2f01ae7aa5c9f65b25501d8078c5d8bece))

## [0.21.0](https://github.com/mbryantms/folio/compare/v0.20.0...v0.21.0) (2026-06-18)


### Added

* **covers:** responsive cover variants — small [@sm](https://github.com/sm) tier + srcset (G9) ([#270](https://github.com/mbryantms/folio/issues/270)) ([5a37d8f](https://github.com/mbryantms/folio/commit/5a37d8fbed7bb86e606f42c4be4c723c3e2b27bb))


### Fixed

* **a11y:** keyboard-focusable widget handle + heading hierarchy + cover announce (E8/E9) ([#271](https://github.com/mbryantms/folio/issues/271)) ([51661dc](https://github.com/mbryantms/folio/commit/51661dc33b0de8b1c816e9257831b0f933725a5d))
* **web:** purpose-shaped empty states + caught-up/search copy (3.7) ([#272](https://github.com/mbryantms/folio/issues/272)) ([cccd5d7](https://github.com/mbryantms/folio/commit/cccd5d78dadb9ceb0dc3458412ebdd9b0567c6c1))

## [0.20.0](https://github.com/mbryantms/folio/compare/v0.19.0...v0.20.0) (2026-06-17)


### Added

* **jobs:** run cover-phash + variant-cover backfills as apalis jobs (B17) ([#268](https://github.com/mbryantms/folio/issues/268)) ([1d88f57](https://github.com/mbryantms/folio/commit/1d88f5708258b80321e650674ecd19b09875be07))
* **library:** surface bulk-archive skip reasons + keep skipped selected (B17) ([#264](https://github.com/mbryantms/folio/issues/264)) ([241be22](https://github.com/mbryantms/folio/commit/241be22dd4f01a91bd8c336258b1dc873df4b715))
* **metadata:** add a "Recent applies" feed to the metadata dashboard (B14) ([#267](https://github.com/mbryantms/folio/issues/267)) ([e1e56fb](https://github.com/mbryantms/folio/commit/e1e56fbf0062bf07ffff8555d5f89a8c21f938f6))
* **metadata:** surface provider quota + retry ETA in the match dialog (B13) ([#265](https://github.com/mbryantms/folio/issues/265)) ([ba8a5f2](https://github.com/mbryantms/folio/commit/ba8a5f2e777667fb3a48d13d4422f64be568eb5f))


### Fixed

* **web:** skeleton loading states match their real page layouts ([#269](https://github.com/mbryantms/folio/issues/269)) ([dca255a](https://github.com/mbryantms/folio/commit/dca255a52485203def7f9281cb53cf2f9e871137))

## [0.19.0](https://github.com/mbryantms/folio/compare/v0.18.0...v0.19.0) (2026-06-17)


### Added

* **admin:** surface + retry + purge dead-lettered background jobs (D8b) ([#262](https://github.com/mbryantms/folio/issues/262)) ([345872c](https://github.com/mbryantms/folio/commit/345872cb622be5ac5e92c96f0a0f3c9aaa56cc2a))
* **scanner:** cooperative scan-cancel — worker drains without reconcile (D8a) ([#260](https://github.com/mbryantms/folio/issues/260)) ([1732848](https://github.com/mbryantms/folio/commit/1732848fe4ffcdd4db1f1ed51b2f178039065daa))


### Fixed

* **metadata:** don't fail sidecar rewrite when a duplicate/nested ComicInfo is dropped ([#263](https://github.com/mbryantms/folio/issues/263)) ([4e8977a](https://github.com/mbryantms/folio/commit/4e8977af75d0ff39f4ec78e9fd5eff9f50c584de))

## [0.18.0](https://github.com/mbryantms/folio/compare/v0.17.0...v0.18.0) (2026-06-17)


### Added

* **collections:** CBL export endpoint (3.3c) ([#249](https://github.com/mbryantms/folio/issues/249)) ([509d29b](https://github.com/mbryantms/folio/commit/509d29b79088c46779844e5d71a1606ea30b845c))
* **creators:** search box on the browse index (web-only follow-up) ([#256](https://github.com/mbryantms/folio/issues/256)) ([890a936](https://github.com/mbryantms/folio/commit/890a9367f96c275377a029def0d042a2dbf8d43c))
* **library:** copy-link / share affordances (3.3a) ([#246](https://github.com/mbryantms/folio/issues/246)) ([0a2b306](https://github.com/mbryantms/folio/commit/0a2b306b66bb2972a7dfb4f0bdae6bd0a3625625))
* **library:** inline single-field metadata edit on the issue Details tab (3.4/B12) ([#255](https://github.com/mbryantms/folio/issues/255)) ([85dad93](https://github.com/mbryantms/folio/commit/85dad934b043175cb02720d32c393c043ac4fb72))
* **library:** paginated creators browse index + sidebar entry (3.4/A11) ([#250](https://github.com/mbryantms/folio/issues/250)) ([2936742](https://github.com/mbryantms/folio/commit/29367425b8cacd95b0ff912b81bd1c6ae977b278))
* **reading-log:** CSV export endpoint (3.3b) ([#247](https://github.com/mbryantms/folio/issues/247)) ([d6536e1](https://github.com/mbryantms/folio/commit/d6536e16b3757c5be8ffdef8457e6bb0b46eec46))
* **search/browse:** A–Z jump rail (starts_with) on library grid + creators (3.4/B9) ([#253](https://github.com/mbryantms/folio/issues/253)) ([2b1532b](https://github.com/mbryantms/folio/commit/2b1532be12ed7be504df03fbda76fb33e5787da2))
* **search:** read-status facet on issue results (3.4) ([#252](https://github.com/mbryantms/folio/issues/252)) ([af1d80d](https://github.com/mbryantms/folio/commit/af1d80d49d207c30c6f70e3316ccb4e027ee499a))
* **search:** trigram typo-tolerance on issue search (3.4/B7) ([#254](https://github.com/mbryantms/folio/issues/254)) ([fe76957](https://github.com/mbryantms/folio/commit/fe76957c407e297882937028ce4e010560e2ad1c))


### Fixed

* **saved-views:** infinite-scroll the filter-view detail page (built-in views) ([#257](https://github.com/mbryantms/folio/issues/257)) ([32e9b57](https://github.com/mbryantms/folio/commit/32e9b57be6c49c408519ae6432938dbefc1f6a0c))


### Changed

* **web:** seed the SSR me + sidebar payloads into the query cache (G7) ([#258](https://github.com/mbryantms/folio/issues/258)) ([a1ddead](https://github.com/mbryantms/folio/commit/a1ddead42257c5ebb7a435dc1533d73e5669b73e))
* **web:** stop rebuilding infinite-scroll observers every render (G10) ([#259](https://github.com/mbryantms/folio/issues/259)) ([e0fb05b](https://github.com/mbryantms/folio/commit/e0fb05b46d8e0d449c344522db3e75bb3cf8ddba))

## [0.17.0](https://github.com/mbryantms/folio/compare/v0.16.0...v0.17.0) (2026-06-16)


### Added

* **shell:** IA renames + breadcrumbs (3.1b) ([#245](https://github.com/mbryantms/folio/issues/245)) ([c55b2ae](https://github.com/mbryantms/folio/commit/c55b2ae3080408715fad857f8f3df7b8ec5b3f31))
* **shell:** mobile bottom nav + wayfinding (3.1a) ([#243](https://github.com/mbryantms/folio/issues/243)) ([4e7f63a](https://github.com/mbryantms/folio/commit/4e7f63a34aa516143f584ac91049d5068183b1f5))

## [0.16.0](https://github.com/mbryantms/folio/compare/v0.15.0...v0.16.0) (2026-06-16)


### Added

* **library:** unified /views index for saved content (3.2 / A3) ([#239](https://github.com/mbryantms/folio/issues/239)) ([43ad3d5](https://github.com/mbryantms/folio/commit/43ad3d5fed6619008c6ac850de517a39efeadaa7))
* **search:** saved-content palette sources + admin search trigger (A4) ([#240](https://github.com/mbryantms/folio/issues/240)) ([37cdae5](https://github.com/mbryantms/folio/commit/37cdae521c07966e707166633a0f004b1fa6c8c3))

## [0.15.0](https://github.com/mbryantms/folio/compare/v0.14.0...v0.15.0) (2026-06-16)


### Added

* **cbl:** bulk-resolve similar entries + auto-advance (2.6 / B10) ([#237](https://github.com/mbryantms/folio/issues/237)) ([c3f2858](https://github.com/mbryantms/folio/commit/c3f285857ef9edd389c7a1cb626f773424170b46))
* **metadata:** "mark metadata complete" escape hatch (2.6 / B4 server) ([#230](https://github.com/mbryantms/folio/issues/230)) ([cd411c1](https://github.com/mbryantms/folio/commit/cd411c1914406656b1a457e9448f6ab2b843577d))
* **metadata:** "Needs metadata" chips deep-link to the match dialog (2.6 / B4) ([#233](https://github.com/mbryantms/folio/issues/233)) ([0738c3b](https://github.com/mbryantms/folio/commit/0738c3bffbc77ca555ee40cfa3ee61e633d11a6b))
* **metadata:** auto-advance through the needs-metadata worklist (2.6 / B4) ([#235](https://github.com/mbryantms/folio/issues/235)) ([5a3a531](https://github.com/mbryantms/folio/commit/5a3a53193175051586719b73765520f996489580))
* **metadata:** bulk fetch routes through a batch + "Review results" (2.6 / B5) ([#236](https://github.com/mbryantms/folio/issues/236)) ([675d341](https://github.com/mbryantms/folio/commit/675d3414e41debbe55500a2694cb8e109f3ec3b9))
* **metadata:** URL-addressable "Needs metadata" worklist grid (2.6 / B4) ([#234](https://github.com/mbryantms/folio/issues/234)) ([68991c3](https://github.com/mbryantms/folio/commit/68991c39679e64fd862ec4a7a37de5307df455c8))


### Fixed

* **metadata:** honor the accepted overlay in the saved-view completeness filter (2.6 / B4) ([#232](https://github.com/mbryantms/folio/issues/232)) ([45be605](https://github.com/mbryantms/folio/commit/45be6053f9430979bec55af8fb09dbbcd5666c47))


### Changed

* **web:** lazy-load the heavy series/issue dialogs (2.6 / G6) ([#229](https://github.com/mbryantms/folio/issues/229)) ([2a0aea7](https://github.com/mbryantms/folio/commit/2a0aea76fa442aebb0f2c1a94a62def8e2c3dcc5))


### Dependencies

* update lucide monorepo to v1.18.0 ([#225](https://github.com/mbryantms/folio/issues/225)) ([007b189](https://github.com/mbryantms/folio/commit/007b18908e7debbb2c1998cdc82209cd12b33efb))
* update pnpm to v11.7.0 ([#226](https://github.com/mbryantms/folio/issues/226)) ([ab65664](https://github.com/mbryantms/folio/commit/ab65664921dca42f042f7f7d1f2705b385c62d3c))

## [0.14.0](https://github.com/mbryantms/folio/compare/v0.13.1...v0.14.0) (2026-06-15)


### Added

* **admin:** cursor-paginate per-library scan runs + users total (D5/D9) ([#203](https://github.com/mbryantms/folio/issues/203)) ([e728cd3](https://github.com/mbryantms/folio/commit/e728cd363a8c18b4bd642e33efeea00539802df4))
* **admin:** finish admin-form polish — reset, thumbnail split, resync, error binding (2.8 / D7 + H2) ([#219](https://github.com/mbryantms/folio/issues/219)) ([e69ff89](https://github.com/mbryantms/folio/commit/e69ff89cc07816d27ef1c78048ba19b73393e708))
* **admin:** metadata refresh cron uses CronInput + restart hint (2.8 / D9) ([#217](https://github.com/mbryantms/folio/issues/217)) ([6ec2f3d](https://github.com/mbryantms/folio/commit/6ec2f3d26ced66d9de1b6798d65a8b0eb05a398a))
* **admin:** restart-pending banner from boot-vs-current settings (2.8 / D9) ([#218](https://github.com/mbryantms/folio/issues/218)) ([408c39d](https://github.com/mbryantms/folio/commit/408c39dea552eca607a0883df0b2635e5cab85e5))
* **admin:** server-paginated + faceted per-library health-issues (D5) ([#209](https://github.com/mbryantms/folio/issues/209)) ([472d0c8](https://github.com/mbryantms/folio/commit/472d0c8e83aa850a98f8e138a39cdeeb09c61d6c))
* **admin:** unsaved-changes guard on the long admin forms (2.8 / D6) ([#215](https://github.com/mbryantms/folio/issues/215)) ([cb6bfa8](https://github.com/mbryantms/folio/commit/cb6bfa8cbd7f3d43cdd672454d776ad3f0595957))
* **series/issue:** gear-icon actions + Read incognito ([#205](https://github.com/mbryantms/folio/issues/205)) ([087eeb2](https://github.com/mbryantms/folio/commit/087eeb21368bcbe8323177f7745775e8f4109198))


### Fixed

* **admin:** per-row pending + optimistic-hide rollback on admin lists (2.8 / D7) ([#216](https://github.com/mbryantms/folio/issues/216)) ([c94c273](https://github.com/mbryantms/folio/commit/c94c2739fb88fc773a3b20d519bbf4ee64871db2))
* **admin:** wrap audit payload + widen beacon clear target (D10) ([#204](https://github.com/mbryantms/folio/issues/204)) ([05e46d7](https://github.com/mbryantms/folio/commit/05e46d7a0c27a4304dbbb8e25588035a8306b812))


### Changed

* **test:** integration tests via cargo-nextest on a shared external Postgres (CI-speed Phase 2) ([#210](https://github.com/mbryantms/folio/issues/210)) ([fc46c81](https://github.com/mbryantms/folio/commit/fc46c815c17b3f9e8794ad8424d62fd6da9cfdca))
* **test:** share Redis across tests + oversubscribe nextest (CI-speed Phase 3) ([#222](https://github.com/mbryantms/folio/issues/222)) ([6ce7635](https://github.com/mbryantms/folio/commit/6ce763561df62aadd406ca8ca1c0e9f2d73b186a))
* **test:** shared Postgres + template-DB clone per test (CI-speed Phase 1) ([#208](https://github.com/mbryantms/folio/issues/208)) ([e82ffc6](https://github.com/mbryantms/folio/commit/e82ffc6175b36c9809bb02b67305b6ba88ce5b52))


### Dependencies

* update dependency react-hook-form to v7.79.0 ([#223](https://github.com/mbryantms/folio/issues/223)) ([6891584](https://github.com/mbryantms/folio/commit/68915840b42a94f22a8b72208a203374ed784fdf))
* update dependency sass to v1.101.0 ([#224](https://github.com/mbryantms/folio/issues/224)) ([282668f](https://github.com/mbryantms/folio/commit/282668fad2d124924d28d367c0fa2649ed3128e3))
* update rust crate time to v0.3.49 ([#220](https://github.com/mbryantms/folio/issues/220)) ([09b6a7c](https://github.com/mbryantms/folio/commit/09b6a7c9d9b561ba0198f862d76b545980466424))
* update scalar monorepo to v0.9.46 ([#143](https://github.com/mbryantms/folio/issues/143)) ([2e44031](https://github.com/mbryantms/folio/commit/2e44031231f69d87356c0ae514039aab47da0bb4))
* update tailwindcss monorepo to v4.3.1 ([#221](https://github.com/mbryantms/folio/issues/221)) ([932d9cd](https://github.com/mbryantms/folio/commit/932d9cd3d4f07f6fd24121661aad9872d4d0e544))

## [0.13.1](https://github.com/mbryantms/folio/compare/v0.13.0...v0.13.1) (2026-06-14)


### Fixed

* **admin:** hide the users pager on a single page (D9) ([#196](https://github.com/mbryantms/folio/issues/196)) ([35d29d4](https://github.com/mbryantms/folio/commit/35d29d4dde715563f60cf95da80c836c963b3ee2))
* **admin:** keyboard-accessible data-table row expander (E8) ([#195](https://github.com/mbryantms/folio/issues/195)) ([049f368](https://github.com/mbryantms/folio/commit/049f3682c6288ee13f04cb049d6706f4928842b5))
* **bookmarks:** keep search + Select + card-size on one row ([#201](https://github.com/mbryantms/folio/issues/201)) ([e71fa74](https://github.com/mbryantms/folio/commit/e71fa74f35b4a19c36d62023da68f9da827d87c5))
* **mobile:** hide the cover kebab on touch; long-press opens the sheet ([#198](https://github.com/mbryantms/folio/issues/198)) ([dbc7c2f](https://github.com/mbryantms/folio/commit/dbc7c2f5d42d31a5cf75c09010535aba8803fc6b))
* **reader:** mention swipe-to-turn in the first-run overlay (C5) ([#200](https://github.com/mbryantms/folio/issues/200)) ([cea2b7e](https://github.com/mbryantms/folio/commit/cea2b7ee3aaae3ea3ca6f1e20db7074e7640f206))
* **ui:** portal tooltips so they aren't trapped under page content ([#199](https://github.com/mbryantms/folio/issues/199)) ([7b12266](https://github.com/mbryantms/folio/commit/7b12266667c4f06b9d98ffa30cf4622df4d12610))

## [0.13.0](https://github.com/mbryantms/folio/compare/v0.12.0...v0.13.0) (2026-06-14)


### Added

* **reader:** blur-up page placeholder from strip thumbnails (C3) ([#194](https://github.com/mbryantms/folio/issues/194)) ([0b8eb02](https://github.com/mbryantms/folio/commit/0b8eb02ac80b2414e226d4a65aff54be5b7892e6))


### Fixed

* **reader:** strip NUL bytes from MarkerEditor that broke dev highlight ([#190](https://github.com/mbryantms/folio/issues/190)) ([a92eeec](https://github.com/mbryantms/folio/commit/a92eeec5368a841dafae69d274f01b41b917cda1))
* **search:** compact recent-search pills ([#191](https://github.com/mbryantms/folio/issues/191)) ([8ae05db](https://github.com/mbryantms/folio/commit/8ae05dbd8f962a5eeb849ccc2f4243eb5017cc63))


### Changed

* **reader:** lazy-load the OCR/crop path off select-text/image (G6) ([#193](https://github.com/mbryantms/folio/issues/193)) ([4d64a29](https://github.com/mbryantms/folio/commit/4d64a29e4dfa1efcca30220f85c6482657fd66b5))

## [0.12.0](https://github.com/mbryantms/folio/compare/v0.11.0...v0.12.0) (2026-06-13)


### Added

* **bookmarks:** multi-select, flat sort, and total count (B11) ([#173](https://github.com/mbryantms/folio/issues/173)) ([79c0c1c](https://github.com/mbryantms/folio/commit/79c0c1c60c1b69a9b2bc2a4d408a47c49132aa49))
* **library:** grid filter URL state + read-status filter (B1/B2) ([#168](https://github.com/mbryantms/folio/issues/168)) ([38702c2](https://github.com/mbryantms/folio/commit/38702c2f803f498cb3871706a4aa66315061a33e))
* **library:** multi-select on the library grid (B3, E9) ([#172](https://github.com/mbryantms/folio/issues/172)) ([339277e](https://github.com/mbryantms/folio/commit/339277e0bab2b728e5ce6ccc2f6ee28f1ca90ef0))
* **library:** persistent cover kebab + one-time hint on touch (B16) ([#175](https://github.com/mbryantms/folio/issues/175)) ([1da13ac](https://github.com/mbryantms/folio/commit/1da13ac2efaaa00d37a2e7e171ce8ebfa0b3835f))
* **library:** window-virtualize the grid + scroll restore (G1, B15) ([#177](https://github.com/mbryantms/folio/issues/177)) ([59e7671](https://github.com/mbryantms/folio/commit/59e76715962cbfaea9c2c82ab87034751983cb3f))
* **library:** window-virtualize the IssuesPanel main run (G1) ([#178](https://github.com/mbryantms/folio/issues/178)) ([607af59](https://github.com/mbryantms/folio/commit/607af59cf6b1453e18c283d2a579ba5bbfcf2bb3))
* **reader:** keyboard/SR-reachable region markers (E4) ([#188](https://github.com/mbryantms/folio/issues/188)) ([b887417](https://github.com/mbryantms/folio/commit/b8874172cad02e5beb9f189b83898dcf0ed27175))
* **reader:** layer Escape — collapse chrome/strip before exiting (E6) ([#187](https://github.com/mbryantms/folio/issues/187)) ([04335bb](https://github.com/mbryantms/folio/commit/04335bb0e0b56a7f313a990f8421bfd495425b9d))
* **reader:** lazy marker editor + active-mode pill (C7, bundle ratchet) ([#183](https://github.com/mbryantms/folio/issues/183)) ([068573b](https://github.com/mbryantms/folio/commit/068573b5221fc8c1d79658101f2e87bca7ba4502))
* **reader:** marker editor dirty-guard (C7) ([#184](https://github.com/mbryantms/folio/issues/184)) ([f4c1ec8](https://github.com/mbryantms/folio/commit/f4c1ec8388f3854cce6170d264c54700cf1172cb))
* **reader:** one-time first-run orientation overlay (C5) ([#185](https://github.com/mbryantms/folio/issues/185)) ([72f48e5](https://github.com/mbryantms/folio/commit/72f48e51e322f028460ee83730bf86dffe8dab65))
* **reader:** page-load error+retry, page-nav keys, hide iOS fullscreen (C3, C13, C11) ([#182](https://github.com/mbryantms/folio/issues/182)) ([c5c91db](https://github.com/mbryantms/folio/commit/c5c91db0d11d2d50fc59f7c00f7f2523d510da6d))
* **reader:** persist brightness & sepia across reloads (E4) ([#186](https://github.com/mbryantms/folio/issues/186)) ([0a6039e](https://github.com/mbryantms/folio/commit/0a6039ea068857ae513a1a0159d467dab0c27c1b))
* **reader:** transform zoom + drag-to-pan via +/-/0 keybinds (C9, part 1) ([#180](https://github.com/mbryantms/folio/issues/180)) ([d242e79](https://github.com/mbryantms/folio/commit/d242e796e7d66dc583aa2764c35a1792c5ae7b8d))
* **reader:** unify the gesture-claim layer — overflow pan + double-tap zoom (C4, C9) ([#181](https://github.com/mbryantms/folio/issues/181)) ([d365a00](https://github.com/mbryantms/folio/commit/d365a00d154b166b8b0d30d3ebeeb070e81d03c2))
* **reader:** webtoon rescue — windowing, end footer, progress integrity (C1b/C2/C12) ([#179](https://github.com/mbryantms/folio/issues/179)) ([ca5a907](https://github.com/mbryantms/folio/commit/ca5a9070fe2e66a9d8ab2f8413e78330976c94cd))
* **search:** multi-select on series + issue result grids (B3) ([#174](https://github.com/mbryantms/folio/issues/174)) ([4915653](https://github.com/mbryantms/folio/commit/49156539f412d77650a49b29ea86cf92d422d07b))
* **search:** rebuild the ⌘K modal on cmdk (E2) ([#170](https://github.com/mbryantms/folio/issues/170)) ([ac77fd6](https://github.com/mbryantms/folio/commit/ac77fd6f8c898be219c7a5a8f9fb091c968c34cd))
* **search:** retire legacy /?q= SearchView, redirect to /search (E2) ([#171](https://github.com/mbryantms/folio/issues/171)) ([017a662](https://github.com/mbryantms/folio/commit/017a662ac94f8413626e9e33dd4c0b799bdd0978))
* **server:** error-envelope field-level validation details ([#163](https://github.com/mbryantms/folio/issues/163)) ([a5d838c](https://github.com/mbryantms/folio/commit/a5d838c2e51b68d246696706d1961466ba9c4abd))
* **server:** health-issue un-dismiss endpoint ([#161](https://github.com/mbryantms/folio/issues/161)) ([843b200](https://github.com/mbryantms/folio/commit/843b20002835fba6ac0fcdd03fc5b67481df6491))
* **server:** issue_id filter on GET /progress ([#159](https://github.com/mbryantms/folio/issues/159)) ([b118dfe](https://github.com/mbryantms/folio/commit/b118dfee4f51b4ef84a51ccf7518b93f400d347f))
* **server:** markers bulk-delete endpoint ([#160](https://github.com/mbryantms/folio/issues/160)) ([f3d852f](https://github.com/mbryantms/folio/commit/f3d852f9a9a5cb09200558b5c38571fd61898f60))
* **ui:** semantic status tokens + sweep (F1/F2) ([#167](https://github.com/mbryantms/folio/issues/167)) ([08da1ee](https://github.com/mbryantms/folio/commit/08da1ee0620c75f305e2fd1fe0545d591b09222f))
* **web:** enable React Compiler ([#162](https://github.com/mbryantms/folio/issues/162)) ([3d7af8a](https://github.com/mbryantms/folio/commit/3d7af8a9749f69816fae31ed3db82ee872b2b2f6))


### Fixed

* **admin:** health filters, settings reset, cancelled scans, stale copy ([#153](https://github.com/mbryantms/folio/issues/153)) ([066f87e](https://github.com/mbryantms/folio/commit/066f87e42968f7a0d780335cf7e37b22fa27f949))
* **deps:** bump esbuild to 0.28.1 (GHSA-gv7w-rqvm-qjhr) ([#164](https://github.com/mbryantms/folio/issues/164)) ([942bf47](https://github.com/mbryantms/folio/commit/942bf47d94f223b456e4fcd3f628855afbb9cd71))
* **reader:** keyboard tab order, progress integrity, error boundary ([#154](https://github.com/mbryantms/folio/issues/154)) ([1953a8d](https://github.com/mbryantms/folio/commit/1953a8d2bdf78ccd59e43a49e278282d689fef50))
* **server:** nonce next-themes bootstrap instead of CSP hash allowlist ([#158](https://github.com/mbryantms/folio/issues/158)) ([a2325f4](https://github.com/mbryantms/folio/commit/a2325f49ca781938b6a4f8808f59b7c78a83b79d))
* **ui:** Select toggles to Cancel; theme the bookmarks toggles ([#176](https://github.com/mbryantms/folio/issues/176)) ([e376c3f](https://github.com/mbryantms/folio/commit/e376c3f51951bd83176bdce01f9e1ac6b37520c6))
* **web:** a11y + first-run quick wins, dead code ([#157](https://github.com/mbryantms/folio/issues/157)) ([d036714](https://github.com/mbryantms/folio/commit/d0367145f5b421a07c58be404c4a161bc374e087))
* **web:** query retry policy, SSR waterfalls, grid render hygiene ([#155](https://github.com/mbryantms/folio/issues/155)) ([58d55d1](https://github.com/mbryantms/folio/commit/58d55d1584a0b5e66dde43009eba3333d2095b78))

## [0.11.0](https://github.com/mbryantms/folio/compare/v0.10.5...v0.11.0) (2026-06-12)


### Added

* **ocr:** recognition quality pipeline + bubble-aware tap-to-OCR ([#149](https://github.com/mbryantms/folio/issues/149)) ([39201e9](https://github.com/mbryantms/folio/commit/39201e99f1a431d04f6578a90dcb6dc7f2a0c511))
* **reader:** add next-issue detail link to the end-of-issue card ([#150](https://github.com/mbryantms/folio/issues/150)) ([3a1f607](https://github.com/mbryantms/folio/commit/3a1f6078ce304de3a667ec904de916d6a2a22c98))

## [0.10.5](https://github.com/mbryantms/folio/compare/v0.10.4...v0.10.5) (2026-06-12)


### Fixed

* **deps:** update nextjs monorepo to v16.2.9 ([#144](https://github.com/mbryantms/folio/issues/144)) ([1ac14f1](https://github.com/mbryantms/folio/commit/1ac14f1aebf8ebb544e0bd5b8e99bfaaa5a87e98))

## [0.10.4](https://github.com/mbryantms/folio/compare/v0.10.3...v0.10.4) (2026-06-11)


### Fixed

* **rails:** gate On Deck CBL cards on saved-view wrapper + frontier activity ([#137](https://github.com/mbryantms/folio/issues/137)) ([b79c415](https://github.com/mbryantms/folio/commit/b79c4156d403e30553c98948d94179c4806aee51))

## [0.10.3](https://github.com/mbryantms/folio/compare/v0.10.2...v0.10.3) (2026-06-11)

> [!IMPORTANT]
> **Action required if you run Folio behind a reverse proxy.** As of this
> release, `X-Forwarded-For` is honored **only** from hops listed in the new
> `COMIC_TRUSTED_PROXIES` setting (a comma-separated list of CIDRs or bare IPs).
> The default is empty, so until you set it Folio uses the **connecting peer's**
> IP — meaning every request behind nginx / Caddy / Traefik will be attributed
> to the proxy's address. Set `COMIC_TRUSTED_PROXIES` to your proxy's IP/CIDR to
> restore real client IPs for rate-limiting buckets and audit-log entries. Direct
> (non-proxied) deployments need no change. This closes a spoofing vector where a
> client could forge `X-Forwarded-For` to evade per-IP rate limits or poison
> audit IPs (#131).


### Fixed

* **deps:** update dependency react-hook-form to v7.78.0 ([#123](https://github.com/mbryantms/folio/issues/123)) ([5851c3e](https://github.com/mbryantms/folio/commit/5851c3e79821b383608a12f84e4d1635f952957e))
* **jobs:** chunk scan-event writes, self-heal scan-coalescing keys, and surface dead-lettered jobs ([#133](https://github.com/mbryantms/folio/issues/133)) ([59e1ee4](https://github.com/mbryantms/folio/commit/59e1ee4b3582b17f2ceeaa0b4dd73c3eb5d8f917))
* **jobs:** stop scan_series retry churn on recorded failures (OPS-3 tail) ([#135](https://github.com/mbryantms/folio/issues/135)) ([6e49eaf](https://github.com/mbryantms/folio/commit/6e49eaf013c8b81b6273267845a1cc1e7e1439cd))
* **security:** trust X-Forwarded-For only from configured proxies, bound image decode, harden SSRF + archive-write paths ([#131](https://github.com/mbryantms/folio/issues/131)) ([1ab7b70](https://github.com/mbryantms/folio/commit/1ab7b709325c87837507a09dee36d8470adb9ff8))


### Changed

* cache app-password auth, stream page bytes, and honor conditional (304) requests ([#132](https://github.com/mbryantms/folio/issues/132)) ([0f4e772](https://github.com/mbryantms/folio/commit/0f4e772192ceea48c096f8fa72336dca72c02594))
* serve uncompressed pages lock-free and trim read-path overhead ([#136](https://github.com/mbryantms/folio/issues/136)) ([b2166e0](https://github.com/mbryantms/folio/commit/b2166e08bcc8f2d1baecb3021a1f1bba1f3784be))

## [0.10.2](https://github.com/mbryantms/folio/compare/v0.10.1...v0.10.2) (2026-06-09)


### Fixed

* dedup On Deck cards by issue id ([#129](https://github.com/mbryantms/folio/issues/129)) ([d9d8f23](https://github.com/mbryantms/folio/commit/d9d8f2329446f88589a2286938aae73ad9dbe826))

## [0.10.1](https://github.com/mbryantms/folio/compare/v0.10.0...v0.10.1) (2026-06-08)


### Fixed

* **security:** harden auth and unsafe IO ([#114](https://github.com/mbryantms/folio/issues/114)) ([f71ff93](https://github.com/mbryantms/folio/commit/f71ff93f540487f7d2599cbb1d84749009f32d2f))

## [0.10.0](https://github.com/mbryantms/folio/compare/v0.9.5...v0.10.0) (2026-06-08)


### Added

* **auth:** opt-in OIDC auto-link to local accounts by verified email ([#120](https://github.com/mbryantms/folio/issues/120)) ([db90d31](https://github.com/mbryantms/folio/commit/db90d31ad0b8795541cd63d24b8a484ea2f25fa4))


### Fixed

* **deps:** update radix-ui ([#118](https://github.com/mbryantms/folio/issues/118)) ([ea36555](https://github.com/mbryantms/folio/commit/ea36555d6b0a8dc2cd2c209d7fb7753ebd2145a6))
* **deps:** update tanstack to v5.101.0 ([#124](https://github.com/mbryantms/folio/issues/124)) ([a8a2d15](https://github.com/mbryantms/folio/commit/a8a2d1594bc39e2424e14a4145882010da178c49))

## [Unreleased]

### Fixed

- **Security hardening for auth, imports, and operational endpoints.** Password
  reset links are now single-use, first-admin bootstrap is serialized under
  concurrent signups, auth cookies use `__Host-` names with signed OIDC state,
  unsafe secret-file permissions are refused at startup, provider/CBL fetches
  reject internal-network URLs and oversized bodies, and production metrics
  require a bearer token unless explicitly opened.

### Internal

- Release workflow: a new `prepare release` dispatcher can stamp the
  changelog, open and auto-merge the changelog PR, create the release tag, and
  hand off to the image-publishing workflow without the local/manual release
  ritual.
- Docs dependency audit: Docusaurus transitive `uuid` callers are pinned to a
  patched `uuid` release.

## [0.9.5] - 2026-06-07

### Fixed

- **Duplicate provider IDs no longer wedge a scan.** When two files for one
  issue carried the same ComicVine/Metron/GTIN id, the second file&rsquo;s scan
  used to abort on a unique-constraint violation and then retry forever. The
  scanner now skips the already-claimed id and raises a **Duplicate external
  ID** finding under Admin&nbsp;→ Findings instead, so the scan completes. The
  surviving file automatically reclaims the id once a duplicate is removed (no
  more manual database cleanup), and manually adding an id that&rsquo;s already
  assigned now returns a clear conflict error.

## [0.9.4] - 2026-06-06

### Added

- **Bulk-fetch only missing or partial metadata.** A series&rsquo; bulk
  metadata fetch can now be scoped to just the issues whose metadata is
  incomplete (partial or missing) rather than every issue — saving provider
  budget and keeping the Review queue focused. The Series&nbsp;… menu&rsquo;s
  three metadata actions are grouped into one **Fetch metadata** submenu (Match
  this series · All issues · Only missing or partial).

## [0.9.3] - 2026-06-06

### Fixed

- Issue page: the "More in series" strip now stays on one horizontal rail on
  mobile instead of wrapping the previous and up-next cards.

## [0.9.2] - 2026-06-06

### Added

- **Bulk "Fill missing" / "Replace all" in the Review queue.** The metadata
  batch Review tab's _Needs review_ section gains one-click bulk actions that
  auto-apply the most-complete metadata merged across every provider that
  matched (covers prefer ComicVine), with an All / Selected scope — clearing
  the review queue without opening each item one at a time. Your pinned fields
  are preserved.

### Changed

- **Issue and series detail pages are easier to scan.** Details tabs now use
  card-based summary sections, avoid reserving empty space from large tabs, hide
  redundant provider web links, and keep empty series genre/tag fields out of
  the page header.

### Fixed

- Metadata matching: zero-padded issue numbers (e.g. `014`) now match a
  provider's unpadded number, and series search no longer hard-filters on a
  too-strict start year — fixing spurious "no matches" on issues that clearly
  exist at the provider.

### Internal

- CI/release: docs-only PRs skip redundant build work, and the release-tagging
  workflow trims steps that re-ran needlessly.

## [0.9.1] - 2026-06-06

### Added

- **Previous-issue cover on the issue page.** The in-series rail now shows the
  preceding issue's cover to the left of the next-issue strip (omitted on the
  first issue of a series); the section is retitled "More in series."
- **Clear detected (OCR) text on a marker.** The marker editor gains a "Clear"
  control to drop a highlighted region's OCR'd text.

### Changed

- **Issue & series tabs reorganized.** Both pages now lead with the same tab
  order (Credits · Cast & Setting · Details · …), and the standalone
  "Genres & Tags" tab folds into Details. Tab contents are regrouped into
  scannable categories that use the full width — full-width credit/cast rows,
  the Details fields split into Publication / Format / Library sections, and
  the issue Metadata tab's status moved into a card row.

### Fixed

- Activity tab: the ranking-dimension selector no longer overflows off the
  screen on mobile (it scrolls within the control instead).
- Home rails: an in-progress issue shown in **Continue Reading** no longer also
  appears in **On Deck**.

### Internal

- Clearing a marker's `selection` / `body` / `region` / `color` by sending
  `null` now works (a `double_option` deserialize fix; previously a silent
  no-op).
- New `GET /series/{slug}/issues/{slug}/prev` endpoint backing the
  previous-issue cover.
- Tooling/CI: a pre-commit `cargo fmt` guard (`.githooks` + `just bootstrap`);
  CI runs on `merge_group`; Renovate uses GitHub-native auto-merge
  (`platformAutomerge`); the release ritual is adapted to a protected `main`
  (changelog lands via PR, only the tag is pushed). Plus a developer-workflow
  cheat sheet under `docs/dev/`.

## [0.9.0] - 2026-06-05

### Added

- **Bulk-metadata Review queue.** A bulk fetch ("fetch all issues in a
  series", a saved view, a library refresh) now groups its per-issue/series
  runs into a single batch with live aggregate progress and one consolidated
  accept surface in `/admin/metadata` → **Review**: one-click "Accept all
  strong", per-item review that reuses the candidates already pulled by the
  batch (no re-search), and a fresh search only on no-match.
- **Metadata completeness.** Issues and series are scored against a
  provider-complete baseline (matched + cover date + summary + page count +
  a credit + cover; title/characters/arcs/genres surfaced as gaps but
  non-gating). The tier drives a card/list badge, a new series **Collection**
  tab (ownership gaps + per-issue completeness coloring), and a saved-view
  filter so you can build a "needs metadata" view.
- **Issue Metadata tab.** A per-issue overview of provenance (field → source
  → when), which sidecar files Folio has (ComicInfo / MetronInfo /
  series.json), and freshness (last synced / last rewritten).
- **Auto-resume for quota-parked fetches.** Runs parked at `awaiting_quota`
  when every provider is out of budget now resume on their own once the
  window passes, reusing the stored entity + batch so a large bulk fetch
  finishes without a re-trigger.

### Fixed

- Sign-in: auth tabs stay full-width on tablet/desktop.

### Internal

- New migrations: `metadata_batch` + `metadata_run.batch_id`; sidecar-presence
  columns (`issues.metroninfo_present`, `series.series_json_present`, both
  nullable so `NULL` reads as "unknown until next rescan", distinct from a
  definite absent).

## [0.8.1] - 2026-06-04

### Added

- **Observability: two non-overlapping admin streams.** The old unified
  `/admin/activity` feed is split into a **Server stream** (app-runtime logs +
  audit + user activity) and a **Library stream** (a durable, itemized record
  of scans, thumbnails, covers, metadata, and archive rewrites).
  - **Library activity** (`/admin/findings`): a durable `library_events`
    manifest of every change — issue/series add·update·remove·restore,
    thumbnail / metadata / archive ops — with expandable rows showing target,
    error, series, and on-disk path, alongside the health-issue and scan-run
    tabs.
  - **Scan dashboard** (`/admin/scan-dashboard`): live aggregate progress
    across a "Scan all" run — per-library status, overall completion, and a
    post-run summary of what changed — backed by a new `scan_batch` grouping.
  - **Server log** (`/admin/logs`): a Server/Library stream toggle and an
    error-code facet (every API error is captured with its `error_code`).
  - New read endpoints: `GET /admin/scan-batches[/{id}]` and
    `GET /admin/library-events`. See `docs/dev/observability.md`.

### Changed

- `/admin/activity` ("Server activity") is now audit + reading volume only;
  scan and health moved to the Library stream so the two never overlap. Nav:
  "Logs" → "Server log", "Activity" → "Server activity", plus new "Scan
  dashboard" and "Library activity" entries.

### Internal

- New migrations `library_events` + `scan_batch`; a daily retention prune
  bounds the event manifest (90 days / 50k rows per library).

## [0.8.0] - 2026-06-04

### Added

- **Phase 1 UX + architecture improvements** (#90). System theme option with
  SSR-safe hydration; backend bulk-selection operations ("all matching") plus
  an explicit "Select loaded" action; search category totals + cursor
  pagination with page-region thumbnails on marker/bookmark result cards;
  admin findings / health-issues / scan-runs tables moved to infinite-query
  pagination; `/me/account` now surfaces `email_editable` / `password_editable`
  so it only offers edits the active auth mode supports. See
  `docs/dev/ux-architecture-improvement-plan.md`.

### Changed

- **Dependency catch-up (round 3 + round 4).** Rust toolchain 1.91.1 → 1.96.0
  (+ constant_time_eq 0.5); postgres/redis completion, imageproc 0.27,
  axum-extra 0.12; web in-range bumps (next / react / react-dom 16.2.7 /
  19.2.7), openapi-typescript 7.4 → 7.13, blake3 1.8.5.
- **pnpm 10.33.2 → 11.5.1.** Security `overrides` moved to `pnpm-workspace.yaml`
  (pnpm 11 no longer reads the `pnpm` field in package.json); skipped native
  build scripts recorded via `allowBuilds`. pnpm 11 also enforces a default
  24h `minimumReleaseAge` supply-chain gate.
- **Lock-file maintenance** (#82): `@playwright/test` 1.59.1 → 1.60.0 plus
  transitive/dev-tooling refreshes (docs-site `@swc/core`,
  `@algolia/client-search`, webpack, react 19.2.7 propagation).

### Fixed

- **CI OpenAPI-drift job** had failed on every branch since the workflow
  regressed: it exec'd `openapi-typescript` from the repo root (where the dep
  doesn't exist) under suppressed stderr, and the downstream `oasdiff` step
  invoked `./oasdiff` instead of the on-PATH binary. Both fixed (#89).
- Offline toast no longer claims changes will be queued; transient failures
  keep an explicit retry path (#90).
- Series "Read from beginning" routes via the slug-based reader URL helper
  instead of the stale `/read/{issueId}` path (#90).

### Internal

- Branch-protection required checks updated to the Docker matrix job names so
  PRs are mergeable without an admin override.

## [0.7.23] - 2026-06-02

### Changed

- **Dependency catch-up (round 2).** Major/migration bumps across the stack:
  postgres 17 → 18 and redis 7 → 8 (dev + test containers), apalis 0.6 → 0.7
  (+ redis crate 0.27 → 0.32), fast_image_resize 5 → 6, and out-of-range rust
  0.x crates (imageproc 0.26, testcontainers 0.27, metrics-exporter-prometheus
  0.18, tokio-cron-scheduler 0.15). CI runner actions bumped to current majors
  (checkout v6, setup-node v6, docker/\* v7/v6/v4, cosign v4) and the
  `docker/dockerfile` syntax + dev `dex` image tags refreshed.

  **Operator note:** an existing dev `.dev-data/postgres` directory is
  PG17-initialized and will not start under PG18 — run `just dev-services-reset`
  (wipes the local dev DB) when adopting. Fresh installs and CI are unaffected.

### Removed

- Dropped the unused `notify` + `notify-debouncer-full` dependencies (declared
  but referenced nowhere).

### Internal

- Renovate tuned: `rangeStrategy` → `update-lockfile` (stops cosmetic
  manifest-floor churn), coordinated groups for cross-pinned crate sets, and
  `yaml` pinned to 1.x (override-only security pin for the docs toolchain).

## [0.7.22] - 2026-06-02

### Changed

- **Dependency refresh.** In-range lockfile updates across both stacks —
  Rust (`cargo update`, 36 crates incl. hyper 1.10, serde_json 1.0.150,
  opentelemetry_sdk 0.32.1) and web (`pnpm update`) — plus behind-by-minors
  bumps for `garde` (0.23), `lru` (0.18), and `infer` (0.19). No runtime
  behavior changes; all gates green.
- **Renovate coordinated groups.** `renovate.json` now groups the
  cross-pinned crate sets that previously surfaced as conflicting standalone
  bumps: `sea-orm + sqlx`, `apalis + redis`, and the RustCrypto
  digest/rand ecosystem (`sha2`/`hmac`/`rand`/`argon2`/`rsa`/…).

## [0.7.21] - 2026-06-02

### Fixed

- **Dead clicks after a dialog or menu closes.** Radix overlays set
  `pointer-events: none` on `<body>` while open; if the close raced a
  navigation, the lock could stick and silently kill every click on the
  page ("no actions taken"). The reset now runs on every route change
  (forward and back) instead of only when the shell first mounts, so any
  navigation un-sticks it.
- **Stalled page transitions now recover on their own.** A new loading
  watchdog mounts inside the library `loading.tsx`: if a route's content
  stays pending past ~15s (a proxy/upstream or RSC-stream stall the App
  Router can't otherwise escape), it hard-reloads the destination URL,
  with a per-URL guard so it never loops. No more spinning on the loading
  skeleton until a manual force-quit.

## [0.7.20] - 2026-06-01

### Fixed

- **iOS Safari / installed-PWA navigation hang.** The first client-side
  navigation after a fresh page load (e.g. tapping a creator pill) could
  hang on the loading skeleton, after which every link went dead until a
  reload. Root cause: the service worker's `clientsClaim` seized a page
  that had loaded _without_ the worker, and on WebKit the first RSC
  navigation through that mid-session-claimed worker never resolved. The
  worker no longer claims already-open pages, disables navigation preload,
  and hands **all** navigation/RSC requests straight to the browser — so it
  can never stall a route transition. (Supersedes the per-route allowlist
  from v0.7.19. As before, fully close/reopen the PWA — or reload the tab —
  once after upgrading to pick up the new worker.)
- **Pills now land at the top of the destination page.** Tapping a credit
  chip (→ creator page) or a cast/setting chip (→ filtered library grid)
  from a scrolled-down page could open the new page scrolled down with its
  header clipped off the top. Forward navigations within the library now
  reliably scroll to the top; back/forward still restore the previous
  scroll position.

## [0.7.19] - 2026-06-01

### Added

- **"Back to this issue" on the end-of-issue card.** The reader's up-next
  card now offers a direct link back to the current issue's detail page
  alongside the "Read" button, so you can leave to the issue you just
  finished without first advancing to the next one.

### Fixed

- **The installed PWA can now open creator pages (and other detail
  pages).** Tapping a writer/penciller credit links to `/creators/<slug>`,
  but that route — along with `/read/`, `/settings/`, `/bookmarks`, and
  `/pages/` — was missing from the service worker's native-loader bypass
  list. In the installed app the client-side navigation fell through to the
  worker's cache and hung; in a normal browser tab it worked. All
  client-navigable detail routes are now handed straight to the browser
  loader like `/series/` already was. (Takes effect once the updated
  service worker activates — fully close and reopen the PWA after upgrade.)
- **Full-width reader pages now start at the top after every page turn.**
  Tapping or swiping to a page whose image hadn't been prefetched could
  leave the viewport parked partway down the new page; scroll anchoring is
  now disabled on the reader and the top position is re-asserted once the
  page decodes.
- **Webtoon page jumps no longer flicker through intermediate pages.** A
  programmatic jump (page strip, keyboard, resume) is no longer dragged to a
  page it scrolled past mid-animation.

### Changed

- **Above-the-fold rail covers load eagerly (LCP).** The first row of cover
  images on the home rails is fetched with high priority instead of lazily,
  improving the largest-contentful-paint on the landing surface.

## [0.7.18] - 2026-06-01

### Added

- **Expanded Prometheus metrics at `/metrics`.** Added the service-level signals
  that were missing: HTTP request rate/latency/errors (`folio_http_requests_total`,
  `folio_http_request_duration_seconds`), process/runtime gauges
  (`folio_process_*` — CPU, RSS, file descriptors, threads), per-job outcomes +
  duration (`folio_jobs_processed_total`, `folio_job_duration_seconds`), and
  job-queue backlog (`folio_jobs_queue_depth`). The endpoint is unauthenticated
  by default; set the new **`COMIC_METRICS_TOKEN`** to require an
  `Authorization: Bearer` header on scrapes. Full catalogue + scrape config in
  [docs/dev/metrics.md](docs/dev/metrics.md).
- **Automated dependency monitoring.** Renovate (`renovate.json`) opens grouped
  update PRs and auto-merges safe patch/minor after CI; the weekly security
  workflow gains an OSV-Scanner sweep over both lockfiles.

### Changed

- **Node runtime upgraded 22 → 24 (Active LTS).** The web build + runtime images
  move to `node:24` / `distroless/nodejs24-debian12`; `@types/node` tracks 24.
- **The server now reports its real version and name.** The startup log
  (`Folio starting`), `/healthz`, `/readyz`, `/admin/server`, and every outbound
  HTTP `User-Agent` now show the build tag (e.g. `v0.7.18`) instead of the
  `0.0.0` / `comic-reader` placeholders.
- **Frontend dependency refresh.** All npm advisories resolved; TanStack Query
  5.100, react-hook-form 7.77 + resolvers 5.4, plus a sweep of safe Radix/UI
  bumps.
- **⚠️ Prometheus metric names renamed `comic_*` → `folio_*`** (every metric).
  **Update any Grafana dashboards or alert rules** that reference the old names.
- **JWT audience renamed `comic-reader` → `folio`.** Verification still accepts
  the legacy audience during the transition window, so existing sessions are
  **not** forced to re-authenticate on upgrade.

### Removed

- The dead, never-wired `openapi-fetch` client and the inert
  `COMIC_OTLP_ENDPOINT` env var (OTLP export was considered and dropped for v1;
  see incompleteness-audit §D-9).

### Fixed

- **The UI no longer locks up after saving an archive edit.** Removing a page
  (or any page-editor save) closed the confirm dialog and the editor in the same
  tick as the background `router.refresh()`. Radix sets `pointer-events: none` on
  `<body>` while a dialog is open and restores it on close; closing two nested
  dialogs while a soft RSC refresh ran raced that restore, and since the refresh
  doesn't remount the app shell (whose mount effect clears the lock), the whole
  page stayed unclickable until a hard refresh. The save now defers the refresh
  past the dialog close and clears any residual body lock itself.

### Security

- Resolved all outstanding npm advisories: a build-time PostCSS XSS in the web
  app, plus three High + several moderate transitive advisories in the docs-site
  build tooling (lodash, serialize-javascript, js-yaml, yaml) via root
  `pnpm.overrides`. One dev-server-only, non-exploitable advisory (sockjs → uuid)
  is documented as an accepted exception in `SECURITY-EXCEPTIONS.md`. None of
  these were reachable in the shipped server or web runtime.

## [0.7.17] - 2026-05-30

### Changed

- **"Generate page thumbnails" now queues only the issues that actually need
  them.** It previously enqueued one strip job per _active_ issue regardless of
  whether the page thumbnails already existed — so on a near-complete library it
  flooded the queue with tens of thousands of redundant jobs (the worker skipped
  each one after a disk check, but the queue depth was meaningless and took
  hours to drain). The enqueue path now does that same disk check up front and
  pushes jobs only for issues whose strips are missing or incomplete; issues
  with an unknown page count still enqueue so the worker can reconcile from the
  archive. The log line reports how many already-complete issues were skipped.

## [0.7.16] - 2026-05-30

### Added

- **The scanner now ingests CBR comics.** Previously a `.cbr` was recognized
  but skipped with an `UnsupportedArchiveFormat` health issue. A new per-library
  setting, **Convert CBR to CBZ on scan** (under Archive writeback, requires the
  master writeback toggle), makes the scanner convert each `.cbr` into a sibling
  `.cbz` in place and ingest it. Real RAR archives are decompressed and repacked
  (the original is kept as `.cbr.bak`); the conversion reuses the same audited,
  atomic archive-rewrite machinery as the page editor.
- The converter **sniffs the real container by magic bytes** rather than
  trusting the extension — a large share of `.cbr` files in the wild are
  actually ZIPs that were renamed. Those are moved into place byte-for-byte
  (an instant rename, no decompression); only genuine RAR archives take the
  decompress-and-repack path. A file that is neither is left skipped with the
  health issue.

## [0.7.15] - 2026-05-30

### Fixed

- **Navigations no longer spin forever.** The server-side API fetches that RSC
  pages depend on had no timeout, so a single hung or slow backend request
  stalled the whole render — leaving client navigations (notably exiting the
  reader and applying an archive edit, both of which land on the issue page)
  stuck on a loader until a force-quit. Server fetches now time out at 10s and
  fail into the route's error boundary, and a client-side watchdog hard-reloads
  a route whose loading state outlives ~15s — covering proxy/stream stalls the
  fetch timeout can't catch. This is the deeper layer beneath the v0.7.10
  service-worker fix.
- **The archive editor no longer shows a phantom trailing page.** It built its
  tiles from the database's `issue.page_count`, which can drift from the actual
  archive (a stale scan, or a ComicInfo `<PageCount>`), producing a blank extra
  page whose deletion errored with "ordinal out of range." The editor now reads
  the archive's real page count live (new `GET /issues/{id}/archive/page-count`)
  and builds from that, so it always matches the file.

## [0.7.14] - 2026-05-29

### Fixed

- **Home no longer inherits the previous view's scroll position.** Home, the
  library grid (`?library=…`), and search (`?q=…`) all share the `/` route, and
  the App Router only resets scroll on a pathname change — so opening Home from
  the grid or search (same path, scrolled down) left it scrolled mid-page. Home
  now resets to the top when it loads from those views. Filtering within the
  grid still preserves scroll, and other pages are unaffected.

## [0.7.13] - 2026-05-29

### Changed

- **Reverted the compact single-row mobile list headers** (Bookmarks, All
  Libraries, CBL list) introduced in v0.7.7. Those headers now stack their
  control rows again as they did before, on mobile and desktop. This also
  removes the `PageHeader` `descriptionClassName` prop and the Libraries
  toolbar's `⋯` overflow that folded Save-as-view / Clear-filters.

## [0.7.12] - 2026-05-29

### Added

- **Bulk archive editing.** The multi-select toolbar on the series, collection,
  and reading-list views gains an admin-only **Edit archives…** action that
  applies one operation across the whole selection — rotate cover, rotate every
  page, or remove the first/last N pages. Each op is _relative_, lowered per
  issue once its page count is known (so "remove the last page" does the right
  thing on every archive, and removal never empties a file). The server skips
  issues whose library has writeback disabled or whose format isn't editable and
  reports them back, so nothing is silently dropped.
- **Admin Queue page** (`/admin/queue`): a live pending-job depth overview
  across all queues (now including archive edits) plus an **Archive operations**
  tab listing recent edits from the audit trail with per-row drill-down.
- **Archive backups storage card** on the library health page — total size,
  file count, and oldest/newest of the `.bak` safety backups the editor keeps,
  so operators can gauge disk use.

### Fixed

- **Highlight thumbnails no longer squish on non-2:3 pages.** A saved highlight
  on a double-page spread (or any page that isn't ~2:3) rendered horizontally
  compressed in the markers grid, because the tile assumed every page is 2:3.
  New markers now stamp the page's natural dimensions at capture time and the
  grid renders them at their true aspect — with no layout reflow. (Markers saved
  before this update keep the old approximation until re-created.)

### Changed

- **Covers now open in an in-app lightbox instead of a new browser tab.** A
  cover tile in the issue's Covers tab was a `target="_blank"` link to the
  raw image bytes — fine in a browser, but an installed PWA has no new tab to
  open, so it navigated the app itself onto the chromeless image endpoint with
  no way back. Tiles now open a full-resolution viewer inside the app: page
  between covers (arrows or ←/→), tap the backdrop or press Esc to close back
  to the gallery. Controls are inset from the device safe areas so they clear
  the iOS status bar and home indicator.

## [0.7.10] - 2026-05-29

### Fixed

- **Exiting the reader no longer hangs on a spinner.** The exit arrow does a
  client-side navigation to the issue page; that request shares a path prefix
  (`/series/…`) with the API hard-guard in the service worker, which re-fetched
  it via `respondWith(fetch(request))` — forwarding the request's abort signal.
  When the App Router superseded the in-flight RSC fetch (intermittently, under
  the reader's decode load), the forwarded signal aborted the worker's fetch and
  the router stranded on the route's loading state until a hard reload. The
  worker now hands these requests to the browser's native loader (matching the
  cross-origin branch), signal intact — no re-fetch, no stranded navigation.
  Existing PWA clients pick up the fix once the new service worker activates
  (close all tabs, or accept the update prompt).
- **iOS PWA: the status bar no longer overlaps the comic art.** With the
  translucent status bar the reader painted full-screen, so the clock / battery /
  home indicator sat on top of the page. The reader now insets its image by the
  device safe areas, so the status bar and home indicator land on the black
  letterbox instead of the art. (iOS can't hide the status bar from a PWA; this
  keeps it clear of the page. No-op off-iOS, where the insets are zero.)

## [0.7.9] - 2026-05-29

### Fixed

- **Covers no longer flash white and paint in top-to-bottom as a page loads.**
  Library/series/issue pages render covers client-side, and the `Cover`
  component had no placeholder or fade — each cover painted onto the page as
  it loaded, cascading down the grid. Covers now sit on a stable dark tile
  and fade in once decoded (cached covers paint instantly, no fade), matching
  the reader's page-image behavior.
- **Library grid loading skeleton** is now a neutral cover-card grid instead
  of a rails shape, so it no longer mismatches the `?library=` grid view
  while loading.

## [0.7.8] - 2026-05-29

### Changed

- **Seamless reader page turning.** Page prefetch now decodes and retains the
  upcoming/previous pages (`img.decode()` + retained element) instead of only
  warming the byte cache, so the next/prev `<img>` mounts already-decoded and
  the flip is instant — no re-decode, no entrance fade. Prefetch now covers
  both directions (3 ahead / 2 behind), dedupes, caps concurrency, and works
  in webtoon mode; the visible page loads at `fetchPriority="high"`.
- **Smoother page map.** Strip thumbnails are pre-warmed around the current
  page when the reader opens (filling the cache and kicking server-side
  generation early) and load eagerly within the visible window, so the strip
  no longer flashes blank placeholders as it slides up.
- **Snappier page transitions.** Slide trimmed 280→210ms, fade 220→160ms.

## [0.7.7] - 2026-05-29

### Changed

- **More compact list headers on mobile.** The Bookmarks, All Libraries, and
  CBL-list headers stacked many full-width control rows, pushing content far
  down on phones. Now: search grows to fill one row with the density/view
  toggle (Bookmarks) or trailing controls (Libraries) beside it; the Libraries
  toolbar's secondary actions (Save as view, Clear filters) fold into a `⋯`
  overflow; the Bookmarks reference blurb is hidden on small screens; and the
  CBL search grows on mobile. (CBL's stats-pills/controls restructure is a
  follow-up.)

## [0.7.6] - 2026-05-29

### Fixed

- **Metadata apply now refreshes open tabs without a page reload.** Applying
  is async; the match dialog only re-hydrated on the writeback path (waiting
  for the rescan's `scan.completed`). A DB-direct (non-writeback) apply had no
  completion signal, so an already-open **Covers** or **Notes** tab stayed
  stale until a manual refresh. The apply job now broadcasts a
  `metadata.applied` event the dialog waits on, so both paths re-hydrate.
- **Action-menu "Thumbnails" item no longer highlights differently** from its
  siblings. The dropdown sub-trigger now flips text to `accent-foreground`
  (and animates) on hover/focus/open like a regular menu item, instead of
  showing the accent background with default-colour text.
- **Dropdown menus now scroll instead of overflowing the screen.** A long
  action menu opened mid-page on mobile ran items off-screen (up or down)
  with no way to reach them. Menu (and submenu) content is now capped to the
  available viewport height and scrolls.

## [0.7.5] - 2026-05-29

### Fixed

- **`GET /libraries/{id}` 404'd when called with a UUID.** The endpoint
  resolved only by slug, but the fetch-metadata dialog holds the issue's
  `library_id` UUID — so the lookup missed, the library never loaded, and
  `metadata_writeback_enabled` read as false. That silently broke the
  apply→wait-for-rescan flow (the dialog closed onto a stale issue page).
  The read endpoint now accepts a slug **or** a UUID.

## [0.7.4] - 2026-05-29

### Fixed

- **Candidate cover images failed to load in the fetch-metadata view.** The
  service worker's cross-origin guard was a no-op (serwist's `defaultCache`
  registered a second fetch listener that still intercepted provider covers);
  the resulting opaque cross-origin response is incompatible with the
  document's `COEP: credentialless`, so the browser blocked the images
  (`NS_ERROR_INTERCEPTION_FAILED`). The SW now hands cross-origin requests to
  the browser's native loader. Existing clients pick up the fix on the next
  service-worker update (hard refresh / close all tabs).

## [0.7.3] - 2026-05-29

### Added

- **"Re-download missing variant covers" button** in the admin Metadata
  dashboard. Triggers the variant-cover backfill (previously API-only) to
  recover provider covers whose local file is missing, looping in batches
  and reporting any that can't be refetched (stale provider URL).

### Changed

- **Error and 404 pages rebuilt** to be theme-aware and on-brand, replacing
  the legacy top-bar shell. A shared `StatusScreen`/`StatusCard` now backs the
  404, the per-area error boundaries, a new root-level not-found, and a new
  `global-error` boundary that catches root-layout crashes.

### Fixed

- **Page title wrapped despite available space.** The page header now extends
  on one line (ellipsizing only when genuinely out of room), matching the
  reading-list header instead of breaking onto two lines.
- **Renaming a page left a dead sidebar link.** The left nav is rendered in the
  server layout, which soft navigation preserved — so its link kept pointing at
  the old slug and 404'd. Renames now refresh the layout so the link updates.

## [0.7.2] - 2026-05-29

### Added

- **Page-editor image adjustments.** The archive page editor can now apply
  non-destructive image transforms per page — brightness/contrast, levels
  clip, sharpen (unsharp mask), despeckle (median filter), and crop — with a
  live canvas preview and a draggable crop box. Transforms are applied at
  archive-rewrite time across CBZ/CBT/CBR, after rotation and before
  re-encode; pages needing no encode still stream-copy verbatim. Frontend and
  backend share an identical transform chain for preview/output parity.
- **Loading-skeleton framework, rebuilt per surface.** Each area now renders a
  shape-matched skeleton inside its real shell instead of one generic cover
  grid in the legacy auth shell: home rails, series detail (hero + stats +
  tabs + issue grid), bookmarks, collections, admin (header + tabs/table),
  and settings (form cards). The top-level fallback is now shell-agnostic.

### Fixed

- **Reader loading flash on iPad.** The reader inherited the library's
  light/cover-grid loading fallback, flashing white before the dark reader
  painted. It now has its own dark, reader-shaped skeleton driven by a shared
  `--reader-bg` token, so the background can't drift between skeleton and
  reader. The reader's server-side prefetches (`/progress`, `/auth/me`) now
  run concurrently, shortening time-to-reader.
- **Variant covers wiped by the nightly orphan sweep.** Downloaded provider
  covers live under `thumbs/issues/…`; the thumbnail orphan sweep read
  `issues` as an issue id and `remove_dir_all`'d the whole tree every night,
  leaving "cover unavailable" 404s and gray gallery boxes. The sweep now skips
  the reserved tree and reclaims only covers of genuinely inactive issues; the
  variant-cover backfill re-downloads rows whose file went missing.
- **Page rename navigated to a 404.** Renaming a custom page reallocates its
  slug, but the post-rename refresh re-rendered the stale `/pages/<old-slug>`
  URL and hit `notFound()`. The rename now navigates to the new slug when on
  the page's detail route. Long page titles also wrap instead of truncating.

### Removed

- Dropped the vestigial `metadata_run_candidate.dismissed_at` column.

[Unreleased]: https://github.com/mbryantms/folio/compare/v0.9.5...HEAD
[0.9.5]: https://github.com/mbryantms/folio/compare/v0.9.4...v0.9.5
[0.9.4]: https://github.com/mbryantms/folio/compare/v0.9.3...v0.9.4
[0.9.3]: https://github.com/mbryantms/folio/compare/v0.9.2...v0.9.3
[0.9.2]: https://github.com/mbryantms/folio/compare/v0.9.1...v0.9.2
[0.9.1]: https://github.com/mbryantms/folio/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/mbryantms/folio/compare/v0.8.1...v0.9.0
[0.8.1]: https://github.com/mbryantms/folio/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/mbryantms/folio/compare/v0.7.23...v0.8.0
[0.7.21]: https://github.com/mbryantms/folio/compare/v0.7.20...v0.7.21
[0.7.20]: https://github.com/mbryantms/folio/compare/v0.7.19...v0.7.20
[0.7.19]: https://github.com/mbryantms/folio/compare/v0.7.18...v0.7.19
[0.7.18]: https://github.com/mbryantms/folio/compare/v0.7.17...v0.7.18
[0.7.15]: https://github.com/mbryantms/folio/compare/v0.7.14...v0.7.15
[0.7.14]: https://github.com/mbryantms/folio/compare/v0.7.13...v0.7.14
[0.7.13]: https://github.com/mbryantms/folio/compare/v0.7.12...v0.7.13
[0.7.12]: https://github.com/mbryantms/folio/compare/v0.7.11...v0.7.12
[0.7.11]: https://github.com/mbryantms/folio/compare/v0.7.10...v0.7.11
[0.7.10]: https://github.com/mbryantms/folio/compare/v0.7.9...v0.7.10
[0.7.9]: https://github.com/mbryantms/folio/compare/v0.7.8...v0.7.9
[0.7.8]: https://github.com/mbryantms/folio/compare/v0.7.7...v0.7.8
[0.7.7]: https://github.com/mbryantms/folio/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/mbryantms/folio/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/mbryantms/folio/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/mbryantms/folio/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/mbryantms/folio/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/mbryantms/folio/compare/v0.7.1...v0.7.2
