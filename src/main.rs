use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::PathBuf;

mod cache;
mod config;
mod dag;
mod engine;
mod watch;

use config::parse_config;
use engine::BuildRunner;

#[derive(Parser)]
#[command(
    name = "buildsmith",
    version,
    about = "A content-hashed, DAG-based build system"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to the build config file (default: buildsmith.toml)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Output results as JSON
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run all tasks or a specific task
    Build {
        /// Specific task to build (runs all if omitted)
        task: Option<String>,

        /// Show what would be built without executing anything
        #[arg(long)]
        dry_run: bool,

        /// Continue building independent tasks after a failure
        #[arg(long)]
        keep_going: bool,

        /// Show a build timeline after completion
        #[arg(long)]
        timeline: bool,

        /// Maximum number of tasks to run concurrently
        #[arg(short = 'j', long)]
        jobs: Option<usize>,

        /// Ignore cache and force rebuild all tasks
        #[arg(long)]
        force: bool,
    },

    /// Show the build graph
    Graph {
        /// Output format: text or dot
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Clear the build cache
    Clean,

    /// Show cache statistics
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },

    /// List all defined tasks
    List,

    /// Show which tasks are cached and which would rebuild
    Status {
        /// Specific task to check (checks all if omitted)
        task: Option<String>,
    },

    /// Explain why a task is stale (which inputs changed)
    Why {
        /// Task to analyze
        task: String,
    },

    /// Initialize a new buildsmith.toml in the current directory
    Init,

    /// Watch for file changes and rebuild automatically
    Watch {
        /// Specific task to build (runs all if omitted)
        task: Option<String>,
    },
}

#[derive(Subcommand)]
enum CacheAction {
    /// Show cache statistics
    Stats,
    /// Clear the cache
    Clear,
    /// Remove stale cache entries not referenced by any task
    Prune,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_path = cli
        .config
        .unwrap_or_else(|| PathBuf::from("buildsmith.toml"));
    let base_dir = std::env::current_dir()?;

    // Init doesn't need an existing config
    if matches!(cli.command, Commands::Init) {
        let target = base_dir.join("buildsmith.toml");
        if target.exists() {
            anyhow::bail!("buildsmith.toml already exists in this directory");
        }

        let template = if base_dir.join("Cargo.toml").exists() {
            r#"project = "<name>"

[tasks.check]
command = "cargo check 2>&1"
inputs = ["src", "Cargo.toml"]
description = "Type-check the project"

[tasks.clippy]
command = "cargo clippy -- -D warnings 2>&1"
inputs = ["src", "Cargo.toml"]
description = "Run clippy lints"

[tasks.fmt]
command = "cargo fmt -- --check 2>&1"
inputs = ["src"]
description = "Check code formatting"

[tasks.build]
command = "cargo build 2>&1"
inputs = ["src", "Cargo.toml"]
outputs = ["target/debug/<name>"]
deps = ["check"]
description = "Build the project"

[tasks.test]
command = "cargo test 2>&1"
inputs = ["src", "Cargo.toml"]
deps = ["build"]
description = "Run the test suite"

[tasks.ci]
command = "echo 'CI pipeline complete'"
deps = ["fmt", "clippy", "build", "test"]
description = "Run full CI pipeline"
"#
        } else if base_dir.join("package.json").exists() {
            r#"project = "<name>"

[tasks.lint]
command = "npm run lint 2>&1"
inputs = ["src", "package.json"]
description = "Run linter"

[tasks.build]
command = "npm run build 2>&1"
inputs = ["src", "package.json"]
outputs = ["dist"]
description = "Build the project"

[tasks.test]
command = "npm test 2>&1"
inputs = ["src", "package.json"]
deps = ["build"]
description = "Run tests"

[tasks.ci]
command = "echo 'CI pipeline complete'"
deps = ["lint", "build", "test"]
description = "Run full CI pipeline"
"#
        } else {
            r#"project = "<name>"

[tasks.build]
command = "echo 'Building...' && make build"
inputs = ["src"]
outputs = ["dist"]
description = "Build the project"

[tasks.test]
command = "make test"
inputs = ["src"]
deps = ["build"]
description = "Run tests"

[tasks.ci]
command = "echo 'CI pipeline complete'"
deps = ["build", "test"]
description = "Run full CI pipeline"
"#
        };

        std::fs::write(&target, template)?;
        println!("{} Created buildsmith.toml", "[OK]".green());
        println!("\nNext steps:");
        println!("  1. Edit buildsmith.toml to customize tasks");
        println!(
            "  2. Run {} to see available tasks",
            "buildsmith list".cyan()
        );
        println!("  3. Run {} to build", "buildsmith build".cyan());
        return Ok(());
    }

    let config = parse_config(&config_path)?;

    match cli.command {
        Commands::Build {
            task,
            dry_run,
            keep_going,
            timeline,
            jobs,
            force,
        } => {
            let runner = BuildRunner::new(config.clone(), base_dir)?;
            let results = if let Some(t) = task {
                runner
                    .run_task_with_flags(&t, dry_run, cli.json, keep_going, jobs, force)
                    .await?
            } else {
                runner
                    .run_all_with_flags(dry_run, cli.json, keep_going, jobs, force)
                    .await?
            };

            if cli.json {
                let json_output = serde_json::json!({
                    "results": results,
                    "summary": {
                        "built": results.iter().filter(|r| !r.cached && !r.dry_run && r.success).count(),
                        "cached": results.iter().filter(|r| r.cached).count(),
                        "dry": results.iter().filter(|r| r.dry_run).count(),
                        "failed": results.iter().filter(|r| !r.success).count(),
                    }
                });
                println!("{}", serde_json::to_string_pretty(&json_output)?);
            } else {
                let mut all_success = true;
                for result in &results {
                    let status = if result.dry_run {
                        "[DRY]".yellow()
                    } else if result.cached {
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
                let dry = results.iter().filter(|r| r.dry_run).count();
                let built = results
                    .iter()
                    .filter(|r| !r.cached && !r.dry_run && r.success)
                    .count();
                let failed = results.iter().filter(|r| !r.success).count();

                println!(
                    "\n{}: {} built, {} cached, {} dry, {} failed",
                    "Summary".bold(),
                    built.to_string().green(),
                    cached.to_string().cyan(),
                    dry.to_string().yellow(),
                    failed.to_string().red(),
                );

                if !all_success {
                    std::process::exit(1);
                }

                if timeline {
                    print_timeline(&results);
                }
            }
        }

        Commands::Graph { format } => {
            let graph = dag::BuildGraph::from_config(&config)?;
            match format.as_str() {
                "dot" => {
                    println!("digraph build {{");
                    for task in graph.all_tasks() {
                        for dep in graph.dependencies(&task) {
                            println!("  \"{}\" -> \"{}\";", dep, task);
                        }
                    }
                    println!("}}");
                }
                _ => {
                    let order = graph.topological_order();
                    for (i, task) in order.iter().enumerate() {
                        let deps = graph.dependencies(task);
                        let dep_str = if deps.is_empty() {
                            String::from("(no deps)")
                        } else {
                            deps.join(", ")
                        };
                        println!("{}. {} <- [{}]", i + 1, task, dep_str);
                    }
                }
            }
        }

        Commands::Clean => {
            let cache_dir = base_dir.join(&config.cache_dir);
            let cache = cache::Cache::new(&cache_dir)?;
            cache.clear()?;
            println!("{} Cache cleared", "[OK]".green());
        }

        Commands::Cache { action } => {
            let cache_dir = base_dir.join(&config.cache_dir);
            let cache = cache::Cache::new(&cache_dir)?;
            match action {
                CacheAction::Stats => {
                    let size = cache.size()?;
                    let count = cache.entry_count()?;
                    println!("Cache directory: {}", cache_dir.display());
                    println!("Cache entries: {}", count);
                    println!("Cache size: {:.2} MB", size as f64 / 1_048_576.0);
                }
                CacheAction::Clear => {
                    cache.clear()?;
                    println!("{} Cache cleared", "[OK]".green());
                }
                CacheAction::Prune => {
                    let runner = BuildRunner::new(config.clone(), base_dir.clone())?;
                    let keep_hashes = runner.current_hashes().await?;
                    let before_count = cache.entry_count()?;
                    let before_size = cache.size()?;
                    let (removed, freed) = cache.prune(&keep_hashes)?;
                    let after_count = before_count - removed;
                    let after_size = before_size - freed;
                    println!("{} Pruned {} stale entries", "[OK]".green(), removed);
                    println!("   Freed {:.2} MB", freed as f64 / 1_048_576.0);
                    println!(
                        "   Remaining: {} entries, {:.2} MB",
                        after_count,
                        after_size as f64 / 1_048_576.0
                    );
                }
            }
        }

        Commands::List => {
            let mut tasks: Vec<_> = config.tasks.iter().collect();
            tasks.sort_by_key(|(name, _)| name.to_lowercase());
            for (name, task) in tasks {
                let desc = task.description.clone().unwrap_or_default();
                let dep_str = if task.deps.is_empty() {
                    String::new()
                } else {
                    format!(" (deps: {})", task.deps.join(", "))
                };
                println!("  {:<20} {}{}", name.green(), desc, dep_str.dimmed());
            }
        }

        Commands::Init => {
            unreachable!("Init is handled before config parsing");
        }

        Commands::Watch { task } => {
            watch::watch(config, base_dir, task).await?;
        }

        Commands::Status { task } => {
            let runner = BuildRunner::new(config.clone(), base_dir)?;
            let statuses = runner.check_status(task.as_deref()).await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&statuses)?);
            } else {
                let cached_count = statuses.iter().filter(|s| s.cached).count();
                let stale_count = statuses.iter().filter(|s| !s.cached).count();

                for status in &statuses {
                    let state = if status.cached {
                        "[CACHED]".cyan()
                    } else {
                        "[STALE]".yellow()
                    };
                    let dep_str = if status.deps.is_empty() {
                        String::new()
                    } else {
                        format!(" <- {}", status.deps.join(", "))
                    };
                    println!(
                        "{} {:<20} {} files, {}{}",
                        state,
                        status.name.green(),
                        status.input_count,
                        status.hash[..12].dimmed(),
                        dep_str.dimmed()
                    );
                }

                println!(
                    "\n{}: {} cached, {} would rebuild",
                    "Status".bold(),
                    cached_count.to_string().cyan(),
                    stale_count.to_string().yellow(),
                );
            }
        }

        Commands::Why { task } => {
            let runner = BuildRunner::new(config.clone(), base_dir)?;
            let why = runner.why_stale(&task).await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&why)?);
            } else {
                println!("Task: {}", why.task.green());
                println!("Hash: {}", why.hash[..12].dimmed());

                if why.cached && why.changed_files.is_empty() && why.removed_files.is_empty() {
                    println!("Status: {}", "CACHED (up to date)".cyan());
                } else if why.cached {
                    println!("Status: {}", "STALE".yellow());
                } else {
                    println!("Status: {}", "NOT CACHED".yellow());
                }

                if !why.changed_files.is_empty() {
                    println!("\n  Changed files:");
                    for f in &why.changed_files {
                        println!("    {} {}", "~".yellow(), f);
                    }
                }
                if !why.removed_files.is_empty() {
                    println!("\n  Removed files:");
                    for f in &why.removed_files {
                        println!("    {} {}", "-".red(), f);
                    }
                }
                if !why.cached && why.changed_files.is_empty() && why.removed_files.is_empty() {
                    println!("\n  No previous cache entry found for this task.");
                }

                if !why.dep_stale.is_empty() {
                    println!("\n  Stale dependencies:");
                    for d in &why.dep_stale {
                        println!("    {} {}", "!".red(), d);
                    }
                }

                if why.deps.is_empty() && why.dep_stale.is_empty() {
                    println!("\n  No dependencies.");
                }
            }
        }
    }

    Ok(())
}

fn print_timeline(results: &[engine::runner::TaskResult]) {
    use colored::Colorize;

    let max_end = results.iter().map(|r| r.end_ms).max().unwrap_or(0);
    if max_end == 0 {
        return;
    }

    let width = 40usize;
    let scale = max_end as f64 / width as f64;

    println!("\n{}", "Build Timeline".bold());
    println!("{}ms", "0".dimmed());

    for result in results {
        let start_col = (result.start_ms as f64 / scale) as usize;
        let end_col = (result.end_ms as f64 / scale) as usize;
        let bar_len = end_col.saturating_sub(start_col).max(1);

        let bar: String = "\u{2588}".repeat(bar_len);
        let padding = " ".repeat(start_col);

        let status = if result.cached {
            " ".to_string()
        } else if result.success {
            bar.green().to_string()
        } else {
            bar.red().to_string()
        };

        println!(
            "  {:<12} {}{} {:>5}ms",
            result.name.cyan(),
            padding,
            status,
            result.duration_ms,
        );
    }

    // Time axis
    let axis = "\u{2500}".repeat(width);
    println!("  {:<12} {}", "", axis.dimmed());
    println!(
        "  {:<12} {:<width$} {}ms",
        "",
        "",
        max_end,
        width = width - format!("{}", max_end).len()
    );
}
