//! Fortnight GUI: theme and views. `main.rs` owns the application state and
//! carries out the actions views return.
//!
//! Views never call the store or the LLM. Each view has its own `Message`
//! (internal: add variants freely), an `Action` enum and `*Data` struct
//! (the FROZEN contract with `main.rs`), `update` that
//! returns an `Action`, and `view`.

pub mod theme;
pub mod top_bar;
pub mod views;
