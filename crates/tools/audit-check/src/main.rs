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
//!    - Skips it if the function name appears in `allowlist.txt`
//!      (one name per line, `#`-prefixed comments allowed).
//!    - Otherwise, walks the function body for any invocation of
//!      `record_admin_action!` or `audit::record(...)` (also accepts
//!      `crate::audit::record(...)`).
//!    - If neither is found, reports the function as a miss.
//! 4. Exits 1 on any unallowed miss; prints `file:line:function`
//!    with a remediation hint.
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
//! text list; comments explain why each entry was added.

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
    let api_dir = workspace_root.join("crates/server/src/api");
    let allowlist_path = manifest_dir.join("allowlist.txt");

    let allowlist = match load_allowlist(&allowlist_path) {
        Ok(set) => set,
        Err(e) => {
            eprintln!(
                "audit-check: could not read {}: {e}",
                allowlist_path.display()
            );
            return ExitCode::from(2);
        }
    };

    let mut misses: Vec<Miss> = Vec::new();
    for entry in WalkDir::new(&api_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("audit-check: read {}: {e}", path.display());
                return ExitCode::from(2);
            }
        };
        let parsed = match syn::parse_file(&src) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("audit-check: parse {}: {e}", path.display());
                return ExitCode::from(2);
            }
        };
        let mut visitor = HandlerVisitor::new(path, &allowlist);
        visitor.visit_file(&parsed);
        misses.extend(visitor.misses);
    }

    if misses.is_empty() {
        let count = WalkDir::new(&api_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .count();
        println!(
            "audit-check: scanned {count} files under {} — all RequireAdmin handlers audit ✓",
            api_dir.display()
        );
        return ExitCode::SUCCESS;
    }

    eprintln!(
        "audit-check: {} unaudited RequireAdmin handler(s):",
        misses.len()
    );
    for m in &misses {
        eprintln!(
            "  {}:{} — fn `{}` is admin-gated but never calls `record_admin_action!`",
            m.file.display(),
            m.line,
            m.name,
        );
    }
    eprintln!();
    eprintln!(
        "Add the function to `crates/tools/audit-check/allowlist.txt` if it's a \
         read-only admin handler (GET / dashboard / list). Otherwise, call \
         `record_admin_action!` before returning success."
    );
    ExitCode::FAILURE
}

#[derive(Debug)]
struct Miss {
    file: PathBuf,
    line: usize,
    name: String,
}

struct HandlerVisitor<'a> {
    file: &'a Path,
    allowlist: &'a std::collections::HashSet<String>,
    misses: Vec<Miss>,
}

impl<'a> HandlerVisitor<'a> {
    fn new(file: &'a Path, allowlist: &'a std::collections::HashSet<String>) -> Self {
        Self {
            file,
            allowlist,
            misses: Vec::new(),
        }
    }

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
        if self.allowlist.contains(name) {
            return;
        }
        let mut audit_finder = AuditMacroFinder::default();
        audit_finder.visit_block(block);
        if !audit_finder.found {
            self.misses.push(Miss {
                file: self.file.to_path_buf(),
                line,
                name: name.to_owned(),
            });
        }
    }
}

impl<'ast, 'a> Visit<'ast> for HandlerVisitor<'a> {
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

fn load_allowlist(path: &Path) -> std::io::Result<std::collections::HashSet<String>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(std::collections::HashSet::new());
        }
        Err(e) => return Err(e),
    };
    Ok(raw
        .lines()
        .filter_map(|line| {
            let trimmed = line.split('#').next().unwrap_or("").trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        })
        .collect())
}
