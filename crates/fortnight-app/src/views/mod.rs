//! The views. Each has its own `Message` (internal: add variants freely), an
//! `Action` enum and a `*Data` struct (the FROZEN contract with `main.rs`),
//! `load`, `update` that returns an `Action`, and `view`.

pub mod day;
pub mod month;
pub mod report_panel;
pub mod settings;
pub mod task_form;
pub mod unlock;
pub mod week;

/// A key pressed while no text field has focus, forwarded by `main.rs` to
/// the Today tab (PLAN.md section 4.4). FROZEN contract (round 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    /// `↑`: select the previous row.
    Up,
    /// `↓`: select the next row.
    Down,
    /// `Space`: toggle done on the selected row.
    Toggle,
    /// `Enter`: expand the selected row for editing.
    Open,
    /// `Escape`: collapse the expanded row / clear the selection.
    Close,
}
