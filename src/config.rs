use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub max_suggestions: usize,
    pub weights: HashMap<String, f64>,
    pub disabled_sources: Vec<String>,
    pub history: HistoryConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    pub frequency_weight: f64,
    pub half_life_days: f64,
    pub recency_weight: f64,
    pub short_half_life_minutes: f64,
    pub short_recency_weight: f64,
    pub directory_boost: f64,
    pub sequence_boost: f64,
    pub shape_sequence_boost: f64,
    pub order_recency_weight: f64,
    pub order_half_life: f64,
    pub next_min_observations: u32,
    pub next_min_probability: f64,
    pub failure_penalty: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_suggestions: 5,
            weights: HashMap::new(),
            disabled_sources: Vec::new(),
            history: HistoryConfig::default(),
        }
    }
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            frequency_weight: 0.7,
            half_life_days: 7.0,
            recency_weight: 2.0,
            short_half_life_minutes: 240.0,
            short_recency_weight: 6.0,
            directory_boost: 1.5,
            sequence_boost: 2.0,
            shape_sequence_boost: 6.0,
            order_recency_weight: 4.0,
            order_half_life: 20.0,
            next_min_observations: 3,
            next_min_probability: 0.5,
            failure_penalty: 0.3,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        Self::load_from(&config_dir().join("config.json"))
    }

    pub fn load_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("zghost: ignoring invalid {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn weight(&self, source: &str) -> f64 {
        self.weights.get(source).copied().unwrap_or(1.0)
    }

    pub fn enabled(&self, source: &str) -> bool {
        !self.disabled_sources.iter().any(|s| s == source)
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("zghost")
}

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("zghost")
}

pub fn default_histfile() -> PathBuf {
    std::env::var_os("HISTFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".zsh_history"))
}

pub fn default_socket() -> PathBuf {
    let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let uid = unsafe { libc::getuid() };
    PathBuf::from(tmp.trim_end_matches('/')).join(format!("zghost-{uid}.sock"))
}
