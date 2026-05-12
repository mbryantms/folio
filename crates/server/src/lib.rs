//! Comic Reader server library — exposed for integration tests and the binary.

pub mod app;
pub mod config;
pub mod observability;
pub mod secrets;

pub mod api;
pub mod audit;
pub mod auth;
pub mod cbl;
pub mod email;
pub mod jobs;
pub mod library;
pub mod middleware;
pub mod reading;
pub mod slug;
pub mod state;
pub mod views;
