//! Add-to-Mylar (add-to-mylar 1.0): the one place Folio calls Mylar.
//!
//! The Upcoming feature shows a pushed Mylar snapshot and never contacts
//! Mylar ([`crate::upcoming::mylar`]). This module is the deliberate,
//! narrow exception the owner asked for: an admin picks missing
//! reading-list entries and Folio adds their series to Mylar's watchlist,
//! optionally marking the issues Wanted; and a handful of admin actions
//! that keep the wanted list honest (check it against the library, search
//! now, recheck files, refresh a series, un-want an issue).
//!
//! - [`client`] — the HTTP client. It can send exactly the Mylar API
//!   commands of [`client::Command`]; none deletes a series, reconfigures
//!   Mylar or stops it.
//! - [`requests`] — the `mylar_add_request` rows: what was asked for, and
//!   how it went.
//! - [`sweep`] — the once-a-minute tick that carries pending rows out.
//! - [`wanted`] — Mylar's live wanted list checked against the library.
//!
//! Off unless an admin has stored both `integrations.mylar.url` and
//! `integrations.mylar.api_key`.

pub mod client;
pub mod requests;
pub mod sweep;
pub mod wanted;
