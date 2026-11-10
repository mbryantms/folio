// This is a CLI binary; printing to stdout/stderr is its whole job,
// so override the workspace-wide bans inherited from `[lints]`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

//! `audit-check` — AST-walking enforcement that every `RequireAdmin`
//! handler in `crates/server/src/api/` writes an audit-log row via
//! `record_admin_action!` (or `audit::record(...)` directly) before
//! returning success.
//!
//! Audit-remediation M10.2. Per decision #6 in
//! `~/.claude/plans/folio-audit-remediation-1.0.md`.
//!
//! ## How it works
//!
//! 1. Walks `crates/server/src/api/**/*.rs`.
//! 2. Parses each file with `syn::parse_file`.
//! 3. For every `fn` whose argument list contains a `RequireAdmin`
//!    parameter type (either bare or destructured as
//!    `RequireAdmin(actor): RequireAdmin`):
//!    - Skips it if `allowlist.txt` names it as `<file>::<fn>`, the file
//!      relative to `crates/server/src` (e.g. `api/libraries.rs::scan`).
//!      `#` starts a comment.
//!    - Otherwise, walks the function body for any invocation of
//!      `record_admin_action!` or `audit::record(...)` (also accepts
//!      `crate::audit::record(...)`).
//!    - If neither is found, reports the function as a miss.
//! 4. Exits 1 on any unallowed miss (prints `file:line:function` with a
//!    remediation hint) and on any allowlist entry that names no
//!    `RequireAdmin` handler (a stale entry), so the list cannot rot.
//!
//! The tool does not flag `CurrentUser` handlers that widen an admin's
//! write access through an inline role check (e.g. shared reading lists
//! under `/me/cbl-lists`): telling "branches on role, then writes" apart
//! from read-only ACL widening needs data-flow analysis a syntax walk
//! cannot do reliably. Those handlers audit through their own helper and
//! are pinned by integration tests instead.
//!
//! ## Wiring
//!
//! Local: `just audit-check`.  CI: a step in `.github/workflows/ci.yml`
//! invokes `cargo run -p audit-check --release` after `cargo test`.
//!
//! ## Allowlist policy
//!
//! Read-only admin GETs (list / get / dashboard surfaces) belong on
//! the allowlist because they don't mutate state — there's nothing
//! to audit. Anything that touches the DB writes belongs in the
//! mutation path with `record_admin_action!`. The file is a plain
//! text list; comments explain why each entry was added. Entries are
//! keyed by file and function so a handler of the same name elsewhere
//! is never exempted by accident.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use syn::visit::Visit;
use syn::{Expr, ExprCall, ExprPath, ImplItemFn, ItemFn, Macro, Pat, PatType, Type, TypePath};
use walkdir::WalkDir;

fn main() -> ExitCode {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // The audit-check crate lives at `crates/tools/audit-check/`;
    // server's API surface is two levels up + `crates/server/src/api`.
    let workspace_root = manifest_dir
        .ancestors()
        .nth(3)
        .expect("walking up from CARGO_MANIFEST_DIR");
    let src_dir = workspace_root.join("crates/server/src");
    let api_dir = src_dir.join("api");
    let allowlist_path = manifest_dir.join("allowlist.txt");

    let allowlist = match std::fs::read_to_string(&allowlist_path) {
        Ok(raw) => match parse_allowlist(&raw) {
            Ok(set) => set,
            Err(e) => {
                eprintln!("audit-check: {}: {e}", allowlist_path.display());
                return ExitCode::from(2);
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashSet::new(),
        Err(e) => {
            eprintln!(
                "audit-check: could not read {}: {e}",
                allowlist_path.display()
            );
            return ExitCode::from(2);
        }
    };

    let mut scan = Scan::new(&allowlist);
    let mut files = 0usize;
    for entry in WalkDir::new(&api_dir)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        files += 1;
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("audit-check: read {}: {e}", path.display());
                return ExitCode::from(2);
            }
        };
        let rel = relative_key(&src_dir, path);
        if let Err(e) = scan.check_source(&rel, path, &src) {
            eprintln!("audit-check: parse {}: {e}", path.display());
            return ExitCode::from(2);
        }
    }

    let stale = scan.stale_entries();
    if scan.misses.is_empty() && stale.is_empty() {
        println!(
            "audit-check: scanned {files} files under {} — all RequireAdmin handlers audit ✓",
            api_dir.display()
        );
        return ExitCode::SUCCESS;
    }

    if !scan.misses.is_empty() {
        eprintln!(
            "audit-check: {} unaudited RequireAdmin handler(s):",
            scan.misses.len()
        );
        for m in &scan.misses {
            eprintln!(
                "  {}:{} — fn `{}` is admin-gated but never calls `record_admin_action!`",
                m.file.display(),
                m.line,
                m.name,
            );
        }
        eprintln!();
        eprintln!(
            "Add `<file>::<fn>` (file relative to crates/server/src) to \
             `crates/tools/audit-check/allowlist.txt` if it's a read-only admin \
             handler (GET / dashboard / list). Otherwise, call \
             `record_admin_action!` before returning success."
        );
    }
    if !stale.is_empty() {
        eprintln!(
            "audit-check: {} allowlist entr(y/ies) name no RequireAdmin handler:",
            stale.len()
        );
        for key in &stale {
            eprintln!("  {key}");
        }
        eprintln!();
        eprintln!(
            "Remove the entry, or fix its `<file>::<fn>` if the handler moved or was renamed."
        );
    }
    ExitCode::FAILURE
}

/// `crates/server/src`-relative path with `/` separators, the file half of
/// an allowlist key.
fn relative_key(src_dir: &Path, path: &Path) -> String {
    path.strip_prefix(src_dir)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Allowlist entries are `<file>::<fn>`; anything after `#` is a comment.
fn parse_allowlist(raw: &str) -> Result<HashSet<String>, String> {
    let mut out = HashSet::new();
    for (i, line) in raw.lines().enumerate() {
        let entry = line.split('#').next().unwrap_or("").trim();
        if entry.is_empty() {
            continue;
        }
        let valid = entry.split_once("::").is_some_and(|(file, func)| {
            file.ends_with(".rs")
                && !func.is_empty()
                && func.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if !valid {
            return Err(format!(
                "line {}: `{entry}` is not `<file>::<fn>` (e.g. `api/libraries.rs::scan`)",
                i + 1
            ));
        }
        if !out.insert(entry.to_owned()) {
            return Err(format!("line {}: duplicate entry `{entry}`", i + 1));
        }
    }
    Ok(out)
}

#[derive(Debug)]
struct Miss {
    file: PathBuf,
    line: usize,
    name: String,
}

/// Accumulates misses across files and remembers which allowlist entries
/// named a real `RequireAdmin` handler.
struct Scan<'a> {
    allowlist: &'a HashSet<String>,
    used: HashSet<String>,
    misses: Vec<Miss>,
}

impl<'a> Scan<'a> {
    fn new(allowlist: &'a HashSet<String>) -> Self {
        Self {
            allowlist,
            used: HashSet::new(),
            misses: Vec::new(),
        }
    }

    /// Check one source file. `rel` is its allowlist file key, `path` what
    /// a miss reports.
    fn check_source(&mut self, rel: &str, path: &Path, src: &str) -> syn::Result<()> {
        let parsed = syn::parse_file(src)?;
        let mut visitor = HandlerVisitor {
            file: path,
            rel,
            scan: self,
        };
        visitor.visit_file(&parsed);
        Ok(())
    }

    /// Entries that matched no `RequireAdmin` handler, sorted.
    fn stale_entries(&self) -> Vec<String> {
        self.allowlist
            .difference(&self.used)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

struct HandlerVisitor<'s, 'a> {
    file: &'s Path,
    rel: &'s str,
    scan: &'s mut Scan<'a>,
}

impl HandlerVisitor<'_, '_> {
    fn check(
        &mut self,
        name: &str,
        line: usize,
        sig_inputs: &syn::punctuated::Punctuated<syn::FnArg, syn::token::Comma>,
        block: &syn::Block,
    ) {
        if !has_require_admin_arg(sig_inputs) {
            return;
        }
        let key = format!("{}::{name}", self.rel);
        if self.scan.allowlist.contains(&key) {
            self.scan.used.insert(key);
            return;
        }
        let mut audit_finder = AuditMacroFinder::default();
        audit_finder.visit_block(block);
        if !audit_finder.found {
            self.scan.misses.push(Miss {
                file: self.file.to_path_buf(),
                line,
                name: name.to_owned(),
            });
        }
    }
}

impl<'ast> Visit<'ast> for HandlerVisitor<'_, '_> {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let name = node.sig.ident.to_string();
        let line = node.sig.fn_token.span.start().line;
        self.check(&name, line, &node.sig.inputs, &node.block);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        let name = node.sig.ident.to_string();
        let line = node.sig.fn_token.span.start().line;
        self.check(&name, line, &node.sig.inputs, &node.block);
        syn::visit::visit_impl_item_fn(self, node);
    }
}

fn has_require_admin_arg(
    inputs: &syn::punctuated::Punctuated<syn::FnArg, syn::token::Comma>,
) -> bool {
    inputs.iter().any(|arg| match arg {
        syn::FnArg::Typed(PatType { ty, pat, .. }) => {
            // Match on the type's last path segment.
            if type_is_require_admin(ty) {
                return true;
            }
            // Pattern `RequireAdmin(actor): RequireAdmin` — already
            // covered by the type check above. Pattern variants like
            // `RequireAdmin(_)` also work because the type still says
            // RequireAdmin. But guard against future destructure
            // patterns by also checking the pat side.
            matches!(pat.as_ref(), Pat::TupleStruct(tup) if path_ends_with(&tup.path, "RequireAdmin"))
        }
        _ => false,
    })
}

fn type_is_require_admin(ty: &Type) -> bool {
    matches!(ty, Type::Path(TypePath { path, .. }) if path_ends_with(path, "RequireAdmin"))
}

fn path_ends_with(path: &syn::Path, ident: &str) -> bool {
    path.segments
        .last()
        .map(|s| s.ident == ident)
        .unwrap_or(false)
}

#[derive(Default)]
struct AuditMacroFinder {
    found: bool,
}

impl<'ast> Visit<'ast> for AuditMacroFinder {
    fn visit_macro(&mut self, node: &'ast Macro) {
        // Matches both `ExprMacro` and `StmtMacro` invocations of
        // `record_admin_action!` (regardless of how they're imported).
        if path_ends_with(&node.path, "record_admin_action") {
            self.found = true;
            return;
        }
        syn::visit::visit_macro(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        // Match `audit::record(...)` and `crate::audit::record(...)`.
        if let Expr::Path(ExprPath { path, .. }) = node.func.as_ref()
            && path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .ends_with(&["audit".to_owned(), "record".to_owned()])
        {
            self.found = true;
            return;
        }
        syn::visit::visit_expr_call(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
        pub async fn read_only(_a: RequireAdmin) {}
        pub async fn mutates(RequireAdmin(actor): RequireAdmin) { write().await; }
        pub async fn audited(_a: RequireAdmin) { record_admin_action!(db = d); }
        pub async fn audited_call(_a: RequireAdmin) { crate::audit::record(d, e).await; }
        pub async fn not_admin(_u: CurrentUser) { write().await; }
    "#;

    fn run(allow: &str, files: &[(&str, &str)]) -> (Vec<String>, Vec<String>) {
        let allowlist = parse_allowlist(allow).unwrap();
        let mut scan = Scan::new(&allowlist);
        for (rel, src) in files {
            scan.check_source(rel, Path::new(rel), src).unwrap();
        }
        let misses = scan
            .misses
            .iter()
            .map(|m| format!("{}::{}", m.file.display(), m.name))
            .collect();
        (misses, scan.stale_entries())
    }

    #[test]
    fn unaudited_admin_handlers_are_misses() {
        let (misses, stale) = run("", &[("api/a.rs", SRC)]);
        assert_eq!(misses, ["api/a.rs::read_only", "api/a.rs::mutates"]);
        assert!(stale.is_empty());
    }

    #[test]
    fn entry_exempts_only_its_own_file() {
        let (misses, stale) = run(
            "api/a.rs::read_only  # GET, read-only\napi/a.rs::mutates",
            &[("api/a.rs", SRC), ("api/b.rs", SRC)],
        );
        assert_eq!(misses, ["api/b.rs::read_only", "api/b.rs::mutates"]);
        assert!(stale.is_empty());
    }

    #[test]
    fn entry_naming_no_admin_handler_is_stale() {
        let (_, stale) = run(
            "api/a.rs::read_only\napi/a.rs::gone\napi/a.rs::not_admin\napi/c.rs::read_only",
            &[("api/a.rs", SRC)],
        );
        assert_eq!(
            stale,
            [
                "api/a.rs::gone",
                "api/a.rs::not_admin",
                "api/c.rs::read_only"
            ]
        );
    }

    #[test]
    fn bare_names_and_duplicates_are_rejected() {
        assert!(parse_allowlist("list  # old form").is_err());
        assert!(parse_allowlist("api/a.rs::").is_err());
        assert!(parse_allowlist("api/a.rs::x\napi/a.rs::x").is_err());
        let ok = parse_allowlist("# comment\n\n  api/a.rs::x  # why\n").unwrap();
        assert_eq!(ok.len(), 1);
        assert!(ok.contains("api/a.rs::x"));
    }
}
