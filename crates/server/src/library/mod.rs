//! Library service module — recursive scan, archive inspection, metadata parse,
//! dedupe, file-watch (Phase 1a + 1b).

pub mod access;
pub mod events;
pub mod hash;
pub mod health;
pub mod identity;
pub mod ignore;
pub mod reconcile;
pub mod scanner;
pub mod thumbnails;
pub mod zip_lru;
