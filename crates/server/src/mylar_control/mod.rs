//! Add-to-Mylar (add-to-mylar 1.0): the one place Folio calls Mylar.
//!
//! The Upcoming feature shows a pushed Mylar snapshot and never contacts
//! Mylar ([`crate::upcoming::mylar`]). This module is the deliberate,
//! narrow exception the owner asked for: an admin picks missing
//! reading-list entries and Folio adds their series to Mylar's watchlist,
//! optionally marking the issues Wanted.
//!
//! - [`client`] — the HTTP client. It can send exactly five Mylar API
//!   commands ([`client::Command`]); none deletes, pauses or restarts
//!   anything.
//! - [`requests`] — the `mylar_add_request` rows: what was asked for, and
//!   how it went.
//! - [`sweep`] — the once-a-minute tick that carries pending rows out.
//!
//! Off unless an admin has stored both `integrations.mylar.url` and
//! `integrations.mylar.api_key`.

pub mod client;
pub mod requests;
pub mod sweep;
