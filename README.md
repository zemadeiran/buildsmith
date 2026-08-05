# forge

A content-hashed, DAG-based build system written in Rust.

[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange)](https://www.rust-lang.org/)
[![crates.io](https://img.shields.io/crates/v/buildsmith)](https://crates.io/crates/buildsmith)

## Why?

Most build systems are either fast but incorrect (Make), correct but complex (Bazel), or simple but slow. `forge` aims for all three:

- **Content hashing** — files are hashed by content, not timestamps. No more false rebuilds from `touch`.
- **DAG-based parallelism** — tasks declare dependencies, `forge` schedules independent tasks in parallel.
- **Incremental builds** — only rebuild what actually changed, transitively.
- **Content-addressed cache** — task outputs are cached by their input hash. Same inputs → same outputs, instantly restored.
- **Simple TOML config** — no DSL, no scripting language, just declarative task definitions.

## Installation

```bash
cargo install buildsmith
```

Or build from source:

```bash
git clone https://github.com/zemadeiran/buildsmith.git
cd buildsmith
cargo install --path .
```

## Quick Start

Create a `forge.toml` in your project:

```toml
project = "myapp"

[tasks.compile]
command = "gcc -c main.c -o main.o"
inputs = ["main.c"]
outputs = ["main.o"]
description = "Compile source files"

[tasks.link]
command = "gcc main.o -o myapp"
inputs = ["main.o"]
outputs = ["myapp"]
deps = ["compile"]
description = "Link binary"

[tasks.test]
command = "./myapp --test"
inputs = ["myapp"]
deps = ["link"]
description = "Run tests"
```

Then run:

```bash
$ forge build test
[OK] compile    120ms - Compile source files
[OK] link        45ms - Link binary
[OK] test         8ms - Run tests

Summary: 3 built, 0 cached, 0 dry, 0 failed
```

Run it again — nothing rebuilds:

```bash
$ forge build test
[CACHED] compile     2ms - Compile source files
[CACHED] link        1ms - Link binary
[CACHED] test        1ms - Run tests

Summary: 0 built, 3 cached, 0 dry, 0 failed
```

Touch a file — only affected tasks rebuild:

```bash
$ touch main.c
$ forge build test
[OK] compile    118ms - Compile source files
[OK] link        44ms - Link binary
[OK] test         8ms - Run tests

Summary: 3 built, 0 cached, 0 dry, 0 failed
```

## Commands

### Building

| Command | Description |
|---------|-------------|
| `forge build` | Run all tasks |
| `forge build <task>` | Run a specific task and its dependencies |
| `forge build --dry-run` | Show what would be built without executing |
| `forge build --keep-going` | Continue building independent tasks after a failure |
| `forge build --force` | Ignore cache and force rebuild all tasks |
| `forge build -j N` / `--jobs N` | Limit concurrent task execution to N |
| `forge build --timeline` | Show a Gantt-like build timeline after completion |
| `forge build --json` | Output results as JSON (for tooling integration) |

### Inspection

| Command | Description |
|---------|-------------|
| `forge status` | Show which tasks are cached and which would rebuild |
| `forge status <task>` | Check a specific task |
| `forge why <task>` | Explain why a task is stale — which input files changed |
| `forge list` | List all defined tasks |
| `forge graph` | Show the dependency graph as text |
| `forge graph -f dot` | Output graph in Graphviz DOT format |

### Cache Management

| Command | Description |
|---------|-------------|
| `forge cache stats` | Show cache directory, entry count, and size |
| `forge cache prune` | Remove stale cache entries not referenced by current tasks |
| `forge cache clear` | Clear the entire cache (alias: `forge clean`) |

### Other

| Command | Description |
|---------|-------------|
| `forge init` | Create a new `forge.toml` (detects Rust, Node, or generic projects) |
| `forge watch` | Watch for file changes and rebuild automatically |
| `forge watch <task>` | Watch and rebuild a specific task on change |

## Output Streaming

When running tasks, `forge` streams stdout and stderr to the terminal in real-time, prefixed with the task name. Each task gets a distinct color for easy visual parsing, even when multiple tasks run in parallel:

```
     check │     Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s
    clippy │     Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.05s
      test │ running 3 tests
      test │ test result: ok. 3 passed; 0 failed
```

## Build Timeline

Pass `--timeline` to see a Gantt-like visualization of task execution after the build:

```
Build Timeline
0ms
  check        ███████████████████████████████  2766ms
  clippy                                        ████   311ms
  fmt                                         ██   212ms
  build                                             ██   236ms
  test                                                      6ms
  ci                                                         0ms
               ────────────────────────────────────────
                                                    3540ms
```

## `forge why`

When a task is stale, `forge why <task>` tells you exactly which input files changed:

```
$ forge why check
Task: check
Hash: b32a59333389
Status: NOT CACHED

  Changed files:
    ~ src/config/task.rs

  No dependencies.
```

For tasks with dependencies, it also reports which deps are stale:

```
$ forge why build
Task: build
Hash: 09cb77143473
Status: NOT CACHED

  Changed files:
    ~ src/config/task.rs

  Stale dependencies:
    ! check
```

## How It Works

1. **Parse** the `forge.toml` config into a task graph
2. **Validate** the DAG (detect cycles, missing dependencies)
3. **Schedule** tasks using dependency-driven parallelism — tasks start as soon as their deps complete, limited by `--jobs` if specified
4. **Hash** each task: `SHA256(command + input_hashes + dependency_hashes)`
5. **Check cache** — if the hash exists, restore outputs and skip execution
6. **Execute** the command via `/bin/sh -c`
7. **Cache** outputs and a manifest of input file hashes, keyed by the task hash

## Configuration Reference

| Field | Required | Description |
|-------|----------|-------------|
| `project` | yes | Project name |
| `cache_dir` | no | Cache directory (default: `.forge`) |
| `command` | yes | Shell command to execute |
| `inputs` | no | Glob patterns for input files (hashed for cache key) |
| `outputs` | no | Glob patterns for output files (cached and restored) |
| `deps` | no | List of task names this task depends on |
| `description` | no | Human-readable description shown in output |

## Project Structure

```
src/
├── main.rs           # CLI entry point (clap)
├── watch.rs          # File watching with debounce and auto-rebuild
├── config/
│   ├── mod.rs        # Module exports
│   ├── task.rs       # BuildConfig, TaskConfig structs
│   └── parser.rs     # TOML parsing + validation
├── dag/
│   ├── mod.rs        # Module exports
│   ├── graph.rs      # BuildGraph (petgraph DAG, cycle detection)
│   └── schedule.rs   # Schedule (topological level scheduling)
├── engine/
│   ├── mod.rs        # Module exports
│   ├── hashing.rs    # Content hashing (SHA-256, glob expansion)
│   └── runner.rs     # BuildRunner (parallel execution, caching, dry-run)
└── cache/
    └── mod.rs        # Content-addressed cache (store/restore outputs, manifests)
```

## License

[MIT](LICENSE)
