use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BuildConfig {
    #[serde(default)]
    pub project: String,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: PathBuf,
    pub tasks: HashMap<String, TaskConfig>,
}

#[allow(dead_code)]
fn default_cache_dir() -> PathBuf {
    PathBuf::from(".buildsmith/cache")
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskConfig {
    pub command: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}
