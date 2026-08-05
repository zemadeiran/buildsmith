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
