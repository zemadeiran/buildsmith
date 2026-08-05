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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn write_config(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("buildsmith.toml");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_parse_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
project = "testproj"

[tasks.build]
command = "echo build"
inputs = ["src"]
description = "Build"

[tasks.test]
command = "echo test"
deps = ["build"]
description = "Test"
"#,
        );
        let config = parse_config(&path).unwrap();
        assert_eq!(config.project, "testproj");
        assert_eq!(config.tasks.len(), 2);
        assert!(config.tasks.contains_key("build"));
        assert!(config.tasks.contains_key("test"));
    }

    #[test]
    fn test_parse_missing_dep_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
project = "testproj"

[tasks.test]
command = "echo test"
deps = ["nonexistent"]
"#,
        );
        let result = parse_config(&path);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nonexistent"));
    }

    #[test]
    fn test_parse_missing_file_fails() {
        let result = parse_config(Path::new("/nonexistent/path.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invalid_toml_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), "this is not valid toml = = =");
        let result = parse_config(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), "");
        let config = parse_config(&path).unwrap();
        assert_eq!(config.project, "");
        assert!(config.tasks.is_empty());
    }

    #[test]
    fn test_parse_default_cache_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
[tasks.build]
command = "echo build"
"#,
        );
        let config = parse_config(&path).unwrap();
        assert_eq!(config.cache_dir, std::path::PathBuf::from(".buildsmith/cache"));
    }
}
