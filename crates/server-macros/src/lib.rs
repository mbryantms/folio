//! Procedural macros for the Folio server crate.
//!
//! Today this crate exports a single attribute: [`macro@handler`]. It wraps an
//! axum handler with a `tracing::instrument` span carrying useful default
//! fields. The macro walks the function signature looking for `CurrentUser` /
//! `RequireAdmin` arguments and seeds the span with a `user_id` field
//! automatically — no per-handler boilerplate.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{FnArg, ItemFn, Pat, PatIdent, PatTupleStruct, PatType, Type, parse_macro_input};

/// Annotate an axum handler with consistent tracing context.
///
/// Expands to `#[tracing::instrument(skip_all, name = "<fn_name>")]` plus
/// `fields(user_id = %<binding>.id)` (or `.0.id` for `RequireAdmin`) when an
/// argument of the supported extractor types is found. The span name is the
/// handler's identifier so log output shows `handler{user_id=...}` rather
/// than the generic `instrument`.
///
/// Supported extractor patterns:
///   - `user: CurrentUser` → `user_id = %user.id`
///   - `_user: CurrentUser` → `user_id = %_user.id` (underscore prefix
///     is a binding hint, not a barrier to access)
///   - `RequireAdmin(actor): RequireAdmin` → `user_id = %actor.id`
///   - `admin: RequireAdmin` → `user_id = %admin.0.id`
///
/// # Example
///
/// ```ignore
/// #[handler]
/// pub async fn list_users(
///     State(app): State<AppState>,
///     _admin: RequireAdmin,
/// ) -> impl IntoResponse {
///     // span: list_users{user_id=...}
/// }
/// ```
#[proc_macro_attribute]
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let fn_name = input.sig.ident.to_string();

    let user_id_expr = find_user_id_expr(&input.sig.inputs);

    let attrs = &input.attrs;
    let vis = &input.vis;
    let sig = &input.sig;
    let block = &input.block;

    let instrument = if let Some(expr) = user_id_expr {
        quote! {
            #[tracing::instrument(skip_all, name = #fn_name, fields(user_id = %#expr))]
        }
    } else {
        quote! {
            #[tracing::instrument(skip_all, name = #fn_name)]
        }
    };

    let expanded = quote! {
        #(#attrs)*
        #instrument
        #vis #sig #block
    };

    expanded.into()
}

/// Walk a function's argument list and return the expression that yields
/// the current user's id, if one of the supported extractor types is
/// present. Returns `None` for handlers that don't carry user identity
/// (e.g. unauth health probes); those still get the named span without a
/// `user_id` field.
fn find_user_id_expr(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> Option<TokenStream2> {
    for arg in inputs {
        let FnArg::Typed(PatType { pat, ty, .. }) = arg else {
            continue;
        };
        let kind = classify_extractor(ty)?;
        match kind {
            ExtractorKind::CurrentUser => {
                if let Pat::Ident(PatIdent { ident, .. }) = pat.as_ref() {
                    return Some(quote! { #ident.id });
                }
            }
            ExtractorKind::RequireAdmin => match pat.as_ref() {
                Pat::Ident(PatIdent { ident, .. }) => {
                    return Some(quote! { #ident.0.id });
                }
                Pat::TupleStruct(PatTupleStruct { elems, .. }) => {
                    // Pattern: `RequireAdmin(actor)` — destructured; first
                    // elem is the inner `CurrentUser` binding name.
                    if let Some(Pat::Ident(PatIdent { ident, .. })) = elems.first() {
                        return Some(quote! { #ident.id });
                    }
                }
                _ => {}
            },
        }
    }
    None
}

enum ExtractorKind {
    CurrentUser,
    RequireAdmin,
}

/// Match the *last* path segment of the type so qualified paths
/// (`crate::auth::CurrentUser`, `auth::RequireAdmin`) work alongside the
/// bare `CurrentUser` / `RequireAdmin` shorthand most handlers use.
fn classify_extractor(ty: &Type) -> Option<ExtractorKind> {
    let Type::Path(tp) = ty else { return None };
    let segment = tp.path.segments.last()?;
    match segment.ident.to_string().as_str() {
        "CurrentUser" => Some(ExtractorKind::CurrentUser),
        "RequireAdmin" => Some(ExtractorKind::RequireAdmin),
        _ => None,
    }
}
