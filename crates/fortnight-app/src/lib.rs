//! Fortnight GUI: theme and views. `main.rs` owns the application state and
//! carries out the actions views return.
//!
//! Views never call the store or the LLM. Each view has its own `Message`
//! (internal: add variants freely), an `Action` enum (the contract with
//! `main.rs`: do not change it without the orchestrator), `update` that
//! returns an `Action`, and `view`.

pub mod theme;
pub mod views;
