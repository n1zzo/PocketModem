//! UI module for PocketModem
//!
//! Clean separation of UI code from main application logic.
//! This module imports from other app modules but main.rs imports from ui.

mod components;
mod pages;
mod styles;

pub use components::*;
pub use pages::*;
pub use styles::*;