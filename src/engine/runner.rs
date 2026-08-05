use crate::cache::{self, Cache};
use crate::config::{BuildConfig, TaskConfig};
use crate::dag::{BuildGraph, Schedule};
use crate::engine::hashing;
use anyhow::Result;
use colored::{Color, Colorize};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;

pub struct BuildRunner {
    config: BuildConfig,
    graph: BuildGraph,
    cache: Cache,
    base_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskResult {
    pub name: String,
    pub success: bool,
    pub cached: bool,
    pub dry_run: bool,
    pub hash: String,
    #[allow(dead_code)]
    pub stdout: String,
    #[allow(dead_code)]
    pub stderr: String,
    pub duration_ms: u128,
    pub start_ms: u128,
    pub end_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskStatus {
    pub name: String,
    pub cached: bool,
    pub hash: String,
    pub input_count: usize,
    pub deps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhyResult {
    pub task: String,
    pub cached: bool,
    pub hash: String,
    pub changed_files: Vec<String>,
    pub removed_files: Vec<String>,
    pub deps: Vec<String>,
    pub dep_stale: Vec<String>,
}

impl BuildRunner {
    pub fn new(config: BuildConfig, base_dir: PathBuf) -> Result<Self> {
        let graph = BuildGraph::from_config(&config)?;
        let cache = Cache::new(&base_dir.join(&config.cache_dir))?;
        Ok(BuildRunner {
            config,
            graph,
            cache,
            base_dir,
        })
    }

    #[allow(dead_code)]
    pub fn task_count(&self) -> usize {
        self.graph.task_count()
    }

    pub async fn run_all(&self, dry_run: bool) -> Result<Vec<TaskResult>> {
        self.run_all_with(dry_run, false).await
    }

    pub async fn run_all_with(&self, dry_run: bool, quiet: bool) -> Result<Vec<TaskResult>> {
        self.run_all_with_flags(dry_run, quiet, false, None, false)
            .await
    }

    pub async fn run_all_with_flags(
        &self,
        dry_run: bool,
        quiet: bool,
        keep_going: bool,
        max_jobs: Option<usize>,
        force: bool,
    ) -> Result<Vec<TaskResult>> {
        let schedule = Schedule::from_graph(&self.graph);
        self.run_schedule(&schedule, dry_run, quiet, keep_going, max_jobs, force)
            .await
    }

    pub async fn run_task(&self, task_name: &str, dry_run: bool) -> Result<Vec<TaskResult>> {
        self.run_task_with(task_name, dry_run, false).await
    }

    pub async fn run_task_with(
        &self,
        task_name: &str,
        dry_run: bool,
        quiet: bool,
    ) -> Result<Vec<TaskResult>> {
        self.run_task_with_flags(task_name, dry_run, quiet, false, None, false)
            .await
    }

    pub async fn run_task_with_flags(
        &self,
        task_name: &str,
        dry_run: bool,
        quiet: bool,
        keep_going: bool,
        max_jobs: Option<usize>,
        force: bool,
    ) -> Result<Vec<TaskResult>> {
        if !self.config.tasks.contains_key(task_name) {
            anyhow::bail!("Unknown task: {}", task_name);
        }
        let schedule = Schedule::for_task(&self.graph, task_name);
        self.run_schedule(&schedule, dry_run, quiet, keep_going, max_jobs, force)
            .await
    }

    pub async fn current_hashes(&self) -> Result<HashSet<String>> {
        let schedule = Schedule::from_graph(&self.graph);
        let mut hashes: HashMap<String, String> = HashMap::new();
        let mut all_hashes = HashSet::new();

        for level in &schedule.levels {
            for name in level {
                let task = &self.config.tasks[name];
                let input_files = hashing::expand_globs(&task.inputs, &self.base_dir);
                let input_hash = hashing::hash_files(&input_files)?;
                let dep_hashes: Vec<(String, String)> = task
                    .deps
                    .iter()
                    .filter_map(|d| hashes.get(d).map(|h| (d.clone(), h.clone())))
                    .collect();
                let task_hash = hashing::hash_task(&task.command, &input_hash, &dep_hashes);
                hashes.insert(name.clone(), task_hash.clone());
                all_hashes.insert(task_hash);
            }
        }

        Ok(all_hashes)
    }

    pub async fn why_stale(&self, task_name: &str) -> Result<WhyResult> {
        if !self.config.tasks.contains_key(task_name) {
            anyhow::bail!("Unknown task: {}", task_name);
        }

        let schedule = Schedule::for_task(&self.graph, task_name);
        let mut hashes: HashMap<String, String> = HashMap::new();

        // Compute hashes in dependency order
        for level in &schedule.levels {
            for name in level {
                let task = &self.config.tasks[name];
                let input_files = hashing::expand_globs(&task.inputs, &self.base_dir);
                let input_hash = hashing::hash_files(&input_files)?;
                let dep_hashes: Vec<(String, String)> = task
                    .deps
                    .iter()
                    .filter_map(|d| hashes.get(d).map(|h| (d.clone(), h.clone())))
                    .collect();
                let task_hash = hashing::hash_task(&task.command, &input_hash, &dep_hashes);
                hashes.insert(name.clone(), task_hash);
            }
        }

        let task = &self.config.tasks[task_name];
        let task_hash = &hashes[task_name];
        let cached = self.cache.get(task_hash).is_some();

        // Look up the previously cached hash for this task (may differ from current)
        let old_hash = self.cache.get_task_hash(task_name);
        let old_manifest = old_hash.as_ref().and_then(|h| self.cache.get_manifest(h));

        if cached {
            // Current hash is cached — up to date
            return Ok(WhyResult {
                task: task_name.to_string(),
                cached: true,
                hash: task_hash.clone(),
                changed_files: Vec::new(),
                removed_files: Vec::new(),
                deps: task.deps.clone(),
                dep_stale: task
                    .deps
                    .iter()
                    .filter(|d| !self.cache.get(&hashes[*d]).is_some())
                    .cloned()
                    .collect(),
            });
        }

        // Not cached — compare with old manifest if available
        let mut changed = Vec::new();
        let mut removed = Vec::new();

        if let Some(manifest) = old_manifest {
            let current_files = hashing::expand_globs(&task.inputs, &self.base_dir);
            let manifest_map: HashMap<String, String> = manifest
                .iter()
                .map(|f| (f.path.clone(), f.hash.clone()))
                .collect();

            for file in &current_files {
                let path_str = file.to_string_lossy().to_string();
                let current_hash = hashing::hash_file(file)?;
                match manifest_map.get(&path_str) {
                    Some(cached_hash) if cached_hash != &current_hash => {
                        changed.push(path_str);
                    }
                    None => {
                        changed.push(path_str);
                    }
                    _ => {}
                }
            }

            for f in &manifest {
                if !current_files.iter().any(|p| p.to_string_lossy() == f.path) {
                    removed.push(f.path.clone());
                }
            }
        }

        let dep_stale: Vec<String> = task
            .deps
            .iter()
            .filter(|d| !self.cache.get(&hashes[*d]).is_some())
            .cloned()
            .collect();

        Ok(WhyResult {
            task: task_name.to_string(),
            cached: false,
            hash: task_hash.clone(),
            changed_files: changed,
            removed_files: removed,
            deps: task.deps.clone(),
            dep_stale,
        })
    }

    pub async fn check_status(&self, task_name: Option<&str>) -> Result<Vec<TaskStatus>> {
        let schedule = if let Some(t) = task_name {
            if !self.config.tasks.contains_key(t) {
                anyhow::bail!("Unknown task: {}", t);
            }
            Schedule::for_task(&self.graph, t)
        } else {
            Schedule::from_graph(&self.graph)
        };

        let mut hashes: HashMap<String, String> = HashMap::new();
        let mut statuses = Vec::new();

        for level in &schedule.levels {
            for name in level {
                let task = &self.config.tasks[name];
                let input_files = hashing::expand_globs(&task.inputs, &self.base_dir);
                let input_hash = hashing::hash_files(&input_files)?;

                let dep_hashes: Vec<(String, String)> = task
                    .deps
                    .iter()
                    .filter_map(|d| hashes.get(d).map(|h| (d.clone(), h.clone())))
                    .collect();

                let task_hash = hashing::hash_task(&task.command, &input_hash, &dep_hashes);
                let cached = self.cache.get(&task_hash).is_some();

                hashes.insert(name.clone(), task_hash.clone());

                statuses.push(TaskStatus {
                    name: name.clone(),
                    cached,
                    hash: task_hash,
                    input_count: input_files.len(),
                    deps: task.deps.clone(),
                });
            }
        }

        Ok(statuses)
    }

    async fn run_schedule(
        &self,
        schedule: &Schedule,
        dry_run: bool,
        quiet: bool,
        keep_going: bool,
        max_jobs: Option<usize>,
        force: bool,
    ) -> Result<Vec<TaskResult>> {
        let build_start = std::time::Instant::now();
        let semaphore = max_jobs.map(|n| Arc::new(Semaphore::new(n)));
        let all_tasks: HashSet<String> = schedule.levels.iter().flatten().cloned().collect();
        let results: Arc<Mutex<HashMap<String, TaskResult>>> = Arc::new(Mutex::new(HashMap::new()));
        let task_hashes: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let completed: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let failed: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        let mut join_set: JoinSet<(String, TaskResult)> = JoinSet::new();
        let mut spawned: HashSet<String> = HashSet::new();
        let mut abort = false;

        loop {
            // Check for tasks that should be skipped due to failed deps
            {
                let completed_guard = completed.lock().await;
                let failed_guard = failed.lock().await;
                let results_guard = results.lock().await;

                let to_skip: Vec<String> = all_tasks
                    .iter()
                    .filter(|name| !results_guard.contains_key(*name) && !spawned.contains(*name))
                    .filter(|name| {
                        let deps = &self.config.tasks[*name].deps;
                        deps.iter().any(|d| failed_guard.contains(d))
                    })
                    .cloned()
                    .collect();

                drop(completed_guard);
                drop(failed_guard);
                drop(results_guard);

                for name in &to_skip {
                    let task = &self.config.tasks[name];
                    let dep_str = task.deps.join(", ");
                    let result = TaskResult {
                        name: name.clone(),
                        success: false,
                        cached: false,
                        dry_run: false,
                        hash: String::new(),
                        stdout: String::new(),
                        stderr: format!("Skipped due to failed dependency: {}", dep_str),
                        duration_ms: 0,
                        start_ms: 0,
                        end_ms: 0,
                    };
                    let mut results_guard = results.lock().await;
                    results_guard.insert(name.clone(), result);
                    let mut completed_guard = completed.lock().await;
                    completed_guard.insert(name.clone());
                    let mut failed_guard = failed.lock().await;
                    failed_guard.insert(name.clone());
                }
            }

            // If not keep_going and something failed, stop spawning new tasks
            if abort {
                if join_set.is_empty() {
                    break;
                }
            } else {
                // Find tasks that are ready (all deps completed, not yet started)
                let completed_guard = completed.lock().await;
                let results_guard = results.lock().await;
                let failed_guard = failed.lock().await;
                let ready: Vec<String> = all_tasks
                    .iter()
                    .filter(|name| !results_guard.contains_key(*name) && !spawned.contains(*name))
                    .filter(|name| {
                        let deps = &self.config.tasks[*name].deps;
                        deps.iter().all(|d| completed_guard.contains(d))
                            && !deps.iter().any(|d| failed_guard.contains(d))
                    })
                    .cloned()
                    .collect();
                drop(completed_guard);
                drop(results_guard);
                drop(failed_guard);

                // Spawn all ready tasks
                for task_name in &ready {
                    let task = self.config.tasks[task_name].clone();
                    let name = task_name.clone();
                    let base_dir = self.base_dir.clone();
                    let cache = self.cache.clone();
                    let task_hashes = Arc::clone(&task_hashes);

                    spawned.insert(name.clone());
                    let sem = semaphore.clone();
                    join_set.spawn(async move {
                        let _permit = if let Some(ref sem) = sem {
                            Some(sem.acquire().await.expect("semaphore closed"))
                        } else {
                            None
                        };
                        let result = run_single_task(
                            &name,
                            &task,
                            &base_dir,
                            &cache,
                            &task_hashes,
                            dry_run,
                            quiet,
                            build_start,
                            force,
                        )
                        .await;
                        (name, result)
                    });
                }
            }

            if join_set.is_empty() {
                break;
            }

            // Wait for at least one task to complete
            let (name, result) = join_set
                .join_next()
                .await
                .ok_or_else(|| anyhow::anyhow!("JoinSet empty"))??;

            if !result.success && !result.cached && !result.dry_run {
                let mut failed_guard = failed.lock().await;
                failed_guard.insert(name.clone());
                if !keep_going {
                    abort = true;
                }
            }

            {
                let mut hashes_guard = task_hashes.lock().await;
                hashes_guard.insert(name.clone(), result.hash.clone());
            }
            {
                let mut completed_guard = completed.lock().await;
                completed_guard.insert(name.clone());
            }
            {
                let mut results_guard = results.lock().await;
                results_guard.insert(name, result);
            }
        }

        // Collect results in topological order
        let results_guard = results.lock().await;
        let order: Vec<String> = schedule.levels.iter().flatten().cloned().collect();
        let results_vec = order
            .iter()
            .filter_map(|name| results_guard.get(name).cloned())
            .collect();
        Ok(results_vec)
    }
}

fn color_for_task(name: &str) -> Color {
    const PALETTE: &[Color] = &[
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Magenta,
        Color::Blue,
        Color::Red,
        Color::BrightCyan,
        Color::BrightGreen,
        Color::BrightYellow,
        Color::BrightMagenta,
        Color::BrightBlue,
        Color::BrightRed,
    ];
    let hash: u32 = name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    PALETTE[(hash as usize) % PALETTE.len()]
}

#[allow(clippy::too_many_arguments)]
async fn run_single_task(
    name: &str,
    task: &TaskConfig,
    base_dir: &Path,
    cache: &Cache,
    task_hashes: &Arc<Mutex<HashMap<String, String>>>,
    dry_run: bool,
    quiet: bool,
    build_start: std::time::Instant,
    force: bool,
) -> TaskResult {
    let start = std::time::Instant::now();

    // Compute input hash
    let input_files = hashing::expand_globs(&task.inputs, base_dir);
    let input_hash = match hashing::hash_files(&input_files) {
        Ok(h) => h,
        Err(e) => {
            return TaskResult {
                name: name.to_string(),
                success: false,
                cached: false,
                dry_run: false,
                hash: String::new(),
                stdout: String::new(),
                stderr: format!("Failed to hash inputs: {}", e),
                duration_ms: start.elapsed().as_millis(),
                start_ms: build_start.elapsed().as_millis(),
                end_ms: build_start.elapsed().as_millis(),
            };
        }
    };

    // Get dependency hashes
    let hashes_guard = task_hashes.lock().await;
    let dep_hashes: Vec<(String, String)> = task
        .deps
        .iter()
        .filter_map(|d| hashes_guard.get(d).map(|h| (d.clone(), h.clone())))
        .collect();
    drop(hashes_guard);

    // Compute task hash
    let task_hash = hashing::hash_task(&task.command, &input_hash, &dep_hashes);

    // Check cache (skip if --force)
    if !force && let Some(cached_outputs) = cache.get(&task_hash) {
        if !dry_run {
            // Restore outputs from cache
            if let Err(e) = cache.restore_outputs(&cached_outputs, base_dir) {
                eprintln!(
                    "{} Cache restore failed for '{}': {}",
                    "[CACHE]".yellow(),
                    name,
                    e
                );
            }
        }
        return TaskResult {
            name: name.to_string(),
            success: true,
            cached: true,
            dry_run: false,
            hash: task_hash,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: start.elapsed().as_millis(),
            start_ms: build_start.elapsed().as_millis(),
            end_ms: build_start.elapsed().as_millis(),
        };
    }

    // Dry run: report what would be executed without running it
    if dry_run {
        return TaskResult {
            name: name.to_string(),
            success: true,
            cached: false,
            dry_run: true,
            hash: task_hash,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: start.elapsed().as_millis(),
            start_ms: build_start.elapsed().as_millis(),
            end_ms: build_start.elapsed().as_millis(),
        };
    }

    // Execute the command with streaming output
    let mut child = match Command::new("/bin/sh")
        .arg("-c")
        .arg(&task.command)
        .current_dir(base_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return TaskResult {
                name: name.to_string(),
                success: false,
                cached: false,
                dry_run: false,
                hash: task_hash,
                stdout: String::new(),
                stderr: format!("Failed to execute: {}", e),
                duration_ms: start.elapsed().as_millis(),
                start_ms: build_start.elapsed().as_millis(),
                end_ms: build_start.elapsed().as_millis(),
            };
        }
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let prefix = format!("{:>10} \u{2502}", name)
        .color(color_for_task(name))
        .to_string();

    // Read stdout and stderr concurrently, streaming lines to terminal
    let stdout_task = tokio::spawn({
        let prefix = prefix.clone();
        async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            let mut buf = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                if !quiet {
                    println!("{} {}", prefix, line);
                }
                buf.push_str(&line);
                buf.push('\n');
            }
            buf
        }
    });

    let stderr_task = tokio::spawn({
        let prefix = prefix.clone();
        async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            let mut buf = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                if !quiet {
                    eprintln!("{} {}", prefix, line);
                }
                buf.push_str(&line);
                buf.push('\n');
            }
            buf
        }
    });

    let status = child.wait().await;
    let stdout_capture = stdout_task.await.unwrap_or_default();
    let stderr_capture = stderr_task.await.unwrap_or_default();

    let duration_ms = start.elapsed().as_millis();
    let end_ms = build_start.elapsed().as_millis();
    let start_ms = end_ms - duration_ms;

    match status {
        Ok(s) => {
            let success = s.success();

            if success {
                let output_files = hashing::expand_globs(&task.outputs, base_dir);
                if let Err(e) = cache.store_outputs(&task_hash, &output_files) {
                    eprintln!(
                        "{} Cache store failed for '{}': {}",
                        "[CACHE]".yellow(),
                        name,
                        e
                    );
                }
                cache.store_hash(&task_hash).ok();

                // Store manifest for forge why
                let manifest: Vec<cache::FileHash> = input_files
                    .iter()
                    .filter_map(|p| {
                        hashing::hash_file(p).ok().map(|h| cache::FileHash {
                            path: p.to_string_lossy().to_string(),
                            hash: h,
                        })
                    })
                    .collect();
                cache.store_manifest(&task_hash, &manifest).ok();
                cache.store_task_hash(name, &task_hash).ok();
            }

            TaskResult {
                name: name.to_string(),
                success,
                cached: false,
                dry_run: false,
                hash: task_hash,
                stdout: stdout_capture,
                stderr: stderr_capture,
                duration_ms,
                start_ms,
                end_ms,
            }
        }
        Err(e) => TaskResult {
            name: name.to_string(),
            success: false,
            cached: false,
            dry_run: false,
            hash: task_hash,
            stdout: stdout_capture,
            stderr: format!("{}{}", stderr_capture, e),
            duration_ms,
            start_ms,
            end_ms,
        },
    }
}
