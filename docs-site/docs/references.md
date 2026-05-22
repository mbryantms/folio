---
sidebar_position: 10
title: Other references (in repo)
---

# Other references (in repo)

These pages live in the repo at [`docs/dev/`](https://github.com/mbryantms/folio/tree/main/docs/dev)
but haven't been promoted into this site. Some are point-in-time
audits dated to a specific cutoff and best read alongside the commit
history that followed; others are deep operator-only references that
move too fast to re-verify on every site rebuild.

## Snapshot audits

Dated findings against the codebase at a specific point in time.
Useful for understanding "why is this debt here?" — verify any
specific finding against current code before acting on it.

- [phase-status.md](https://github.com/mbryantms/folio/blob/main/docs/dev/phase-status.md)
  — milestone snapshot through the v0.2 rust-public-origin cutover.
  Significant work has shipped since (OPDS readiness 1.0, runtime
  config 1.0, multi-select, scanner nested folders, manga + bulk
  metadata, recovery visibility, …); consult `git log` for the
  full picture.
- [code-quality-audit.md](https://github.com/mbryantms/folio/blob/main/docs/dev/code-quality-audit.md)
  — tech-debt snapshot (duplicate `fn error()` helpers, oversized
  handlers, panic surface, concurrency hot spots).
- [incompleteness-audit.md](https://github.com/mbryantms/folio/blob/main/docs/dev/incompleteness-audit.md)
  — unfinished features and stale comments through 2026-05-15.
- [keyboard-shortcuts-audit.md](https://github.com/mbryantms/folio/blob/main/docs/dev/keyboard-shortcuts-audit.md)
  — reader + global keybind inventory and discoverability gaps.
- [notifications-audit.md](https://github.com/mbryantms/folio/blob/main/docs/dev/notifications-audit.md)
  — toast/sonner consistency findings.
- [opds-audit.md](https://github.com/mbryantms/folio/blob/main/docs/dev/opds-audit.md)
  — OPDS readiness check (largely closed by the OPDS Readiness 1.0
  plan; kept for the gap matrix).
- [scanner-perf.md](https://github.com/mbryantms/folio/blob/main/docs/dev/scanner-perf.md)
  — scanner performance baseline against a 1395-CBZ / 67 GB library.
- [security-audit.md](https://github.com/mbryantms/folio/blob/main/docs/dev/security-audit.md)
  — security audit; H-1 SSRF closed pre-v1, M-1 downgraded after
  rust-public-origin cutover.

## Operator-facing live references

These describe live network protocols and per-client compatibility
quirks. They move with each client release; consult them alongside
the source in the repo rather than this site for the freshest copy.

- [opds.md](https://github.com/mbryantms/folio/blob/main/docs/dev/opds.md)
  — OPDS catalog reference: endpoints, auth, metadata, per-client
  workarounds (Panels, KOReader, Komga compat).
- [opds-progress-protocol.md](https://github.com/mbryantms/folio/blob/main/docs/dev/opds-progress-protocol.md)
  — wire format for `PUT /opds/v1/issues/{id}/progress`, including
  conflict resolution and the KOReader Sync.app shim.
- [reader-shortcuts.md](https://github.com/mbryantms/folio/blob/main/docs/dev/reader-shortcuts.md)
  — older reader keymap reference; partially superseded by the
  in-app help sheet (`?` in the reader). The keybind registry in
  `web/lib/keybinds/` is the source of truth.
- [library-scanner-spec.md](https://github.com/mbryantms/folio/blob/main/docs/dev/library-scanner-spec.md)
  — original v0.1 scanner spec; superseded by the promoted
  [Library scanner reference](./architecture/library-scanner).
