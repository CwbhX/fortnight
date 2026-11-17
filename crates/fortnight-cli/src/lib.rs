//! `fortnight-cli`: the headless shell over the core (PLAN.md section 8).
//!
//! `commands` holds the logic over `&dyn Store` and a writer; `llm_setup`
//! chooses the endpoint and key; `run` is the glue `main` calls.

pub mod args;
pub mod commands;
pub mod error;
pub mod llm_setup;
pub mod run;
