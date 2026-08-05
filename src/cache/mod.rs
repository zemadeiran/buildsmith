use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Cache {
    dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct FileHash {
    pub path: String,
    pub hash: String,
}

impl Cache {
    pub fn new(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        Ok(Cache {
            dir: dir.to_path_buf(),
        })
    }

    fn entry_path(&self, hash: &str) -> PathBuf {
        let prefix = &hash[..2];
        let dir = self.dir.join(prefix);
        dir.join(hash)
    }

    pub fn get(&self, hash: &str) -> Option<PathBuf> {
        let path = self.entry_path(hash);
        if path.exists() { Some(path) } else { None }
    }

    pub fn store_hash(&self, hash: &str) -> Result<()> {
        let path = self.entry_path(hash);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, b"")?;
        Ok(())
    }

    pub fn store_manifest(&self, hash: &str, files: &[FileHash]) -> Result<()> {
        let path = self.entry_path(hash).with_extension("manifest");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut content = String::new();
        for f in files {
            content.push_str(&f.path);
            content.push('\t');
            content.push_str(&f.hash);
            content.push('\n');
        }
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn get_manifest(&self, hash: &str) -> Option<Vec<FileHash>> {
        let path = self.entry_path(hash).with_extension("manifest");
        let content = fs::read_to_string(&path).ok()?;
        let mut files = Vec::new();
        for line in content.lines() {
            let mut parts = line.splitn(2, '\t');
            if let (Some(path), Some(hash)) = (parts.next(), parts.next()) {
                files.push(FileHash {
                    path: path.to_string(),
                    hash: hash.to_string(),
                });
            }
        }
        Some(files)
    }

    fn task_hash_path(&self) -> PathBuf {
        self.dir.join("task_hashes")
    }

    pub fn store_task_hash(&self, task_name: &str, hash: &str) -> Result<()> {
        let path = self.task_hash_path();
        let mut content = fs::read_to_string(&path).unwrap_or_default();
        let mut lines: Vec<String> = content
            .lines()
            .filter(|l| !l.starts_with(&format!("{}\t", task_name)))
            .map(|l| l.to_string())
            .collect();
        lines.push(format!("{}\t{}", task_name, hash));
        content = lines.join("\n");
        if !content.ends_with('\n') {
            content.push('\n');
        }
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn get_task_hash(&self, task_name: &str) -> Option<String> {
        let path = self.task_hash_path();
        let content = fs::read_to_string(&path).ok()?;
        for line in content.lines() {
            let mut parts = line.splitn(2, '\t');
            if let (Some(name), Some(hash)) = (parts.next(), parts.next())
                && name == task_name
            {
                return Some(hash.to_string());
            }
        }
        None
    }

    pub fn store_outputs(&self, hash: &str, output_files: &[PathBuf]) -> Result<()> {
        let cache_entry = self.entry_path(hash);
        let outputs_dir = cache_entry.with_extension("outputs");
        fs::create_dir_all(&outputs_dir)?;

        for file in output_files {
            if !file.exists() {
                continue;
            }
            let rel = file.strip_prefix(std::env::current_dir()?).unwrap_or(file);
            let dest = outputs_dir.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(file, &dest)?;
        }

        Ok(())
    }

    pub fn restore_outputs(&self, cache_entry: &Path, base_dir: &Path) -> Result<()> {
        let outputs_dir = cache_entry.with_extension("outputs");
        if !outputs_dir.exists() {
            return Ok(());
        }

        for entry in walkdir::WalkDir::new(&outputs_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let rel = entry.path().strip_prefix(&outputs_dir)?;
                let dest = base_dir.join(rel);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.path(), &dest)?;
            }
        }

        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        if self.dir.exists() {
            fs::remove_dir_all(&self.dir)?;
            fs::create_dir_all(&self.dir)?;
        }
        Ok(())
    }

    pub fn prune(&self, keep_hashes: &std::collections::HashSet<String>) -> Result<(usize, u64)> {
        let mut removed = 0usize;
        let mut freed = 0u64;

        if !self.dir.exists() {
            return Ok((0, 0));
        }

        for entry in walkdir::WalkDir::new(&self.dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if !keep_hashes.contains(&file_name) {
                    if let Ok(meta) = entry.metadata() {
                        freed += meta.len();
                    }
                    fs::remove_file(entry.path())?;
                    removed += 1;

                    // Try to remove empty parent dir
                    if let Some(parent) = entry.path().parent() {
                        let _ = fs::remove_dir(parent);
                    }
                }
            }
        }

        Ok((removed, freed))
    }

    pub fn entry_count(&self) -> Result<usize> {
        let mut count = 0usize;
        for entry in walkdir::WalkDir::new(&self.dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn size(&self) -> Result<u64> {
        let mut total = 0u64;
        for entry in walkdir::WalkDir::new(&self.dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                total += entry.metadata()?.len();
            }
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_cache_store_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let hash = "abcdef1234567890";
        assert!(cache.get(hash).is_none());
        cache.store_hash(hash).unwrap();
        assert!(cache.get(hash).is_some());
    }

    #[test]
    fn test_cache_manifest_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let hash = "abcdef1234567890";
        let files = vec![
            FileHash { path: "src/main.rs".to_string(), hash: "aaa".to_string() },
            FileHash { path: "src/lib.rs".to_string(), hash: "bbb".to_string() },
        ];
        cache.store_manifest(hash, &files).unwrap();
        let retrieved = cache.get_manifest(hash).unwrap();
        assert_eq!(retrieved.len(), 2);
        assert_eq!(retrieved[0].path, "src/main.rs");
        assert_eq!(retrieved[0].hash, "aaa");
        assert_eq!(retrieved[1].path, "src/lib.rs");
        assert_eq!(retrieved[1].hash, "bbb");
    }

    #[test]
    fn test_cache_manifest_missing() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        assert!(cache.get_manifest("nonexistent").is_none());
    }

    #[test]
    fn test_cache_task_hash_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        cache.store_task_hash("build", "hash123").unwrap();
        cache.store_task_hash("test", "hash456").unwrap();
        assert_eq!(cache.get_task_hash("build").unwrap(), "hash123");
        assert_eq!(cache.get_task_hash("test").unwrap(), "hash456");
    }

    #[test]
    fn test_cache_task_hash_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        cache.store_task_hash("build", "old_hash").unwrap();
        cache.store_task_hash("build", "new_hash").unwrap();
        assert_eq!(cache.get_task_hash("build").unwrap(), "new_hash");
    }

    #[test]
    fn test_cache_task_hash_missing() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        assert!(cache.get_task_hash("nonexistent").is_none());
    }

    #[test]
    fn test_cache_clear() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        cache.store_hash("abcdef1234567890").unwrap();
        assert!(cache.get("abcdef1234567890").is_some());
        cache.clear().unwrap();
        assert!(cache.get("abcdef1234567890").is_none());
    }

    #[test]
    fn test_cache_prune() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let keep = "abcdef1234567890";
        let stale = "fedcba0987654321";
        cache.store_hash(keep).unwrap();
        cache.store_hash(stale).unwrap();
        let keep_set: HashSet<String> = vec![keep.to_string()].into_iter().collect();
        let (removed, _) = cache.prune(&keep_set).unwrap();
        assert!(cache.get(keep).is_some());
        assert!(cache.get(stale).is_none());
        assert!(removed >= 1);
    }

    #[test]
    fn test_cache_entry_count() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        cache.store_hash("abcdef1234567890").unwrap();
        cache.store_hash("fedcba0987654321").unwrap();
        let count = cache.entry_count().unwrap();
        assert!(count >= 2);
    }
}
