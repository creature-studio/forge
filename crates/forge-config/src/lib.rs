//! User / project configuration for forge.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Top-level application configuration (stub).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub theme: Option<String>,
}

impl Config {
    pub fn load_default() -> Self {
        Self::default()
    }
}
