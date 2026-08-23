//! Comic Reader server library — exposed for integration tests and the binary.

// `axum::response::Response` is the canonical error type for every fallible
// helper in this crate (see the error-envelope convention in CLAUDE.md): the
// `Err` arm is already the finished HTTP response, handed straight back to
// axum. Clippy 1.98 flags it at ~128 bytes, but boxing it would add an
// allocation on every error path and an unwrap at every call site for no
// benefit. Other crates keep the lint.
#![allow(clippy::result_large_err)]

pub mod app;
pub mod config;
pub mod observability;
pub mod secrets;

pub mod api;
pub mod archive_rewrite;
pub mod audit;
pub mod auth;
pub mod build_info;
pub mod cbl;
pub mod email;
pub mod jobs;
pub mod library;
pub mod metadata;
pub mod middleware;
pub mod ocr;
pub mod pages;
pub mod reading;
pub mod settings;
pub mod slug;
pub mod state;
pub mod upstream;
pub mod util;
pub mod views;
