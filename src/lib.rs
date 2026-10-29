//! zghost: Warp-style inline autosuggestions for Zsh.
//!
//! A long-running daemon owns the history index and context sources; a thin
//! ZLE plugin streams the current buffer to it and renders the top
//! candidate as ghost text.

pub mod bench;
pub mod config;
pub mod daemon;
pub mod engine;
pub mod parser;
pub mod protocol;
pub mod sources;
pub mod store;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
