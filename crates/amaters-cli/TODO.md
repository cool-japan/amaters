# amaters-cli TODO

## Status Summary (v0.2.0)

| Phase | Title | Status |
|-------|-------|--------|
| 1 | Basic commands (set/get/delete) | ✅ COMPLETE |
| 2 | Query commands (range, filter) | ✅ COMPLETE |
| 3 | Key management | ✅ COMPLETE |
| 4 | Server management | ✅ COMPLETE |
| 5 | Administration (backup/restore/compact) | ✅ COMPLETE |
| 6 | Output formatting | ✅ COMPLETE |
| 7 | REPL + shell completion | ✅ COMPLETE |
| 8 | Batch / piping / watch / diff | 📋 Future |

**Tests:** 243 | **Public items:** 87

---

## Phase 1: Basic Commands ✅

- [x] clap derive CLI skeleton
- [x] `set` command
- [x] `get` command
- [x] `delete` command
- [x] Configuration loading (`~/.amaters/config.toml`)
- [x] Clear error messages and help text

## Phase 2: Query Commands ✅

- [x] `range <start> <end>` query
- [x] Filter query API (ready; full SDK implementation deferred)
- [x] Result formatting (JSON / YAML / table)
- [ ] AQL filter parser (partial; requires SDK support)
- [x] Pagination support (implemented 2026-04-17)
  - **Goal:** `--limit <n>`, `--offset <n>`, `--cursor <token>` flags on scan/query commands.
  - **Design:** clap argument additions to `range`/`scan`/`query` subcommands; pass through to SDK `PaginatedQueryBuilder::limit()`, `.offset()`, `.cursor()`; display next cursor in output when present.
  - **Files:** `crates/amaters-cli/src/main.rs`, `crates/amaters-cli/src/output.rs`, `crates/amaters-sdk-rust/src/client.rs`
  - **Tests:** `test_pagination_limit_respected`, `test_pagination_cursor_output_format`, `test_pagination_offset_and_cursor`, `test_scan_pagination_flags`
  - Added `scan` subcommand for prefix-based paginated queries.

## Phase 3: Key Management ✅

- [x] `key generate <name> [--description]`
- [x] `key import <name> --file <path>`
- [x] `key export <name> --file <path>`
- [x] `key list`
- [x] `key delete <name>`
- [ ] Default key selection

## Phase 4: Server Management ✅

- [x] `server status`
- [x] `server health`
- [x] `server metrics`
- [x] `server cluster`
- [x] `server nodes`

## Phase 5: Administration ✅

- [x] `admin backup <dir> [--incremental]`
- [x] `admin restore <dir>`
- [x] `admin compact [--collection <name>]`
- [x] `admin stats`
- [x] `admin verify`
- [x] `admin logs [-n <lines>] [--follow]`

## Phase 6: Output Formatting ✅

- [x] JSON output (`-f json`)
- [x] YAML output (`-f yaml`)
- [x] Table output with Unicode box-drawing (`-f table`, default)
- [x] Color support (auto-detected, overridable via `AMATERS_COLOR`)
- [x] Progress bars for long-running operations

## Phase 7: REPL + Shell Completion ✅

- [x] REPL (`amaters-cli repl`)
- [x] History persistence across sessions
- [x] Multi-line input (trailing `\`)
- [x] Bang expansion (`!<shell-cmd>`)
- [x] Colorized REPL output
- [x] Per-session statistics on exit
- [x] Shell completion — Bash
- [x] Shell completion — Zsh
- [x] Shell completion — Fish
- [x] Shell completion — PowerShell
- [x] Shell completion — Elvish

## Phase 8: Config Management ✅

- [x] `config init`
- [x] `config validate`
- [x] `config show`
- [x] `config set <key> <value>`
- [x] `config get <key>`

## Phase 9: Advanced Features 📋

- [x] Batch operations (read from file / stdin) (implemented 2026-04-17)
  - **Goal:** `amaters batch <file>` reads newline-delimited op records from file; `amaters batch -` reads from stdin.
  - **Design:** New `batch.rs` module; `BatchCommand` streams lines one-by-one (never loads all into memory), parses `<op> <key> [value]` format; dispatches via SDK client; reports success/failure/skip per line.
  - **Files:** `crates/amaters-cli/src/batch.rs` (new), `crates/amaters-cli/src/main.rs`
  - **Tests:** `test_batch_stats_counters_from_parse`, `test_batch_skips_malformed_lines_with_error`, `test_batch_large_input_parse_streaming`, `test_parse_put`, `test_parse_delete`, etc.
- [ ] Pipe-friendly output (NUL-delimited / streaming JSON)
- [x] `watch` mode (re-execute a command on interval) (implemented 2026-04-17)
  - **Goal:** `watch <interval_secs> <command...>` subcommand; re-executes command on tokio interval; clears screen between runs.
  - **Design:** `tokio::time::interval(Duration::from_secs(n))`; `watch_loop` helper calls existing command dispatch via `execute_command`; ANSI `\x1b[2J\x1b[H` for screen clear; Ctrl-C exits cleanly.
  - **Files:** `crates/amaters-cli/src/main.rs`
  - **Tests:** `test_watch_executes_at_least_twice`, `test_watch_cli_parsing`
  - **Refinement (2026-04-17):** dropped crossterm dep in favor of ANSI escape sequences (\x1b[2J\x1b[H); portable, no new workspace dep.
- [ ] `diff` command (compare two keys or snapshots)
- [ ] Full AQL filter query parsing (requires SDK)
- [x] Pagination flags (`--limit`, `--offset`, `--cursor`) — covered by Pagination support above

## Dependencies

- `amaters-core` — core types
- `amaters-sdk-rust` — client SDK
- `clap` — CLI parsing (derive API)
- `tokio` — async runtime
- `serde_json` / `serde_yaml` — output serialization
- `comfy-table` — table formatting

## Notes

- Error messages must be human-readable and actionable
- Non-interactive mode (no TTY) must suppress REPL chrome
- All commands should be scriptable (stable exit codes, machine-readable output via `-f json`)
