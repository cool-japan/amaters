//! Interactive REPL mode for AmateRS CLI
//!
//! Provides a read-eval-print loop for interacting with an AmateRS server
//! without needing to invoke the CLI binary for each command.
//!
//! Features:
//! - History persistence to `~/.amaters/history` (or `$XDG_DATA_HOME/amaters/history`)
//! - Multi-line command support via trailing `\` or unclosed brackets
//! - Bang expansion (`!!` for last command, `!prefix` for prefix search)
//! - Session statistics tracking
//! - Command timing with `.timing on/off`
//! - Colorized output (green for success, red for errors)

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::client::Client;
use crate::output::OutputFormat;

// ---------------------------------------------------------------------------
// ANSI color helpers
// ---------------------------------------------------------------------------

/// Wrap text in ANSI green
fn green(text: &str) -> String {
    format!("\x1b[32m{text}\x1b[0m")
}

/// Wrap text in ANSI red
fn red(text: &str) -> String {
    format!("\x1b[31m{text}\x1b[0m")
}

/// Wrap text in ANSI yellow
fn yellow(text: &str) -> String {
    format!("\x1b[33m{text}\x1b[0m")
}

/// Wrap text in ANSI cyan
fn cyan(text: &str) -> String {
    format!("\x1b[36m{text}\x1b[0m")
}

// ---------------------------------------------------------------------------
// HistoryManager
// ---------------------------------------------------------------------------

/// Manages persistent command history.
///
/// History is stored one command per line in a plain-text file.
/// Consecutive duplicate commands are suppressed, and empty lines / comments
/// (lines starting with `#`) are never recorded.
pub struct HistoryManager {
    entries: Vec<String>,
    max_size: usize,
    file_path: PathBuf,
}

impl HistoryManager {
    /// Create a new `HistoryManager` with the given maximum size and file path.
    pub fn new(max_size: usize, file_path: PathBuf) -> Self {
        Self {
            entries: Vec::new(),
            max_size,
            file_path,
        }
    }

    /// Resolve the default history file path.
    ///
    /// Uses `$XDG_DATA_HOME/amaters/history` if set, otherwise `~/.amaters/history`.
    pub fn default_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            let mut p = PathBuf::from(xdg);
            p.push("amaters");
            p.push("history");
            return p;
        }
        if let Ok(home) = std::env::var("HOME") {
            let mut p = PathBuf::from(home);
            p.push(".amaters");
            p.push("history");
            return p;
        }
        // Fallback — temp dir
        let mut p = std::env::temp_dir();
        p.push("amaters_history");
        p
    }

    /// Load history from the backing file. Missing file is not an error.
    pub fn load(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.file_path)
            .with_context(|| format!("Failed to read history file: {:?}", self.file_path))?;
        self.entries.clear();
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                self.entries.push(trimmed.to_string());
            }
        }
        // Truncate to max_size keeping the newest entries
        self.truncate();
        Ok(())
    }

    /// Persist history to the backing file.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create history directory: {parent:?}"))?;
        }
        let content = self.entries.join("\n");
        std::fs::write(&self.file_path, content)
            .with_context(|| format!("Failed to write history file: {:?}", self.file_path))?;
        Ok(())
    }

    /// Add a command to history.
    ///
    /// Empty lines, lines starting with `#`, and consecutive duplicates are
    /// silently skipped.
    pub fn add(&mut self, command: &str) {
        let trimmed = command.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return;
        }
        // Skip consecutive duplicates
        if self.entries.last().map(|s| s.as_str()) == Some(trimmed) {
            return;
        }
        self.entries.push(trimmed.to_string());
        self.truncate();
    }

    /// Search history for entries whose prefix matches `prefix`.
    ///
    /// Returns matches in reverse chronological order (newest first).
    pub fn search(&self, prefix: &str) -> Vec<&str> {
        self.entries
            .iter()
            .rev()
            .filter(|e| e.starts_with(prefix))
            .map(|e| e.as_str())
            .collect()
    }

    /// Return a slice over all history entries.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Return the most recent entry, if any.
    pub fn last(&self) -> Option<&str> {
        self.entries.last().map(|s| s.as_str())
    }

    /// Number of entries currently stored.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // Keep entries within `max_size`, discarding oldest.
    fn truncate(&mut self) {
        if self.entries.len() > self.max_size {
            let excess = self.entries.len() - self.max_size;
            self.entries.drain(..excess);
        }
    }
}

// ---------------------------------------------------------------------------
// Multi-line input helpers
// ---------------------------------------------------------------------------

/// Result of inspecting a line for continuation needs.
#[derive(Debug, PartialEq)]
pub enum LineStatus {
    /// Line is complete — ready to execute
    Complete,
    /// Line ends with `\` — explicit continuation
    BackslashContinuation,
    /// Line has unclosed brackets/parens — implicit continuation
    UnclosedBrackets,
}

/// Determine whether the accumulated input so far needs more lines.
pub fn check_line_status(input: &str) -> LineStatus {
    let trimmed = input.trim_end();

    // Explicit backslash continuation (trailing `\` not inside a string)
    if trimmed.ends_with('\\') {
        return LineStatus::BackslashContinuation;
    }

    // Count unclosed brackets / parens / braces (simplistic — ignores strings)
    let mut depth: i32 = 0;
    for ch in trimmed.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    if depth > 0 {
        return LineStatus::UnclosedBrackets;
    }

    LineStatus::Complete
}

/// Combine multi-line input into a single command string.
///
/// - Trailing `\` are stripped and the next line is appended with a space.
/// - Bracket-continuation lines are joined with a space.
pub fn combine_lines(lines: &[String]) -> String {
    let mut combined = String::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.ends_with('\\') {
            // Strip trailing backslash
            combined.push_str(trimmed.trim_end_matches('\\').trim_end());
        } else {
            combined.push_str(trimmed);
        }
        if i + 1 < lines.len() {
            combined.push(' ');
        }
    }
    combined
}

// ---------------------------------------------------------------------------
// Bang expansion
// ---------------------------------------------------------------------------

/// Result of attempting to expand a bang expression.
#[derive(Debug, PartialEq)]
pub enum BangExpansion {
    /// Not a bang expression — use the input as-is
    None,
    /// `!!` — repeat last command
    Last,
    /// `!prefix` — repeat last command matching prefix
    Prefix(String),
}

/// Parse a raw input line for bang expansion.
pub fn parse_bang(input: &str) -> BangExpansion {
    let trimmed = input.trim();
    if trimmed == "!!" {
        return BangExpansion::Last;
    }
    if let Some(prefix) = trimmed.strip_prefix('!') {
        if !prefix.is_empty() && !prefix.starts_with(' ') {
            return BangExpansion::Prefix(prefix.to_string());
        }
    }
    BangExpansion::None
}

/// Attempt to expand a bang expression using history.
///
/// Returns `Ok(Some(expanded))` if expansion succeeded,
/// `Ok(None)` if the input was not a bang expression,
/// `Err` if it was a bang expression but no match was found.
pub fn expand_bang(input: &str, history: &HistoryManager) -> Result<Option<String>> {
    match parse_bang(input) {
        BangExpansion::None => Ok(None),
        BangExpansion::Last => {
            let last = history
                .last()
                .ok_or_else(|| anyhow::anyhow!("No previous command in history"))?;
            Ok(Some(last.to_string()))
        }
        BangExpansion::Prefix(prefix) => {
            let matches = history.search(&prefix);
            let first = matches
                .first()
                .ok_or_else(|| anyhow::anyhow!("No command matching '!{prefix}' in history"))?;
            Ok(Some(first.to_string()))
        }
    }
}

// ---------------------------------------------------------------------------
// Session statistics
// ---------------------------------------------------------------------------

/// Tracks session-level statistics.
pub struct SessionStats {
    /// When the session started (UTC ISO-8601 string)
    pub started_at: String,
    /// Monotonic instant for computing elapsed time
    start_instant: Instant,
    /// Number of commands executed
    pub commands_executed: u64,
    /// Number of errors encountered
    pub errors: u64,
}

impl SessionStats {
    /// Create a new `SessionStats` anchored at the current time.
    pub fn new() -> Self {
        Self {
            started_at: Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            start_instant: Instant::now(),
            commands_executed: 0,
            errors: 0,
        }
    }

    /// Record a successful command execution.
    pub fn record_success(&mut self) {
        self.commands_executed += 1;
    }

    /// Record an error.
    pub fn record_error(&mut self) {
        self.commands_executed += 1;
        self.errors += 1;
    }

    /// Elapsed wall-clock duration since session start.
    pub fn elapsed(&self) -> std::time::Duration {
        self.start_instant.elapsed()
    }

    /// Human-readable summary.
    pub fn summary(&self) -> String {
        let elapsed = self.elapsed();
        let secs = elapsed.as_secs();
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let remaining_secs = secs % 60;
        format!(
            "Session started:    {}\n\
             Uptime:             {}h {}m {}s\n\
             Commands executed:  {}\n\
             Errors:             {}",
            self.started_at, hours, mins, remaining_secs, self.commands_executed, self.errors,
        )
    }
}

impl Default for SessionStats {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ReplConfig
// ---------------------------------------------------------------------------

/// REPL configuration
pub struct ReplConfig {
    /// The prompt string displayed before each input line
    pub prompt: String,
    /// Maximum number of history entries to retain
    pub history_size: usize,
    /// Server URL to connect to
    pub server_url: String,
    /// Default collection name
    pub default_collection: String,
    /// Output format for results
    pub output_format: OutputFormat,
}

impl Default for ReplConfig {
    fn default() -> Self {
        Self {
            prompt: "amaters> ".to_string(),
            history_size: 1000,
            server_url: "http://localhost:7878".to_string(),
            default_collection: "default".to_string(),
            output_format: OutputFormat::Table,
        }
    }
}

// ---------------------------------------------------------------------------
// ReplCommand
// ---------------------------------------------------------------------------

/// A parsed REPL command
#[derive(Debug, PartialEq)]
pub enum ReplCommand {
    /// Show help text
    Help,
    /// Exit the REPL
    Quit,
    /// Show command history
    History,
    /// Clear the terminal screen
    Clear,
    /// Check server connection status
    Status,
    /// Set a key-value pair
    Set { key: String, value: String },
    /// Get a value by key
    Get { key: String },
    /// Delete a key
    Delete { key: String },
    /// Range query from start to end
    Range { start: String, end: String },
    /// List all keys (range scan)
    Keys,
    /// Show server info
    Info,
    /// Switch active collection
    Use { collection: String },
    /// Show current collection
    Collection,
    /// Show session statistics
    Stats,
    /// Toggle command timing display
    Timing { enabled: Option<bool> },
    /// Unknown command
    Unknown { input: String },
}

impl ReplCommand {
    /// Parse a raw input line into a `ReplCommand`.
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Self::Help;
        }

        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        let cmd = tokens.first().copied().unwrap_or_default().to_lowercase();
        let arg1 = tokens.get(1).copied();

        // For the second argument, find where arg1 ends and take the rest
        let arg2 = if tokens.len() > 2 {
            let mut pos = 0;
            let after_first = trimmed[pos..].trim_start();
            pos = trimmed.len() - after_first.len();
            let first_tok = tokens.first().copied().unwrap_or_default();
            pos += first_tok.len();
            let after_second_start = trimmed[pos..].trim_start();
            pos = trimmed.len() - after_second_start.len();
            let second_tok = tokens.get(1).copied().unwrap_or_default();
            pos += second_tok.len();
            let rest = trimmed[pos..].trim();
            if rest.is_empty() { None } else { Some(rest) }
        } else {
            None
        };

        match cmd.as_str() {
            "help" | "h" | "?" | ".help" => Self::Help,
            "quit" | "exit" | "q" => Self::Quit,
            "history" | ".history" => Self::History,
            "clear" | "cls" | ".clear" => Self::Clear,
            "status" => Self::Status,
            ".stats" | "stats" => Self::Stats,
            ".timing" | "timing" => match arg1.map(|s| s.to_lowercase()).as_deref() {
                Some("on") | Some("true") | Some("1") => Self::Timing {
                    enabled: Some(true),
                },
                Some("off") | Some("false") | Some("0") => Self::Timing {
                    enabled: Some(false),
                },
                _ => Self::Timing { enabled: None },
            },
            "set" | "put" => match (arg1, arg2) {
                (Some(key), Some(value)) if !key.is_empty() && !value.is_empty() => Self::Set {
                    key: key.to_string(),
                    value: value.to_string(),
                },
                _ => Self::Unknown {
                    input: format!("set requires <key> <value>: {trimmed}"),
                },
            },
            "get" => match arg1 {
                Some(key) if !key.is_empty() => Self::Get {
                    key: key.to_string(),
                },
                _ => Self::Unknown {
                    input: format!("get requires <key>: {trimmed}"),
                },
            },
            "delete" | "del" | "rm" => match arg1 {
                Some(key) if !key.is_empty() => Self::Delete {
                    key: key.to_string(),
                },
                _ => Self::Unknown {
                    input: format!("delete requires <key>: {trimmed}"),
                },
            },
            "range" | "scan" => match (arg1, arg2) {
                (Some(start), Some(end)) if !start.is_empty() && !end.is_empty() => Self::Range {
                    start: start.to_string(),
                    end: end.to_string(),
                },
                _ => Self::Unknown {
                    input: format!("range requires <start> <end>: {trimmed}"),
                },
            },
            "keys" | "ls" => Self::Keys,
            "info" => Self::Info,
            "use" => match arg1 {
                Some(collection) if !collection.is_empty() => Self::Use {
                    collection: collection.to_string(),
                },
                _ => Self::Unknown {
                    input: format!("use requires <collection>: {trimmed}"),
                },
            },
            "collection" | "coll" => Self::Collection,
            _ => Self::Unknown {
                input: trimmed.to_string(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Repl
// ---------------------------------------------------------------------------

/// Interactive REPL for AmateRS CLI
pub struct Repl {
    config: ReplConfig,
    history: HistoryManager,
    running: bool,
    active_collection: String,
    client: Option<Client>,
    stats: SessionStats,
    timing_enabled: bool,
}

impl Repl {
    /// Create a new REPL with the given configuration.
    pub fn new(config: ReplConfig) -> Self {
        let active_collection = config.default_collection.clone();
        let history_path = HistoryManager::default_path();
        let history = HistoryManager::new(config.history_size, history_path);
        Self {
            config,
            history,
            running: true,
            active_collection,
            client: None,
            stats: SessionStats::new(),
            timing_enabled: false,
        }
    }

    /// Start the REPL loop, reading from stdin.
    pub async fn run(&mut self) -> Result<()> {
        self.print_banner();

        // Load persisted history (best-effort)
        if let Err(e) = self.history.load() {
            eprintln!(
                "{}",
                yellow(&format!("Warning: could not load history: {e}"))
            );
        }

        // Attempt initial connection
        self.try_connect().await;

        let stdin = io::stdin();
        let mut reader = stdin.lock();
        let mut multiline_buf: Vec<String> = Vec::new();

        loop {
            if multiline_buf.is_empty() {
                self.print_prompt(false)?;
            } else {
                self.print_prompt(true)?;
            }

            let mut line = String::new();
            let bytes_read = reader
                .read_line(&mut line)
                .context("Failed to read input line")?;

            // EOF (Ctrl-D)
            if bytes_read == 0 {
                println!();
                break;
            }

            let trimmed = line.trim().to_string();

            // Empty line in normal mode => skip; in multiline mode => also skip
            if trimmed.is_empty() && multiline_buf.is_empty() {
                continue;
            }

            // Accumulate for multi-line
            multiline_buf.push(trimmed.clone());
            let combined_so_far = combine_lines(&multiline_buf);
            let status = check_line_status(&combined_so_far);

            if status != LineStatus::Complete {
                // Need more input
                continue;
            }

            // We have a complete command
            let command_str = combined_so_far;
            multiline_buf.clear();

            if command_str.trim().is_empty() {
                continue;
            }

            // Attempt bang expansion
            let final_command = match expand_bang(&command_str, &self.history) {
                Ok(Some(expanded)) => {
                    println!("{}", cyan(&format!("  -> {expanded}")));
                    expanded
                }
                Ok(None) => command_str,
                Err(e) => {
                    eprintln!("{}", red(&format!("Error: {e}")));
                    self.stats.record_error();
                    continue;
                }
            };

            // Add to history (filtering happens inside HistoryManager)
            self.history.add(&final_command);

            let command = ReplCommand::parse(&final_command);

            let start = Instant::now();
            match self.execute_command(command).await {
                Ok(output) => {
                    if !output.is_empty() {
                        println!("{output}");
                    }
                    self.stats.record_success();
                }
                Err(e) => {
                    eprintln!("{}", red(&format!("Error: {e}")));
                    self.stats.record_error();
                }
            }

            if self.timing_enabled {
                let elapsed = start.elapsed();
                println!(
                    "{}",
                    yellow(&format!("  [{:.3}ms]", elapsed.as_secs_f64() * 1000.0))
                );
            }

            if !self.running {
                break;
            }
        }

        // Persist history on exit (best-effort)
        if let Err(e) = self.history.save() {
            eprintln!(
                "{}",
                yellow(&format!("Warning: could not save history: {e}"))
            );
        }

        Ok(())
    }

    /// Attempt to connect to the server, printing status.
    async fn try_connect(&mut self) {
        print!("Connecting to {} ... ", self.config.server_url);
        if io::stdout().flush().is_err() {
            // best effort flush
        }

        match Client::connect(&self.config.server_url, self.active_collection.clone()).await {
            Ok(c) => {
                self.client = Some(c);
                println!("{}", green("connected"));
            }
            Err(e) => {
                println!("{}", red(&format!("failed ({e})")));
                println!(
                    "You can still use local commands. Server commands will attempt reconnection."
                );
            }
        }
    }

    /// Ensure we have a connected client, reconnecting if necessary.
    async fn ensure_client(&mut self) -> Result<&Client> {
        if self.client.is_none() {
            let c = Client::connect(&self.config.server_url, self.active_collection.clone())
                .await
                .context("Failed to connect to AmateRS server")?;
            self.client = Some(c);
        }
        self.client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Client unavailable after reconnection attempt"))
    }

    /// Execute a parsed REPL command.
    async fn execute_command(&mut self, command: ReplCommand) -> Result<String> {
        match command {
            ReplCommand::Help => Ok(Self::help_text()),
            ReplCommand::Quit => {
                self.running = false;
                Ok(green("Goodbye!"))
            }
            ReplCommand::History => Ok(self.format_history()),
            ReplCommand::Clear => {
                print!("\x1b[2J\x1b[H");
                io::stdout().flush().context("Failed to flush stdout")?;
                Ok(String::new())
            }
            ReplCommand::Status => self.cmd_status().await,
            ReplCommand::Set { key, value } => self.cmd_set(&key, &value).await,
            ReplCommand::Get { key } => self.cmd_get(&key).await,
            ReplCommand::Delete { key } => self.cmd_delete(&key).await,
            ReplCommand::Range { start, end } => self.cmd_range(&start, &end).await,
            ReplCommand::Keys => self.cmd_keys().await,
            ReplCommand::Info => self.cmd_info().await,
            ReplCommand::Use { collection } => {
                self.active_collection = collection.clone();
                self.client = None;
                Ok(green(&format!("Switched to collection: {collection}")))
            }
            ReplCommand::Collection => {
                Ok(format!("Current collection: {}", self.active_collection))
            }
            ReplCommand::Stats => Ok(self.stats.summary()),
            ReplCommand::Timing { enabled } => {
                match enabled {
                    Some(on) => {
                        self.timing_enabled = on;
                        if on {
                            Ok(green("Timing: ON"))
                        } else {
                            Ok(green("Timing: OFF"))
                        }
                    }
                    None => {
                        // Toggle
                        self.timing_enabled = !self.timing_enabled;
                        if self.timing_enabled {
                            Ok(green("Timing: ON"))
                        } else {
                            Ok(green("Timing: OFF"))
                        }
                    }
                }
            }
            ReplCommand::Unknown { input } => Ok(format!(
                "{}\nType 'help' for available commands.",
                red(&format!("Unknown command: {input}"))
            )),
        }
    }

    // ---- Server commands ----

    async fn cmd_status(&mut self) -> Result<String> {
        match self.ensure_client().await {
            Ok(client) => match client.health_check().await {
                Ok(()) => Ok(format!(
                    "Connected to: {}\nCollection: {}\nStatus: {}",
                    self.config.server_url,
                    self.active_collection,
                    green("healthy")
                )),
                Err(e) => Ok(format!(
                    "Connected to: {}\nCollection: {}\nStatus: {}",
                    self.config.server_url,
                    self.active_collection,
                    red(&format!("unhealthy ({e})"))
                )),
            },
            Err(_) => Ok(format!(
                "Server: {}\nCollection: {}\nStatus: {}",
                self.config.server_url,
                self.active_collection,
                red("disconnected")
            )),
        }
    }

    async fn cmd_set(&mut self, key: &str, value: &str) -> Result<String> {
        use amaters_core::{CipherBlob, Key};

        self.ensure_client().await?;
        let k = Key::from_str(key);
        let v = CipherBlob::new(value.as_bytes().to_vec());

        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Client unavailable"))?;

        client
            .set(&k, &v)
            .await
            .map_err(|e| anyhow::anyhow!("Set failed: {e}"))?;

        Ok(green(&format!("OK (set '{key}')")))
    }

    async fn cmd_get(&mut self, key: &str) -> Result<String> {
        use amaters_core::Key;

        self.ensure_client().await?;
        let k = Key::from_str(key);

        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Client unavailable"))?;

        let result = client
            .get(&k)
            .await
            .map_err(|e| anyhow::anyhow!("Get failed: {e}"))?;

        match result {
            Some(blob) => {
                let raw = blob.as_bytes().to_vec();
                let display_value = String::from_utf8(raw)
                    .unwrap_or_else(|_| format!("<binary {} bytes>", blob.len()));
                // Truncate long values
                let truncated = truncate_display(&display_value, 200);
                Ok(format!("{key} = {truncated}"))
            }
            None => Ok(yellow(&format!("(nil) key '{key}' not found"))),
        }
    }

    async fn cmd_delete(&mut self, key: &str) -> Result<String> {
        use amaters_core::Key;

        self.ensure_client().await?;
        let k = Key::from_str(key);

        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Client unavailable"))?;

        client
            .delete(&k)
            .await
            .map_err(|e| anyhow::anyhow!("Delete failed: {e}"))?;

        Ok(green(&format!("OK (deleted '{key}')")))
    }

    async fn cmd_range(&mut self, start: &str, end: &str) -> Result<String> {
        use amaters_core::Key;

        self.ensure_client().await?;
        let start_key = Key::from_str(start);
        let end_key = Key::from_str(end);

        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Client unavailable"))?;

        let results = client
            .range(&start_key, &end_key)
            .await
            .map_err(|e| anyhow::anyhow!("Range query failed: {e}"))?;

        if results.is_empty() {
            return Ok(yellow("(empty result set)"));
        }

        let mut output = String::new();
        for (i, (key, blob)) in results.iter().enumerate() {
            let raw = blob.as_bytes().to_vec();
            let display_value =
                String::from_utf8(raw).unwrap_or_else(|_| format!("<binary {} bytes>", blob.len()));
            let truncated = truncate_display(&display_value, 120);
            output.push_str(&format!(
                "{}) {} = {}\n",
                i + 1,
                key.to_string_lossy(),
                truncated,
            ));
        }
        output.push_str(&green(&format!("({} results)", results.len())));
        Ok(output)
    }

    async fn cmd_keys(&mut self) -> Result<String> {
        self.cmd_range("\x00", "\x7e").await
    }

    async fn cmd_info(&mut self) -> Result<String> {
        Ok(format!(
            "Server URL:  {}\nCollection:  {}\nHistory:     {} entries\nOutput:      {:?}\nTiming:      {}",
            self.config.server_url,
            self.active_collection,
            self.history.len(),
            self.config.output_format,
            if self.timing_enabled { "ON" } else { "OFF" },
        ))
    }

    // ---- Display helpers ----

    fn print_banner(&self) {
        println!(
            "{}",
            cyan(&format!(
                "AmateRS Interactive REPL v{}",
                env!("CARGO_PKG_VERSION")
            ))
        );
        println!("Type 'help' for available commands, 'quit' to exit.");
        println!();
    }

    fn print_prompt(&self, continuation: bool) -> Result<()> {
        let prompt = if continuation {
            "... > ".to_string()
        } else if self.active_collection != "default" {
            format!("amaters({})> ", self.active_collection)
        } else {
            self.config.prompt.clone()
        };
        print!("{prompt}");
        io::stdout().flush().context("Failed to flush stdout")?;
        Ok(())
    }

    fn format_history(&self) -> String {
        let entries = self.history.entries();
        if entries.is_empty() {
            return "(no history)".to_string();
        }
        let mut out = String::new();
        for (i, entry) in entries.iter().enumerate() {
            out.push_str(&format!("  {}: {entry}\n", i + 1));
        }
        out.truncate(out.trim_end().len());
        out
    }

    fn help_text() -> String {
        [
            "Available commands:",
            "",
            "  Data operations:",
            "    set <key> <value>    Set a key-value pair",
            "    get <key>            Get a value by key",
            "    delete <key>         Delete a key (aliases: del, rm)",
            "    range <start> <end>  Range query between keys (alias: scan)",
            "    keys                 List all keys (alias: ls)",
            "",
            "  Collection:",
            "    use <collection>     Switch active collection",
            "    collection           Show current collection (alias: coll)",
            "",
            "  Server:",
            "    status               Show server connection status",
            "    info                 Show session information",
            "",
            "  Session:",
            "    history              Show command history (alias: .history)",
            "    clear                Clear the screen (aliases: cls, .clear)",
            "    .stats               Show session statistics",
            "    .timing [on|off]     Toggle command execution timing",
            "    help                 Show this help (aliases: h, ?, .help)",
            "    quit                 Exit REPL (aliases: exit, q)",
            "",
            "  History shortcuts:",
            "    !!                   Repeat last command",
            "    !<prefix>            Repeat last command matching prefix",
            "",
            "  Multi-line input:",
            "    End a line with \\ for explicit continuation",
            "    Unclosed brackets/parens trigger automatic continuation",
        ]
        .join("\n")
    }
}

/// Truncate a display string to `max_len` chars, appending `...` if truncated.
fn truncate_display(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut truncated = s[..max_len].to_string();
        truncated.push_str("...");
        truncated
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- HistoryManager tests --

    #[test]
    fn test_history_manager_add() {
        let path = std::env::temp_dir().join("amaters_test_hm_add");
        let mut hm = HistoryManager::new(100, path);
        hm.add("get foo");
        hm.add("set bar baz");
        hm.add("delete qux");
        assert_eq!(hm.len(), 3);
        assert_eq!(hm.entries()[0], "get foo");
        assert_eq!(hm.entries()[1], "set bar baz");
        assert_eq!(hm.entries()[2], "delete qux");
    }

    #[test]
    fn test_history_manager_save_load() {
        let path = std::env::temp_dir().join("amaters_test_hm_save_load");
        // Clean up any leftover
        let _ = std::fs::remove_file(&path);

        {
            let mut hm = HistoryManager::new(100, path.clone());
            hm.add("alpha");
            hm.add("beta");
            hm.add("gamma");
            hm.save().expect("save should succeed");
        }

        {
            let mut hm = HistoryManager::new(100, path.clone());
            hm.load().expect("load should succeed");
            assert_eq!(hm.len(), 3);
            assert_eq!(hm.entries()[0], "alpha");
            assert_eq!(hm.entries()[1], "beta");
            assert_eq!(hm.entries()[2], "gamma");
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_history_manager_max_size() {
        let path = std::env::temp_dir().join("amaters_test_hm_max");
        let mut hm = HistoryManager::new(3, path);
        hm.add("cmd1");
        hm.add("cmd2");
        hm.add("cmd3");
        hm.add("cmd4");
        hm.add("cmd5");
        assert_eq!(hm.len(), 3);
        // Oldest entries should be gone
        assert_eq!(hm.entries()[0], "cmd3");
        assert_eq!(hm.entries()[1], "cmd4");
        assert_eq!(hm.entries()[2], "cmd5");
    }

    #[test]
    fn test_history_manager_no_duplicates() {
        let path = std::env::temp_dir().join("amaters_test_hm_nodup");
        let mut hm = HistoryManager::new(100, path);
        hm.add("get foo");
        hm.add("get foo");
        hm.add("get foo");
        assert_eq!(hm.len(), 1);
        hm.add("set bar baz");
        hm.add("get foo"); // not consecutive duplicate anymore
        assert_eq!(hm.len(), 3);
    }

    #[test]
    fn test_history_manager_skip_empty_and_comments() {
        let path = std::env::temp_dir().join("amaters_test_hm_skip");
        let mut hm = HistoryManager::new(100, path);
        hm.add("");
        hm.add("   ");
        hm.add("# this is a comment");
        hm.add("get foo");
        assert_eq!(hm.len(), 1);
        assert_eq!(hm.entries()[0], "get foo");
    }

    #[test]
    fn test_history_search_prefix() {
        let path = std::env::temp_dir().join("amaters_test_hm_search");
        let mut hm = HistoryManager::new(100, path);
        hm.add("set alpha 1");
        hm.add("get beta");
        hm.add("set gamma 2");
        hm.add("delete alpha");
        hm.add("set delta 3");

        let matches = hm.search("set");
        assert_eq!(matches.len(), 3);
        // Newest first
        assert_eq!(matches[0], "set delta 3");
        assert_eq!(matches[1], "set gamma 2");
        assert_eq!(matches[2], "set alpha 1");

        let matches = hm.search("get");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], "get beta");

        let matches = hm.search("nonexistent");
        assert!(matches.is_empty());
    }

    // -- Multi-line detection tests --

    #[test]
    fn test_multiline_detection_backslash() {
        assert_eq!(
            check_line_status("set foo \\"),
            LineStatus::BackslashContinuation
        );
        assert_eq!(check_line_status("set foo bar"), LineStatus::Complete);
        assert_eq!(
            check_line_status("some command \\"),
            LineStatus::BackslashContinuation
        );
    }

    #[test]
    fn test_multiline_detection_brackets() {
        assert_eq!(check_line_status("set foo {"), LineStatus::UnclosedBrackets);
        assert_eq!(
            check_line_status("range (start"),
            LineStatus::UnclosedBrackets
        );
        assert_eq!(
            check_line_status("data [a, b"),
            LineStatus::UnclosedBrackets
        );
        assert_eq!(check_line_status("set foo {}"), LineStatus::Complete);
        assert_eq!(check_line_status("(a, b)"), LineStatus::Complete);
    }

    #[test]
    fn test_multiline_combine() {
        let lines = vec!["set foo \\".to_string(), "bar baz".to_string()];
        let combined = combine_lines(&lines);
        assert_eq!(combined, "set foo bar baz");

        let lines = vec![
            "set foo {".to_string(),
            "key: value".to_string(),
            "}".to_string(),
        ];
        let combined = combine_lines(&lines);
        assert_eq!(combined, "set foo { key: value }");
    }

    // -- Bang expansion tests --

    #[test]
    fn test_bang_prefix_expansion() {
        let path = std::env::temp_dir().join("amaters_test_bang_prefix");
        let mut hm = HistoryManager::new(100, path);
        hm.add("select * from users");
        hm.add("get alpha");
        hm.add("set beta 123");

        let result = expand_bang("!sel", &hm);
        assert!(result.is_ok());
        assert_eq!(
            result.expect("should succeed").expect("should expand"),
            "select * from users"
        );

        let result = expand_bang("!get", &hm);
        assert_eq!(
            result.expect("should succeed").expect("should expand"),
            "get alpha"
        );
    }

    #[test]
    fn test_double_bang() {
        let path = std::env::temp_dir().join("amaters_test_double_bang");
        let mut hm = HistoryManager::new(100, path);
        hm.add("get mykey");
        hm.add("set foo bar");

        let result = expand_bang("!!", &hm);
        assert_eq!(
            result.expect("should succeed").expect("should expand"),
            "set foo bar"
        );
    }

    #[test]
    fn test_bang_no_match() {
        let path = std::env::temp_dir().join("amaters_test_bang_nomatch");
        let hm = HistoryManager::new(100, path);

        let result = expand_bang("!nonexistent", &hm);
        assert!(result.is_err());
    }

    #[test]
    fn test_bang_not_a_bang() {
        let path = std::env::temp_dir().join("amaters_test_bang_none");
        let hm = HistoryManager::new(100, path);

        let result = expand_bang("get foo", &hm);
        assert!(result.is_ok());
        assert!(result.expect("should succeed").is_none());
    }

    // -- Session stats test --

    #[test]
    fn test_session_stats() {
        let mut stats = SessionStats::new();
        assert_eq!(stats.commands_executed, 0);
        assert_eq!(stats.errors, 0);

        stats.record_success();
        stats.record_success();
        stats.record_error();

        assert_eq!(stats.commands_executed, 3);
        assert_eq!(stats.errors, 1);

        let summary = stats.summary();
        assert!(summary.contains("Commands executed:  3"));
        assert!(summary.contains("Errors:             1"));
        assert!(summary.contains("Session started:"));
        assert!(summary.contains("Uptime:"));
    }

    // -- Timing toggle test --

    #[tokio::test]
    async fn test_timing_toggle() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);
        assert!(!repl.timing_enabled);

        let result = repl
            .execute_command(ReplCommand::Timing {
                enabled: Some(true),
            })
            .await
            .expect("timing on should succeed");
        assert!(result.contains("ON"));
        assert!(repl.timing_enabled);

        let result = repl
            .execute_command(ReplCommand::Timing {
                enabled: Some(false),
            })
            .await
            .expect("timing off should succeed");
        assert!(result.contains("OFF"));
        assert!(!repl.timing_enabled);

        // Toggle mode (None)
        let result = repl
            .execute_command(ReplCommand::Timing { enabled: None })
            .await
            .expect("timing toggle should succeed");
        assert!(result.contains("ON"));
        assert!(repl.timing_enabled);
    }

    // -- Existing tests (preserved and updated) --

    #[test]
    fn test_repl_creation() {
        let config = ReplConfig::default();
        let repl = Repl::new(config);
        assert!(repl.running);
        assert!(repl.history.is_empty());
        assert_eq!(repl.active_collection, "default");
        assert!(repl.client.is_none());
        assert!(!repl.timing_enabled);
    }

    #[test]
    fn test_parse_command_set() {
        let cmd = ReplCommand::parse("set mykey myvalue");
        assert_eq!(
            cmd,
            ReplCommand::Set {
                key: "mykey".to_string(),
                value: "myvalue".to_string(),
            }
        );

        let cmd = ReplCommand::parse("set mykey hello world 123");
        assert_eq!(
            cmd,
            ReplCommand::Set {
                key: "mykey".to_string(),
                value: "hello world 123".to_string(),
            }
        );

        let cmd = ReplCommand::parse("put foo bar");
        assert_eq!(
            cmd,
            ReplCommand::Set {
                key: "foo".to_string(),
                value: "bar".to_string(),
            }
        );

        let cmd = ReplCommand::parse("set mykey");
        assert!(matches!(cmd, ReplCommand::Unknown { .. }));

        let cmd = ReplCommand::parse("set");
        assert!(matches!(cmd, ReplCommand::Unknown { .. }));
    }

    #[test]
    fn test_parse_command_get() {
        let cmd = ReplCommand::parse("get mykey");
        assert_eq!(
            cmd,
            ReplCommand::Get {
                key: "mykey".to_string(),
            }
        );

        let cmd = ReplCommand::parse("get");
        assert!(matches!(cmd, ReplCommand::Unknown { .. }));
    }

    #[test]
    fn test_parse_command_delete() {
        let cmd = ReplCommand::parse("delete mykey");
        assert_eq!(
            cmd,
            ReplCommand::Delete {
                key: "mykey".to_string(),
            }
        );

        assert_eq!(
            ReplCommand::parse("del foo"),
            ReplCommand::Delete {
                key: "foo".to_string(),
            }
        );
        assert_eq!(
            ReplCommand::parse("rm bar"),
            ReplCommand::Delete {
                key: "bar".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_command_range() {
        let cmd = ReplCommand::parse("range a z");
        assert_eq!(
            cmd,
            ReplCommand::Range {
                start: "a".to_string(),
                end: "z".to_string(),
            }
        );

        let cmd = ReplCommand::parse("scan start end");
        assert_eq!(
            cmd,
            ReplCommand::Range {
                start: "start".to_string(),
                end: "end".to_string(),
            }
        );

        let cmd = ReplCommand::parse("range a");
        assert!(matches!(cmd, ReplCommand::Unknown { .. }));
    }

    #[test]
    fn test_parse_command_help() {
        assert_eq!(ReplCommand::parse("help"), ReplCommand::Help);
        assert_eq!(ReplCommand::parse("h"), ReplCommand::Help);
        assert_eq!(ReplCommand::parse("?"), ReplCommand::Help);
        assert_eq!(ReplCommand::parse("HELP"), ReplCommand::Help);
        assert_eq!(ReplCommand::parse(".help"), ReplCommand::Help);
    }

    #[test]
    fn test_parse_command_quit() {
        assert_eq!(ReplCommand::parse("quit"), ReplCommand::Quit);
        assert_eq!(ReplCommand::parse("exit"), ReplCommand::Quit);
        assert_eq!(ReplCommand::parse("q"), ReplCommand::Quit);
        assert_eq!(ReplCommand::parse("QUIT"), ReplCommand::Quit);
    }

    #[test]
    fn test_parse_command_use() {
        let cmd = ReplCommand::parse("use mydb");
        assert_eq!(
            cmd,
            ReplCommand::Use {
                collection: "mydb".to_string(),
            }
        );

        let cmd = ReplCommand::parse("use");
        assert!(matches!(cmd, ReplCommand::Unknown { .. }));
    }

    #[test]
    fn test_parse_command_misc() {
        assert_eq!(ReplCommand::parse("history"), ReplCommand::History);
        assert_eq!(ReplCommand::parse(".history"), ReplCommand::History);
        assert_eq!(ReplCommand::parse("clear"), ReplCommand::Clear);
        assert_eq!(ReplCommand::parse("cls"), ReplCommand::Clear);
        assert_eq!(ReplCommand::parse(".clear"), ReplCommand::Clear);
        assert_eq!(ReplCommand::parse("status"), ReplCommand::Status);
        assert_eq!(ReplCommand::parse("keys"), ReplCommand::Keys);
        assert_eq!(ReplCommand::parse("ls"), ReplCommand::Keys);
        assert_eq!(ReplCommand::parse("info"), ReplCommand::Info);
        assert_eq!(ReplCommand::parse("collection"), ReplCommand::Collection);
        assert_eq!(ReplCommand::parse("coll"), ReplCommand::Collection);
        assert_eq!(ReplCommand::parse(".stats"), ReplCommand::Stats);
    }

    #[test]
    fn test_parse_command_timing() {
        assert_eq!(
            ReplCommand::parse(".timing on"),
            ReplCommand::Timing {
                enabled: Some(true)
            }
        );
        assert_eq!(
            ReplCommand::parse(".timing off"),
            ReplCommand::Timing {
                enabled: Some(false)
            }
        );
        assert_eq!(
            ReplCommand::parse(".timing"),
            ReplCommand::Timing { enabled: None }
        );
    }

    #[test]
    fn test_parse_command_unknown() {
        let cmd = ReplCommand::parse("foobar");
        assert!(matches!(cmd, ReplCommand::Unknown { .. }));
    }

    #[test]
    fn test_use_collection() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);
        assert_eq!(repl.active_collection, "default");

        repl.active_collection = "production".to_string();
        repl.client = None;
        assert_eq!(repl.active_collection, "production");
    }

    #[test]
    fn test_format_history_empty() {
        let config = ReplConfig::default();
        let repl = Repl::new(config);
        assert_eq!(repl.format_history(), "(no history)");
    }

    #[test]
    fn test_format_history_with_entries() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);
        repl.history.add("get foo");
        repl.history.add("set bar baz");

        let output = repl.format_history();
        assert!(output.contains("1: get foo"));
        assert!(output.contains("2: set bar baz"));
    }

    #[test]
    fn test_help_text_contains_commands() {
        let text = Repl::help_text();
        assert!(text.contains("set <key> <value>"));
        assert!(text.contains("get <key>"));
        assert!(text.contains("delete <key>"));
        assert!(text.contains("range <start> <end>"));
        assert!(text.contains("keys"));
        assert!(text.contains("use <collection>"));
        assert!(text.contains("quit"));
        assert!(text.contains("help"));
        // New commands
        assert!(text.contains(".stats"));
        assert!(text.contains(".timing"));
        assert!(text.contains("!!"));
        assert!(text.contains("!<prefix>"));
    }

    #[tokio::test]
    async fn test_execute_quit() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);
        assert!(repl.running);

        let result = repl.execute_command(ReplCommand::Quit).await;
        assert!(result.is_ok());
        assert!(!repl.running);
        let msg = result.expect("quit should return Ok");
        assert!(msg.contains("Goodbye!"));
    }

    #[tokio::test]
    async fn test_execute_help() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);

        let result = repl
            .execute_command(ReplCommand::Help)
            .await
            .expect("help should succeed");
        assert!(result.contains("Available commands:"));
    }

    #[tokio::test]
    async fn test_execute_collection() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);

        let result = repl
            .execute_command(ReplCommand::Collection)
            .await
            .expect("collection should succeed");
        assert!(result.contains("default"));
    }

    #[tokio::test]
    async fn test_execute_use() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);

        let result = repl
            .execute_command(ReplCommand::Use {
                collection: "mydb".to_string(),
            })
            .await
            .expect("use should succeed");
        assert!(result.contains("mydb"));
        assert_eq!(repl.active_collection, "mydb");
    }

    #[tokio::test]
    async fn test_execute_info() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);

        let result = repl
            .execute_command(ReplCommand::Info)
            .await
            .expect("info should succeed");
        assert!(result.contains("Server URL:"));
        assert!(result.contains("Collection:"));
        assert!(result.contains("Timing:"));
    }

    #[tokio::test]
    async fn test_execute_unknown() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);

        let result = repl
            .execute_command(ReplCommand::Unknown {
                input: "bogus".to_string(),
            })
            .await
            .expect("unknown should succeed");
        assert!(result.contains("Unknown command"));
    }

    #[tokio::test]
    async fn test_execute_clear() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);

        let result = repl
            .execute_command(ReplCommand::Clear)
            .await
            .expect("clear should succeed");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_execute_history() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);
        repl.history.add("get x");

        let result = repl
            .execute_command(ReplCommand::History)
            .await
            .expect("history should succeed");
        assert!(result.contains("get x"));
    }

    #[tokio::test]
    async fn test_execute_stats() {
        let config = ReplConfig::default();
        let mut repl = Repl::new(config);
        repl.stats.record_success();
        repl.stats.record_success();
        repl.stats.record_error();

        let result = repl
            .execute_command(ReplCommand::Stats)
            .await
            .expect("stats should succeed");
        assert!(result.contains("Commands executed:  3"));
        assert!(result.contains("Errors:             1"));
    }

    #[test]
    fn test_case_insensitive_parsing() {
        assert_eq!(
            ReplCommand::parse("SET foo bar"),
            ReplCommand::Set {
                key: "foo".to_string(),
                value: "bar".to_string(),
            }
        );
        assert_eq!(
            ReplCommand::parse("GET mykey"),
            ReplCommand::Get {
                key: "mykey".to_string(),
            }
        );
        assert_eq!(
            ReplCommand::parse("DELETE mykey"),
            ReplCommand::Delete {
                key: "mykey".to_string(),
            }
        );
    }

    #[test]
    fn test_whitespace_handling() {
        assert_eq!(ReplCommand::parse("  help  "), ReplCommand::Help);
        assert_eq!(
            ReplCommand::parse("  set  foo  bar baz  "),
            ReplCommand::Set {
                key: "foo".to_string(),
                value: "bar baz".to_string(),
            }
        );
    }

    #[test]
    fn test_truncate_display() {
        assert_eq!(truncate_display("short", 10), "short");
        assert_eq!(truncate_display("hello world", 5), "hello...");
        assert_eq!(truncate_display("", 5), "");
        assert_eq!(truncate_display("exact", 5), "exact");
    }

    #[test]
    fn test_history_manager_load_nonexistent_file() {
        let path = std::env::temp_dir().join("amaters_test_hm_noexist_xyz");
        let _ = std::fs::remove_file(&path);
        let mut hm = HistoryManager::new(100, path);
        // Loading a non-existent file should succeed silently
        assert!(hm.load().is_ok());
        assert!(hm.is_empty());
    }

    #[test]
    fn test_parse_bang() {
        assert_eq!(parse_bang("!!"), BangExpansion::Last);
        assert_eq!(parse_bang("!set"), BangExpansion::Prefix("set".to_string()));
        assert_eq!(parse_bang("get foo"), BangExpansion::None);
        assert_eq!(parse_bang("! space"), BangExpansion::None);
    }

    #[test]
    fn test_line_status_complete() {
        assert_eq!(check_line_status("set foo bar"), LineStatus::Complete);
        assert_eq!(check_line_status("get key"), LineStatus::Complete);
        assert_eq!(check_line_status("{}"), LineStatus::Complete);
    }

    #[test]
    fn test_combine_lines_single() {
        let lines = vec!["set foo bar".to_string()];
        assert_eq!(combine_lines(&lines), "set foo bar");
    }

    #[test]
    fn test_default_session_stats() {
        let stats = SessionStats::default();
        assert_eq!(stats.commands_executed, 0);
        assert_eq!(stats.errors, 0);
    }
}
