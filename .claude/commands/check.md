---
description: Run the full Rust + web check suite (check, clippy, test, typecheck, lint) and report failures.
argument-hint: "[--skip-test|--rust-only|--web-only]"
allowed-tools: Bash(cargo check:*), Bash(cargo clippy:*), Bash(cargo test:*), Bash(pnpm:*), Bash(pnpm --filter web run:*)
---

Run the project's check suite and report results compactly.

Default: run all of these in order; stop the section on first hard failure
(compile error or test failure) but always finish the other side.

Arg `$ARGUMENTS` may include `--skip-test`, `--rust-only`, `--web-only`. Honor
those filters; otherwise run everything.

**Rust side** (from repo root):
1. `cargo check --workspace --all-targets`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace` *(unless `--skip-test`)*

**Web side**:
1. `pnpm --filter web run typecheck`
2. `pnpm --filter web run lint`
3. `pnpm --filter web run test` *(unless `--skip-test`)*

Run long commands in the background and stream results. Don't run `cargo build`
or `pnpm build` — those are slower and not part of the contract.

**Report format** (terse — under 30 lines total):

```
Rust
  check     ✓ / ✗ N errors
  clippy    ✓ / ✗ N warnings (treated as errors)
  test      ✓ N passed / ✗ N failed
Web
  typecheck ✓ / ✗ N errors
  lint      ✓ / ✗ N errors (warnings noted but not failed)
  test      ✓ N passed / ✗ N failed
```

For any ✗, include the file:line and one-line failure summary inline.
Don't dump full compiler output unless the failure is non-obvious. If
everything passes, just say "All green (Rust: check/clippy/test · Web:
typecheck/lint/test)."
