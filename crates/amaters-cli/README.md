# amaters-cli

Command-line interface for AmateRS

**Status:** Alpha | **Version:** 0.2.0 | **License:** Apache-2.0 | **Tests:** 243 | **Public items:** 87

## Overview

`amaters-cli` provides a full-featured command-line tool for administering and interacting with AmateRS servers. It ships with a persistent REPL, shell completion generation for five shells, flexible config management, and colorized table/JSON/YAML output.

## Features

- **CLI** built with clap derive — full help text, typed arguments, subcommand tree
- **REPL**: history persistence, multi-line editing, bang-expansion (`!cmd`), colorized output, per-session statistics
- **Admin commands**: backup, restore, compaction, integrity verify, stats, live log tailing
- **Shell completion**: Bash, Zsh, Fish, PowerShell, Elvish
- **Config management**: `init`, `validate`, `show`, `set <key> <value>`, `get <key>`
- **Output formats**: JSON, YAML, table (comfy-table with Unicode box-drawing)
- **Progress bars** for long-running operations
- **Batch operations**: `amaters-cli batch <file>` / `amaters-cli batch -` (stdin); `BatchCommand` with streaming `BufReader` (never buffers all); line-by-line `put`/`delete` op parsing; JSON mode for scripts
- **Pagination flags**: `--limit <n>`, `--offset <n>`, `--cursor <token>` on `range`/`scan`/`query` subcommands; next cursor displayed in output
- **Watch mode**: `amaters-cli watch <interval_secs> <command...>` re-executes a command on a tokio interval; ANSI screen clear between runs; Ctrl-C exits cleanly (no crossterm dependency)

## Installation

```bash
# Build from source
cargo build --release --bin amaters-cli

# Or install directly
cargo install --path crates/amaters-cli
```

## Quick Start

```bash
# Initialize configuration
amaters-cli config init

# Start the REPL
amaters-cli repl

# Or run individual commands
amaters-cli set my_key "hello world"
amaters-cli get my_key
amaters-cli delete my_key
```

## Commands

### Data Operations

```bash
amaters-cli set <key> <value>
amaters-cli get <key>
amaters-cli delete <key>
amaters-cli range <start-key> <end-key>
```

### REPL

```bash
# Start interactive REPL with history and multi-line editing
amaters-cli repl

# Inside the REPL:
# - Arrow keys for history navigation
# - Ctrl-D or "exit" to quit
# - "!" prefix to run a shell command (bang expansion)
# - Multi-line input with trailing backslash
# - Session stats shown on exit
```

### Admin Commands

```bash
# Backup
amaters-cli admin backup <destination-dir>
amaters-cli admin backup <destination-dir> --incremental

# Restore
amaters-cli admin restore <source-dir>

# Compaction
amaters-cli admin compact
amaters-cli admin compact --collection <name>

# Statistics
amaters-cli admin stats

# Integrity verification
amaters-cli admin verify

# Server logs
amaters-cli admin logs
amaters-cli admin logs -n 500
amaters-cli admin logs --follow
```

### Shell Completion

```bash
# Generate and install completions
amaters-cli completion bash   >> ~/.bash_completion.d/amaters-cli
amaters-cli completion zsh    > ~/.zfunc/_amaters-cli
amaters-cli completion fish   > ~/.config/fish/completions/amaters-cli.fish
amaters-cli completion powershell >> $PROFILE
amaters-cli completion elvish >> ~/.elvish/rc.elv
```

### Config Management

```bash
# Create default config file
amaters-cli config init

# Validate current config file
amaters-cli config validate

# Show all configuration values
amaters-cli config show

# Set a single key
amaters-cli config set server_url http://prod.example.com:50051

# Get a single key
amaters-cli config get server_url
```

### Server Management

```bash
amaters-cli server status
amaters-cli server health
amaters-cli server metrics
amaters-cli server cluster
amaters-cli server nodes
```

### Key Management

```bash
amaters-cli key generate <key-name> --description "Production key"
amaters-cli key import <key-name> --file <path>
amaters-cli key export <key-name> --file <output-path>
amaters-cli key list
amaters-cli key delete <key-name>
```

## Configuration

The configuration file lives at `~/.amaters/config.toml` and is created by `amaters-cli config init`:

```toml
server_url = "http://localhost:50051"
default_collection = "default"
output_format = "table"
color = true

[tls]
enabled = false
# ca_cert = "/path/to/ca.pem"
# client_cert = "/path/to/client.pem"
# client_key = "/path/to/client-key.pem"
```

### CLI flags (override config)

```bash
amaters-cli -s <server-url> -c <collection> -f <format> <command>
```

| Flag | Description |
|------|-------------|
| `-s, --server <URL>` | Server URL |
| `-c, --collection <NAME>` | Collection name |
| `-f, --format <FORMAT>` | Output format: `json`, `yaml`, `table` |

### Environment variables (override config file)

```bash
export AMATERS_SERVER_URL="http://localhost:50051"
export AMATERS_COLLECTION="default"
export AMATERS_OUTPUT_FORMAT="json"
export AMATERS_COLOR="true"
```

## Output Formats

### Table (default)

```
┌────────────┬──────────────────────┐
│ Property   │ Value                │
├────────────┼──────────────────────┤
│ Key        │ my_key               │
│ Size       │ 1024 bytes           │
│ Checksum   │ 0xa1b2c3d4           │
│ Created At │ 2026-03-28T12:00:00Z │
└────────────┴──────────────────────┘
```

### JSON

```json
{
  "status": "success",
  "operation": "get",
  "key": "my_key",
  "value": {
    "size": 1024,
    "checksum": 2712847316,
    "created_at": "2026-03-28T12:00:00Z"
  }
}
```

### YAML

```yaml
status: success
operation: get
key: my_key
value:
  size: 1024
  checksum: 2712847316
  created_at: '2026-03-28T12:00:00Z'
```

## Examples

### REPL session

```
$ amaters-cli repl
amaters> set user:1 "Alice"
✓ Set user:1
amaters> get user:1
┌──────┬───────┐
│ Key  │ user:1│
│ Value│ Alice │
└──────┴───────┘
amaters> !date
Sat Mar 28 12:00:00 UTC 2026
amaters> exit
Session: 3 commands, 0 errors
```

### Admin backup and restore

```bash
amaters-cli admin backup /backups/db_$(date +%Y%m%d)
# Shows backup metadata (ID, size, key count, duration)

amaters-cli admin restore /backups/db_20260328
# Shows restore result (keys restored, duration)
```

### Install Zsh completion

```bash
amaters-cli completion zsh > ~/.zfunc/_amaters-cli
echo 'fpath=(~/.zfunc $fpath)' >> ~/.zshrc
echo 'autoload -U compinit && compinit' >> ~/.zshrc
```

## License

Licensed under Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
