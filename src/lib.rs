
pub mod bench;
pub mod config;
pub mod daemon;
pub mod engine;
pub mod parser;
pub mod protocol;
pub mod sources;
pub mod store;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
