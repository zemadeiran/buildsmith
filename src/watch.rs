use crate::config::BuildConfig;
use crate::engine::BuildRunner;
use crate::engine::hashing;
use anyhow::Result;
use colored::Colorize;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

pub async fn watch(config: BuildConfig, base_dir: PathBuf, task: Option<String>) -> Result<()> {
    // Collect all input paths to watch
    let watch_paths = collect_watch_paths(&config, &base_dir);

    if watch_paths.is_empty() {
        anyhow::bail!("No input paths found to watch. Add 'inputs' to your tasks.");
    }

    println!(
        "{} Watching {} paths for changes...",
        "[WATCH]".cyan().bold(),
        watch_paths.len()
    );
    println!("{} Press Ctrl+C to stop\n", "[WATCH]".dimmed());

    // Do an initial build
    run_build(&config, &base_dir, &task).await?;

    // Set up file watcher in a blocking thread
    let (tx, rx) = channel::<notify::Result<notify::Event>>();

    let watch_paths_clone = watch_paths.clone();
    std::thread::spawn(move || -> Result<()> {
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            notify::Config::default(),
        )?;

        for path in &watch_paths_clone {
            if path.is_dir() {
                watcher.watch(path, RecursiveMode::Recursive)?;
            } else if path.exists() {
                watcher.watch(path, RecursiveMode::NonRecursive)?;
            }
        }

        // Keep the watcher alive — park this thread
        std::thread::park();
        Ok(())
    });

    // Debounce: collect events for 200ms before triggering rebuild
    let debounce = Duration::from_millis(200);
    let mut last_event: Option<Instant> = None;

    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(event)) => {
                let relevant = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                );
                if !relevant {
                    continue;
                }

                // Ignore buildsmith's own cache directory and target directory
                if event.paths.iter().any(|p| {
                    let s = p.to_string_lossy();
                    s.contains(".buildsmith/cache") || s.contains("/target/")
                }) {
                    continue;
                }

                last_event = Some(Instant::now());
            }
            Ok(Err(e)) => {
                eprintln!("{} Watch error: {}", "[WATCH]".yellow(), e);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("{} Watcher disconnected", "[WATCH]".yellow());
                break;
            }
        }

        // Check if debounce window has elapsed
        if let Some(last) = last_event
            && last.elapsed() >= debounce
        {
            last_event = None;

            // Drain any remaining events
            while rx.try_recv().is_ok() {}

            // Clear screen and rebuild
            print!("\x1b[2J\x1b[H");
            println!(
                "{} Change detected, rebuilding...\n",
                "[WATCH]".cyan().bold()
            );

            run_build(&config, &base_dir, &task).await?;
        }
    }

    Ok(())
}

async fn run_build(config: &BuildConfig, base_dir: &Path, task: &Option<String>) -> Result<()> {
    let runner = BuildRunner::new(config.clone(), base_dir.to_path_buf())?;

    let results = if let Some(t) = task {
        runner.run_task(t, false).await?
    } else {
        runner.run_all(false).await?
    };

    let mut all_success = true;
    for result in &results {
        let status = if result.cached {
            "[CACHED]".cyan()
        } else if result.success {
            "[OK]".green()
        } else {
            all_success = false;
            "[FAIL]".red()
        };
        let desc = config
            .tasks
            .get(&result.name)
            .and_then(|t| t.description.clone())
            .unwrap_or_default();
        let desc_str = if desc.is_empty() {
            String::new()
        } else {
            format!(" - {}", desc)
        };
        println!(
            "{} {:<20} {:>6}ms{}",
            status, result.name, result.duration_ms, desc_str
        );
    }

    let cached = results.iter().filter(|r| r.cached).count();
    let built = results.iter().filter(|r| !r.cached && r.success).count();
    let failed = results.iter().filter(|r| !r.success).count();

    println!(
        "\n{}: {} built, {} cached, {} failed",
        "Summary".bold(),
        built.to_string().green(),
        cached.to_string().cyan(),
        failed.to_string().red(),
    );

    if !all_success {
        println!(
            "\n{} Build failed — waiting for changes...",
            "[WATCH]".yellow()
        );
    } else {
        println!("\n{} Waiting for changes...", "[WATCH]".dimmed());
    }

    Ok(())
}

fn collect_watch_paths(config: &BuildConfig, base_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for task in config.tasks.values() {
        let expanded = hashing::expand_globs(&task.inputs, base_dir);
        for path in expanded {
            if seen.insert(path.clone()) {
                paths.push(path);
            }
        }
    }

    paths
}
