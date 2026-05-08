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
- [x] Default key selection (`key default`) (implemented 2026-05-07)
  - **Goal:** Persistent default-key config. `key default <name>` (set), `key default --clear` (unset), `key default --show` (display). Dispatcher falls back to default when no `--key` flag present.
  - **Design:** Add `default_key: Option<String>` (serde-default) to `Config`. Extend `Key` clap subcommand with `Default { name: Option<String>, clear: bool, show: bool }`. Atomic config write (temp file + rename). Encryption-using commands fall back to `config.default_key`; if both None, error helpfully.
  - **Files:** `crates/amaters-cli/src/config.rs`, `crates/amaters-cli/src/main.rs`, `crates/amaters-cli/src/keys.rs`
  - **Tests:** `test_default_key_set_persists_to_config`, `test_default_key_clear_removes_setting`, `test_default_key_show_displays_current`, `test_default_key_used_when_flag_absent`, `test_explicit_flag_overrides_default`, `test_no_default_no_flag_errors_helpfully`, `test_default_key_atomic_write_no_partial_file`
  - **Risk:** Atomic-rename mitigates concurrent writes; documented limitation.

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
- [x] Pipe-friendly output (`-f nul`, `-f ndjson`) (implemented 2026-05-07)
  - **Goal:** NUL-delimited and NDJSON output formats for shell-pipeline integration. Apply to `range`, `scan`, `query`, `batch`, `key list`.
  - **Design:** Extend `OutputFormat` enum with `Nul` and `Ndjson` variants. `Nul`: `<key>\0<value>\0...`, base64-wrap binary values with `BASE64:<...>` sentinel. `Ndjson`: `{"key":"...","value":"..."}\n` per record, base64 for binary. Streaming-write per record (no buffering).
  - **Files:** `crates/amaters-cli/src/output.rs`, `crates/amaters-cli/src/main.rs`
  - **Tests:** `test_output_nul_text_record`, `test_output_nul_binary_value_base64_wrapped`, `test_output_ndjson_record_per_line`, `test_output_ndjson_escapes_quotes`, `test_output_ndjson_binary_value_base64_wrapped`, `test_format_flag_accepts_nul_and_ndjson`, `test_range_emits_ndjson_streaming`
  - **Risk:** Binary NUL bytes handled by base64 sentinel; documented in `--help`.
- [x] `watch` mode (re-execute a command on interval) (implemented 2026-04-17)
  - **Goal:** `watch <interval_secs> <command...>` subcommand; re-executes command on tokio interval; clears screen between runs.
  - **Design:** `tokio::time::interval(Duration::from_secs(n))`; `watch_loop` helper calls existing command dispatch via `execute_command`; ANSI `\x1b[2J\x1b[H` for screen clear; Ctrl-C exits cleanly.
  - **Files:** `crates/amaters-cli/src/main.rs`
  - **Tests:** `test_watch_executes_at_least_twice`, `test_watch_cli_parsing`
  - **Refinement (2026-04-17):** dropped crossterm dep in favor of ANSI escape sequences (\x1b[2J\x1b[H); portable, no new workspace dep.
- [x] `diff` command (keys & snapshots) (implemented 2026-05-07)
  - **Goal:** New `diff` subcommand to compare two keys (live from server) or two snapshot files on disk. Output formats: unified-diff text (default), JSON diff, summary stats.
  - **Design:** New `src/diff.rs`. `Diff { Keys { a, b, key_a, key_b }, Snapshots { a: PathBuf, b: PathBuf } }`. Keys mode: `client.get()` x2, decrypt if keys provided, diff bytes. Snapshots mode: parse NDJSON/array, diff record-by-record. Use `similar` crate for unified diff. JSON diff: serde_json Value walk → `{added, removed, modified}`. Add `similar` to workspace Cargo.toml and consume via `similar = { workspace = true }`.
  - **Files:** `crates/amaters-cli/src/diff.rs` (new), `crates/amaters-cli/src/main.rs`, `crates/amaters-cli/Cargo.toml`, workspace `Cargo.toml`
  - **Tests:** `test_diff_two_identical_keys_no_changes`, `test_diff_two_different_keys_unified_format`, `test_diff_two_keys_json_format`, `test_diff_two_keys_stats_format`, `test_diff_two_snapshots_added_removed_modified`, `test_diff_handles_missing_key_a`, `test_diff_handles_missing_key_b`, `test_diff_help_text_documents_ciphertext_caveat`
  - **Risk:** Refuse to diff FHE ciphertexts (CipherBlob magic header heuristic); documented in `--help`.
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
