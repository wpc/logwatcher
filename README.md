# Logwatcher

A Rust TUI app that tails the N most recently changed files in a directory, displaying each in its own terminal panel. Designed for monitoring ephemeral task outputs (e.g., Claude Code subagent logs).

## Features

- Dynamic split-panel layout that adapts to the number of active files
- Auto-detects new, modified, and deleted files via filesystem watcher
- LRU eviction when panel limit is reached (deleted files evicted first)
- Process detection via `lsof`/`/proc` — shows which process is writing each file
- Optional LLM-powered process summaries (OpenAI-compatible API)
- Vim-style keybindings with per-panel scrolling
- Glob filtering to watch only matching filenames

## Install

```bash
cargo build --release
# Binary at target/release/logwatcher
```

## Usage

```bash
logwatcher /path/to/dir
```

### Options

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `<DIR>` | | `.` | Directory to watch |
| `--panels` | `-n` | `4` | Max simultaneous panels |
| `--tail-lines` | `-t` | `50` | Lines to read on file open |
| `--stale-seconds` | `-s` | `30` | Inactivity before panel recycling |
| `--glob` | `-g` | | Filename filter (e.g. `"*.log"`) |
| `--llm-api-url` | | | OpenAI-compatible endpoint for process summaries |
| `--llm-log-file` | | | Log LLM requests/responses to file |
| `--scan-back-minutes` | | `30` | How far back to scan on startup |
| `--tick-rate-ms` | | `250` | UI refresh interval |

### Examples

```bash
# Watch current directory with 6 panels, only .log files
logwatcher . -n 6 -g "*.log"

# Watch with LLM process summaries
logwatcher /tmp/tasks --llm-api-url https://your-llm/v1/chat/completions

# Debug LLM calls
logwatcher /tmp/tasks --llm-api-url https://your-llm/v1/chat/completions \
  --llm-log-file /tmp/llm-debug.log
```

## Keybindings

| Key | Action |
|-----|--------|
| `q` / `Esc` / `Ctrl+C` | Quit |
| `Tab` / `BackTab` | Cycle panels |
| `1`-`9` | Select panel by number |
| `j` / `Down` | Scroll down |
| `k` / `Up` | Scroll up |
| `PgDn` / `PgUp` | Page scroll |
| `Home` / `g` | Scroll to top |
| `End` / `G` | Scroll to bottom |
| `?` | Toggle help overlay |
