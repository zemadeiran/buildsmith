use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn hash_files(paths: &[PathBuf]) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut sorted: Vec<_> = paths.iter().collect();
    sorted.sort();

    for path in &sorted {
        let path_str = path.to_string_lossy();
        hasher.update(path_str.as_bytes());
        hasher.update(b"\0");
        if path.exists() {
            let hash = hash_file(path)?;
            hasher.update(hash.as_bytes());
        } else {
            hasher.update(b"<missing>");
        }
        hasher.update(b"\0");
    }

    Ok(hex::encode(hasher.finalize()))
}

#[allow(dead_code)]
pub fn hash_command(command: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(command.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn hash_task(command: &str, input_hash: &str, dep_hashes: &[(String, String)]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"cmd:");
    hasher.update(command.as_bytes());
    hasher.update(b"\ninputs:");
    hasher.update(input_hash.as_bytes());
    hasher.update(b"\ndeps:");
    let mut sorted_deps: Vec<_> = dep_hashes.to_vec();
    sorted_deps.sort();
    for (name, hash) in &sorted_deps {
        hasher.update(name.as_bytes());
        hasher.update(b":");
        hasher.update(hash.as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

pub fn expand_globs(patterns: &[String], base: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for pattern in patterns {
        let full = base.join(pattern);
        if full.is_dir() {
            for entry in WalkDir::new(&full).into_iter().filter_map(|e| e.ok()) {
                if entry.file_type().is_file() {
                    result.push(entry.path().to_path_buf());
                }
            }
        } else if full.exists() {
            result.push(full);
        }
    }
    result.sort();
    result.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_file_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, b"hello world").unwrap();
        let h1 = hash_file(&path).unwrap();
        let h2 = hash_file(&path).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_hash_file_changes_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, b"content a").unwrap();
        let h1 = hash_file(&path).unwrap();
        fs::write(&path, b"content b").unwrap();
        let h2 = hash_file(&path).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_files_order_independent() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        fs::write(&p1, b"aaa").unwrap();
        fs::write(&p2, b"bbb").unwrap();
        let h1 = hash_files(&[p1.clone(), p2.clone()]).unwrap();
        let h2 = hash_files(&[p2.clone(), p1.clone()]).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_task_deterministic() {
        let deps = vec![("check".to_string(), "abc123".to_string())];
        let h1 = hash_task("cargo build", "input_hash", &deps);
        let h2 = hash_task("cargo build", "input_hash", &deps);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_task_different_command() {
        let deps = vec![];
        let h1 = hash_task("cargo build", "abc", &deps);
        let h2 = hash_task("cargo test", "abc", &deps);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_task_different_deps_order() {
        let deps1 = vec![("a".to_string(), "1".to_string()), ("b".to_string(), "2".to_string())];
        let deps2 = vec![("b".to_string(), "2".to_string()), ("a".to_string(), "1".to_string())];
        let h1 = hash_task("cmd", "inputs", &deps1);
        let h2 = hash_task("cmd", "inputs", &deps2);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_expand_globs_directory() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.rs"), b"").unwrap();
        fs::write(src.join("b.rs"), b"").unwrap();
        let patterns = vec!["src".to_string()];
        let files = expand_globs(&patterns, dir.path());
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.ends_with("a.rs")));
        assert!(files.iter().any(|f| f.ends_with("b.rs")));
    }

    #[test]
    fn test_expand_globs_single_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), b"").unwrap();
        let patterns = vec!["Cargo.toml".to_string()];
        let files = expand_globs(&patterns, dir.path());
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_expand_globs_missing() {
        let dir = tempfile::tempdir().unwrap();
        let patterns = vec!["nonexistent".to_string()];
        let files = expand_globs(&patterns, dir.path());
        assert!(files.is_empty());
    }
}
