use crate::config::BuildConfig;
use anyhow::{Context, Result};
use std::path::Path;

pub fn parse_config(path: &Path) -> Result<BuildConfig> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let config: BuildConfig = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &BuildConfig) -> Result<()> {
    for (name, task) in &config.tasks {
        for dep in &task.deps {
            if !config.tasks.contains_key(dep) {
                anyhow::bail!("Task '{}' depends on unknown task '{}'", name, dep);
            }
        }
    }
    Ok(())
}
