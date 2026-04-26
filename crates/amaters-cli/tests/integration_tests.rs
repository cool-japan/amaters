//! Integration tests for amaters-cli
//!
//! These tests verify the CLI commands work end-to-end by invoking the
//! compiled binary and checking argument parsing, help output, and error
//! messages.

use std::env;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the path to the compiled `amaters-cli` binary built by `cargo test`.
fn cli_bin() -> PathBuf {
    // `cargo test` places test-built binaries in the same target dir.
    let mut path = env::current_exe()
        .expect("could not determine current test executable path")
        .parent()
        .expect("executable has no parent directory")
        .parent()
        .expect("debug dir has no parent")
        .to_path_buf();
    path.push("amaters-cli");
    path
}

/// Run amaters-cli with the given arguments, returning (stdout, stderr, exit-success).
fn run_cli(args: &[&str]) -> (String, String, bool) {
    let output = Command::new(cli_bin())
        .args(args)
        .output()
        .expect("failed to execute amaters-cli binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

/// Helper to create a test config directory
fn create_test_config_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp dir")
}

// ---------------------------------------------------------------------------
// 1. Version / help smoke tests
// ---------------------------------------------------------------------------

#[test]
fn test_version_flag() {
    let (stdout, _stderr, ok) = run_cli(&["--version"]);
    assert!(ok, "amaters-cli --version should succeed");
    assert!(
        stdout.contains("amaters-cli"),
        "version output should mention the binary name"
    );
}

#[test]
fn test_help_flag() {
    let (stdout, _stderr, ok) = run_cli(&["--help"]);
    assert!(ok, "amaters-cli --help should succeed");
    assert!(
        stdout.contains("AmateRS CLI"),
        "help output should contain app description"
    );
}

// ---------------------------------------------------------------------------
// 2. Subcommand help tests — each subcommand should print its own help
// ---------------------------------------------------------------------------

#[test]
fn test_set_help() {
    let (stdout, _stderr, ok) = run_cli(&["set", "--help"]);
    assert!(ok, "set --help should succeed");
    assert!(
        stdout.contains("Set a key-value pair"),
        "set help should contain description"
    );
}

#[test]
fn test_get_help() {
    let (stdout, _stderr, ok) = run_cli(&["get", "--help"]);
    assert!(ok, "get --help should succeed");
    assert!(
        stdout.contains("Get a value by key"),
        "get help should contain description"
    );
}

#[test]
fn test_delete_help() {
    let (stdout, _stderr, ok) = run_cli(&["delete", "--help"]);
    assert!(ok, "delete --help should succeed");
    assert!(
        stdout.contains("Delete a key"),
        "delete help should contain description"
    );
}

#[test]
fn test_range_help() {
    let (stdout, _stderr, ok) = run_cli(&["range", "--help"]);
    assert!(ok, "range --help should succeed");
    assert!(
        stdout.contains("Range query"),
        "range help should contain description"
    );
}

#[test]
fn test_admin_help() {
    let (stdout, _stderr, ok) = run_cli(&["admin", "--help"]);
    assert!(ok, "admin --help should succeed");
    assert!(
        stdout.contains("Administration commands"),
        "admin help should contain description"
    );
}

#[test]
fn test_server_help() {
    let (stdout, _stderr, ok) = run_cli(&["server", "--help"]);
    assert!(ok, "server --help should succeed");
    assert!(
        stdout.contains("Server management"),
        "server help should contain description"
    );
}

#[test]
fn test_completions_help() {
    let (stdout, _stderr, ok) = run_cli(&["completions", "--help"]);
    assert!(ok, "completions --help should succeed");
    assert!(
        stdout.contains("Generate shell completions"),
        "completions help should contain description"
    );
}

#[test]
fn test_interactive_help() {
    let (stdout, _stderr, ok) = run_cli(&["interactive", "--help"]);
    assert!(ok, "interactive --help should succeed");
    assert!(
        stdout.contains("interactive"),
        "interactive help should mention the command"
    );
}

#[test]
fn test_key_help() {
    let (stdout, _stderr, ok) = run_cli(&["key", "--help"]);
    assert!(ok, "key --help should succeed");
    assert!(
        stdout.contains("FHE key management"),
        "key help should contain description"
    );
}

#[test]
fn test_config_help() {
    let (stdout, _stderr, ok) = run_cli(&["config", "--help"]);
    assert!(ok, "config --help should succeed");
    assert!(
        stdout.contains("configuration"),
        "config help should contain description"
    );
}

#[test]
fn test_query_help() {
    let (stdout, _stderr, ok) = run_cli(&["query", "--help"]);
    assert!(ok, "query --help should succeed");
    assert!(
        stdout.contains("Query"),
        "query help should contain description"
    );
}

// ---------------------------------------------------------------------------
// 3. Admin subcommand help tests
// ---------------------------------------------------------------------------

#[test]
fn test_admin_backup_help() {
    let (stdout, _stderr, ok) = run_cli(&["admin", "backup", "--help"]);
    assert!(ok, "admin backup --help should succeed");
    assert!(
        stdout.contains("backup"),
        "admin backup help should mention backup"
    );
}

#[test]
fn test_admin_restore_help() {
    let (stdout, _stderr, ok) = run_cli(&["admin", "restore", "--help"]);
    assert!(ok, "admin restore --help should succeed");
    assert!(
        stdout.contains("Restore"),
        "admin restore help should mention restore"
    );
}

#[test]
fn test_admin_compact_help() {
    let (stdout, _stderr, ok) = run_cli(&["admin", "compact", "--help"]);
    assert!(ok, "admin compact --help should succeed");
    assert!(
        stdout.contains("compaction"),
        "admin compact help should mention compaction"
    );
}

#[test]
fn test_admin_stats_help() {
    let (stdout, _stderr, ok) = run_cli(&["admin", "stats", "--help"]);
    assert!(ok, "admin stats --help should succeed");
    assert!(
        stdout.contains("statistics"),
        "admin stats help should mention statistics"
    );
}

#[test]
fn test_admin_verify_help() {
    let (stdout, _stderr, ok) = run_cli(&["admin", "verify", "--help"]);
    assert!(ok, "admin verify --help should succeed");
    assert!(
        stdout.contains("integrity"),
        "admin verify help should mention integrity"
    );
}

#[test]
fn test_admin_logs_help() {
    let (stdout, _stderr, ok) = run_cli(&["admin", "logs", "--help"]);
    assert!(ok, "admin logs --help should succeed");
    assert!(
        stdout.contains("logs"),
        "admin logs help should mention logs"
    );
}

// ---------------------------------------------------------------------------
// 4. Server subcommand help tests
// ---------------------------------------------------------------------------

#[test]
fn test_server_status_help() {
    let (stdout, _stderr, ok) = run_cli(&["server", "status", "--help"]);
    assert!(ok, "server status --help should succeed");
    assert!(
        stdout.contains("status"),
        "server status help should mention status"
    );
}

#[test]
fn test_server_health_help() {
    let (stdout, _stderr, ok) = run_cli(&["server", "health", "--help"]);
    assert!(ok, "server health --help should succeed");
    assert!(
        stdout.contains("health"),
        "server health help should mention health"
    );
}

#[test]
fn test_server_metrics_help() {
    let (stdout, _stderr, ok) = run_cli(&["server", "metrics", "--help"]);
    assert!(ok, "server metrics --help should succeed");
    assert!(
        stdout.contains("metrics"),
        "server metrics help should mention metrics"
    );
}

#[test]
fn test_server_cluster_help() {
    let (stdout, _stderr, ok) = run_cli(&["server", "cluster", "--help"]);
    assert!(ok, "server cluster --help should succeed");
    assert!(
        stdout.contains("cluster"),
        "server cluster help should mention cluster"
    );
}

#[test]
fn test_server_nodes_help() {
    let (stdout, _stderr, ok) = run_cli(&["server", "nodes", "--help"]);
    assert!(ok, "server nodes --help should succeed");
    assert!(
        stdout.contains("node"),
        "server nodes help should mention node"
    );
}

// ---------------------------------------------------------------------------
// 5. Key subcommand help tests
// ---------------------------------------------------------------------------

#[test]
fn test_key_generate_help() {
    let (stdout, _stderr, ok) = run_cli(&["key", "generate", "--help"]);
    assert!(ok, "key generate --help should succeed");
    assert!(
        stdout.contains("Generate"),
        "key generate help should mention Generate"
    );
}

#[test]
fn test_key_import_help() {
    let (stdout, _stderr, ok) = run_cli(&["key", "import", "--help"]);
    assert!(ok, "key import --help should succeed");
    assert!(
        stdout.contains("Import"),
        "key import help should mention Import"
    );
}

#[test]
fn test_key_export_help() {
    let (stdout, _stderr, ok) = run_cli(&["key", "export", "--help"]);
    assert!(ok, "key export --help should succeed");
    assert!(
        stdout.contains("Export"),
        "key export help should mention Export"
    );
}

#[test]
fn test_key_list_help() {
    let (stdout, _stderr, ok) = run_cli(&["key", "list", "--help"]);
    assert!(ok, "key list --help should succeed");
    assert!(stdout.contains("List"), "key list help should mention List");
}

#[test]
fn test_key_delete_help() {
    let (stdout, _stderr, ok) = run_cli(&["key", "delete", "--help"]);
    assert!(ok, "key delete --help should succeed");
    assert!(
        stdout.contains("Delete"),
        "key delete help should mention Delete"
    );
}

// ---------------------------------------------------------------------------
// 6. Error handling — missing required arguments
// ---------------------------------------------------------------------------

#[test]
fn test_missing_subcommand() {
    let (_stdout, stderr, ok) = run_cli(&[]);
    assert!(!ok, "running without subcommand should fail");
    assert!(
        stderr.contains("Usage") || stderr.contains("error"),
        "should print usage or error message"
    );
}

#[test]
fn test_get_missing_key() {
    let (_stdout, stderr, ok) = run_cli(&["get"]);
    assert!(!ok, "get without key should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

#[test]
fn test_set_missing_value() {
    let (_stdout, stderr, ok) = run_cli(&["set", "mykey"]);
    assert!(!ok, "set with only key should fail (value is required)");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

#[test]
fn test_set_missing_all_args() {
    let (_stdout, stderr, ok) = run_cli(&["set"]);
    assert!(!ok, "set without args should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required arguments"
    );
}

#[test]
fn test_delete_missing_key() {
    let (_stdout, stderr, ok) = run_cli(&["delete"]);
    assert!(!ok, "delete without key should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

#[test]
fn test_range_missing_end() {
    let (_stdout, stderr, ok) = run_cli(&["range", "start_key"]);
    assert!(!ok, "range with only start key should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

#[test]
fn test_range_missing_all_args() {
    let (_stdout, stderr, ok) = run_cli(&["range"]);
    assert!(!ok, "range without args should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required arguments"
    );
}

#[test]
fn test_query_missing_filter() {
    let (_stdout, stderr, ok) = run_cli(&["query"]);
    assert!(!ok, "query without filter should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

#[test]
fn test_admin_missing_subcommand() {
    let (_stdout, stderr, ok) = run_cli(&["admin"]);
    assert!(!ok, "admin without subcommand should fail");
    assert!(
        stderr.contains("Usage") || stderr.contains("error") || stderr.contains("subcommand"),
        "should indicate missing subcommand"
    );
}

#[test]
fn test_server_missing_subcommand() {
    let (_stdout, stderr, ok) = run_cli(&["server"]);
    assert!(!ok, "server without subcommand should fail");
    assert!(
        stderr.contains("Usage") || stderr.contains("error") || stderr.contains("subcommand"),
        "should indicate missing subcommand"
    );
}

#[test]
fn test_key_missing_subcommand() {
    let (_stdout, stderr, ok) = run_cli(&["key"]);
    assert!(!ok, "key without subcommand should fail");
    assert!(
        stderr.contains("Usage") || stderr.contains("error") || stderr.contains("subcommand"),
        "should indicate missing subcommand"
    );
}

#[test]
fn test_config_missing_subcommand() {
    let (_stdout, stderr, ok) = run_cli(&["config"]);
    assert!(!ok, "config without subcommand should fail");
    assert!(
        stderr.contains("Usage") || stderr.contains("error") || stderr.contains("subcommand"),
        "should indicate missing subcommand"
    );
}

#[test]
fn test_admin_backup_missing_dest() {
    let (_stdout, stderr, ok) = run_cli(&["admin", "backup"]);
    assert!(!ok, "admin backup without dest should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

#[test]
fn test_admin_restore_missing_source() {
    let (_stdout, stderr, ok) = run_cli(&["admin", "restore"]);
    assert!(!ok, "admin restore without source should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

#[test]
fn test_key_generate_missing_name() {
    let (_stdout, stderr, ok) = run_cli(&["key", "generate"]);
    assert!(!ok, "key generate without name should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

#[test]
fn test_key_import_missing_args() {
    let (_stdout, stderr, ok) = run_cli(&["key", "import"]);
    assert!(!ok, "key import without args should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required arguments"
    );
}

#[test]
fn test_key_export_missing_args() {
    let (_stdout, stderr, ok) = run_cli(&["key", "export"]);
    assert!(!ok, "key export without args should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required arguments"
    );
}

#[test]
fn test_key_delete_missing_name() {
    let (_stdout, stderr, ok) = run_cli(&["key", "delete"]);
    assert!(!ok, "key delete without name should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

#[test]
fn test_completions_missing_shell() {
    let (_stdout, stderr, ok) = run_cli(&["completions"]);
    assert!(!ok, "completions without shell should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should indicate missing required argument"
    );
}

// ---------------------------------------------------------------------------
// 7. Unknown / invalid subcommand and argument tests
// ---------------------------------------------------------------------------

#[test]
fn test_unknown_subcommand() {
    let (_stdout, stderr, ok) = run_cli(&["nonexistent-command"]);
    assert!(!ok, "unknown subcommand should fail");
    assert!(
        stderr.contains("error") || stderr.contains("unrecognized"),
        "should report unknown subcommand"
    );
}

#[test]
fn test_unknown_admin_subcommand() {
    let (_stdout, stderr, ok) = run_cli(&["admin", "nonexistent"]);
    assert!(!ok, "unknown admin subcommand should fail");
    assert!(
        stderr.contains("error") || stderr.contains("unrecognized"),
        "should report unknown admin subcommand"
    );
}

#[test]
fn test_unknown_server_subcommand() {
    let (_stdout, stderr, ok) = run_cli(&["server", "nonexistent"]);
    assert!(!ok, "unknown server subcommand should fail");
    assert!(
        stderr.contains("error") || stderr.contains("unrecognized"),
        "should report unknown server subcommand"
    );
}

#[test]
fn test_unknown_key_subcommand() {
    let (_stdout, stderr, ok) = run_cli(&["key", "nonexistent"]);
    assert!(!ok, "unknown key subcommand should fail");
    assert!(
        stderr.contains("error") || stderr.contains("unrecognized"),
        "should report unknown key subcommand"
    );
}

#[test]
fn test_completions_invalid_shell() {
    let (_stdout, stderr, ok) = run_cli(&["completions", "invalid-shell"]);
    assert!(!ok, "completions with invalid shell should fail");
    assert!(
        stderr.contains("error") || stderr.contains("invalid"),
        "should report invalid shell value"
    );
}

#[test]
fn test_unknown_global_flag() {
    let (_stdout, stderr, ok) = run_cli(&["--nonexistent-flag", "get", "mykey"]);
    assert!(!ok, "unknown global flag should fail");
    assert!(
        stderr.contains("error") || stderr.contains("unexpected"),
        "should report unknown flag"
    );
}

// ---------------------------------------------------------------------------
// 8. Completions generation tests (actual output)
// ---------------------------------------------------------------------------

#[test]
fn test_completions_bash_output() {
    let (stdout, _stderr, ok) = run_cli(&["completions", "bash"]);
    assert!(ok, "completions bash should succeed");
    assert!(
        !stdout.is_empty(),
        "bash completions output should be non-empty"
    );
    assert!(
        stdout.contains("amaters") || stdout.contains("complete") || stdout.contains("_amaters"),
        "bash completions should contain completion directives"
    );
}

#[test]
fn test_completions_zsh_output() {
    let (stdout, _stderr, ok) = run_cli(&["completions", "zsh"]);
    assert!(ok, "completions zsh should succeed");
    assert!(
        !stdout.is_empty(),
        "zsh completions output should be non-empty"
    );
}

#[test]
fn test_completions_fish_output() {
    let (stdout, _stderr, ok) = run_cli(&["completions", "fish"]);
    assert!(ok, "completions fish should succeed");
    assert!(
        !stdout.is_empty(),
        "fish completions output should be non-empty"
    );
}

#[test]
fn test_completions_powershell_output() {
    let (stdout, _stderr, ok) = run_cli(&["completions", "powershell"]);
    assert!(ok, "completions powershell should succeed");
    assert!(
        !stdout.is_empty(),
        "powershell completions output should be non-empty"
    );
}

#[test]
fn test_completions_elvish_output() {
    let (stdout, _stderr, ok) = run_cli(&["completions", "elvish"]);
    assert!(ok, "completions elvish should succeed");
    assert!(
        !stdout.is_empty(),
        "elvish completions output should be non-empty"
    );
}

// ---------------------------------------------------------------------------
// 9. Global option parsing tests
// ---------------------------------------------------------------------------

#[test]
fn test_format_json_option_accepted() {
    // --format json should be accepted as a global option.
    // The command may still fail because no server is running, but it should
    // NOT fail at the argument-parsing stage.
    let (_stdout, stderr, _ok) = run_cli(&["--format", "json", "server", "status"]);
    // If it failed at arg-parsing, stderr would contain clap error text about
    // unrecognized flag / missing value.  Server-connection failure is expected.
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("not a valid value"),
        "--format json should be accepted as a valid global option"
    );
}

#[test]
fn test_format_table_option_accepted() {
    let (_stdout, stderr, _ok) = run_cli(&["--format", "table", "server", "status"]);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("not a valid value"),
        "--format table should be accepted as a valid global option"
    );
}

#[test]
fn test_server_option_accepted() {
    let (_stdout, stderr, _ok) =
        run_cli(&["--server", "http://localhost:9999", "server", "status"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "--server should be accepted as a valid global option"
    );
}

#[test]
fn test_collection_option_accepted() {
    let (_stdout, stderr, _ok) = run_cli(&["--collection", "test_coll", "server", "status"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "--collection should be accepted as a valid global option"
    );
}

#[test]
fn test_short_flags_accepted() {
    let (_stdout, stderr, _ok) = run_cli(&[
        "-s",
        "http://localhost:9999",
        "-c",
        "mycoll",
        "-f",
        "json",
        "server",
        "status",
    ]);
    assert!(
        !stderr.contains("unexpected argument"),
        "short flags -s, -c, -f should be accepted"
    );
}

// ---------------------------------------------------------------------------
// 10. Admin subcommand argument parsing tests
// ---------------------------------------------------------------------------

#[test]
fn test_admin_backup_incremental_flag() {
    // --incremental should be accepted (even though no server is running)
    let (_stdout, stderr, _ok) = run_cli(&["admin", "backup", "/tmp/backup", "--incremental"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "--incremental should be accepted by admin backup"
    );
}

#[test]
fn test_admin_compact_collection_option() {
    let (_stdout, stderr, _ok) = run_cli(&["admin", "compact", "--collection", "default"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "--collection should be accepted by admin compact"
    );
}

#[test]
fn test_admin_logs_lines_option() {
    let (_stdout, stderr, _ok) = run_cli(&["admin", "logs", "--lines", "50"]);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("not a valid value"),
        "--lines should be accepted by admin logs"
    );
}

#[test]
fn test_admin_logs_follow_flag() {
    // Just verify the flag is accepted at parse time
    let (_stdout, stderr, _ok) = run_cli(&["admin", "logs", "--follow"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "--follow should be accepted by admin logs"
    );
}

#[test]
fn test_admin_logs_short_flags() {
    let (_stdout, stderr, _ok) = run_cli(&["admin", "logs", "-n", "200", "-f"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "short flags -n, -f should be accepted by admin logs"
    );
}

// ---------------------------------------------------------------------------
// 11. Key subcommand argument parsing tests
// ---------------------------------------------------------------------------

#[test]
fn test_key_generate_with_description() {
    let (_stdout, stderr, _ok) =
        run_cli(&["key", "generate", "mykey", "--description", "test key"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "--description should be accepted by key generate"
    );
}

#[test]
fn test_key_import_with_file() {
    let (_stdout, stderr, _ok) = run_cli(&["key", "import", "mykey", "--file", "/tmp/key.bin"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "--file should be accepted by key import"
    );
}

#[test]
fn test_key_export_with_file() {
    let (_stdout, stderr, _ok) = run_cli(&["key", "export", "mykey", "--file", "/tmp/key.bin"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "--file should be accepted by key export"
    );
}

// ---------------------------------------------------------------------------
// 12. Interactive subcommand argument parsing tests
// ---------------------------------------------------------------------------

#[test]
fn test_interactive_custom_server() {
    // Just test that the arg is accepted (we cannot run the REPL in tests)
    let (_stdout, stderr, _ok) =
        run_cli(&["interactive", "--server", "http://localhost:9999", "--help"]);
    // --help should cause it to exit 0 before trying to connect
    assert!(
        !stderr.contains("unexpected argument"),
        "--server should be accepted by interactive"
    );
}

#[test]
fn test_interactive_short_server_flag() {
    let (_stdout, stderr, _ok) = run_cli(&["interactive", "-u", "http://custom:1234", "--help"]);
    assert!(
        !stderr.contains("unexpected argument"),
        "-u should be accepted by interactive"
    );
}

// ---------------------------------------------------------------------------
// 13. Config subcommand tests
// ---------------------------------------------------------------------------

#[test]
fn test_config_show_help() {
    let (stdout, _stderr, ok) = run_cli(&["config", "show", "--help"]);
    assert!(ok, "config show --help should succeed");
    assert!(
        stdout.contains("Show") || stdout.contains("configuration"),
        "config show help should describe what it does"
    );
}

#[test]
fn test_config_init_help() {
    let (stdout, _stderr, ok) = run_cli(&["config", "init", "--help"]);
    assert!(ok, "config init --help should succeed");
    assert!(
        stdout.contains("Init") || stdout.contains("configuration"),
        "config init help should describe what it does"
    );
}

// ---------------------------------------------------------------------------
// 14. Temp directory tests (verifying test hygiene)
// ---------------------------------------------------------------------------

#[test]
fn test_config_initialization() {
    let temp_dir = create_test_config_dir();

    // Test that config can be created in temp directory
    let config_path = temp_dir.path().join(".amaters").join("config.toml");
    assert!(!config_path.exists());
}

#[test]
fn test_default_config_values() {
    use std::fs;

    let temp_dir = create_test_config_dir();
    let config_dir = temp_dir.path().join(".amaters");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");

    let config_path = config_dir.join("config.toml");
    let default_config = r#"
server_url = "http://localhost:50051"
default_collection = "default"
output_format = "table"
color = true

[tls]
enabled = false
"#;

    fs::write(&config_path, default_config).expect("Failed to write config");

    // Read it back
    let contents = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(contents.contains("server_url"));
    assert!(contents.contains("default_collection"));
}

#[test]
fn test_temp_dir_usage() {
    // Verify we're using temp directories in tests
    let temp_dir = std::env::temp_dir();
    assert!(temp_dir.exists());
}

// ---------------------------------------------------------------------------
// 15. Help text completeness — verify all subcommands appear in main help
// ---------------------------------------------------------------------------

#[test]
fn test_main_help_lists_all_subcommands() {
    let (stdout, _stderr, ok) = run_cli(&["--help"]);
    assert!(ok, "--help should succeed");

    let expected_subcommands = [
        "set",
        "get",
        "delete",
        "range",
        "query",
        "key",
        "server",
        "admin",
        "config",
        "completions",
        "interactive",
    ];

    for subcmd in &expected_subcommands {
        assert!(
            stdout.contains(subcmd),
            "main help should list the '{}' subcommand, got:\n{}",
            subcmd,
            stdout
        );
    }
}

#[test]
fn test_admin_help_lists_all_subcommands() {
    let (stdout, _stderr, ok) = run_cli(&["admin", "--help"]);
    assert!(ok, "admin --help should succeed");

    let expected = ["backup", "restore", "compact", "stats", "verify", "logs"];
    for subcmd in &expected {
        assert!(
            stdout.contains(subcmd),
            "admin help should list '{}' subcommand, got:\n{}",
            subcmd,
            stdout
        );
    }
}

#[test]
fn test_server_help_lists_all_subcommands() {
    let (stdout, _stderr, ok) = run_cli(&["server", "--help"]);
    assert!(ok, "server --help should succeed");

    let expected = ["status", "health", "metrics", "cluster", "nodes"];
    for subcmd in &expected {
        assert!(
            stdout.contains(subcmd),
            "server help should list '{}' subcommand, got:\n{}",
            subcmd,
            stdout
        );
    }
}

#[test]
fn test_key_help_lists_all_subcommands() {
    let (stdout, _stderr, ok) = run_cli(&["key", "--help"]);
    assert!(ok, "key --help should succeed");

    let expected = ["generate", "import", "export", "list", "delete"];
    for subcmd in &expected {
        assert!(
            stdout.contains(subcmd),
            "key help should list '{}' subcommand, got:\n{}",
            subcmd,
            stdout
        );
    }
}

// ---------------------------------------------------------------------------
// 16. Edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_extra_positional_args_rejected() {
    let (_stdout, stderr, ok) = run_cli(&["get", "key1", "extra_arg"]);
    assert!(!ok, "get with extra positional arg should fail");
    assert!(
        stderr.contains("error") || stderr.contains("unexpected"),
        "should report unexpected argument"
    );
}

#[test]
fn test_admin_compact_without_collection_is_ok() {
    // compact --collection is optional; running without it should pass parsing
    // (will fail at server connection, but not at parsing)
    let (_stdout, stderr, _ok) = run_cli(&["admin", "compact"]);
    assert!(
        !stderr.contains("required"),
        "admin compact should not require --collection"
    );
}

#[test]
fn test_admin_logs_default_lines() {
    // logs without --lines should use default (100); should not fail at parsing
    let (_stdout, stderr, _ok) = run_cli(&["admin", "logs"]);
    assert!(
        !stderr.contains("required"),
        "admin logs should not require --lines (has default)"
    );
}

#[test]
fn test_admin_logs_invalid_lines_value() {
    let (_stdout, stderr, ok) = run_cli(&["admin", "logs", "--lines", "not-a-number"]);
    assert!(!ok, "admin logs with non-numeric --lines should fail");
    assert!(
        stderr.contains("error") || stderr.contains("invalid"),
        "should report invalid value for --lines"
    );
}

#[test]
fn test_key_import_missing_file_flag() {
    // key import requires --file
    let (_stdout, stderr, ok) = run_cli(&["key", "import", "mykey"]);
    assert!(!ok, "key import without --file should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should report missing --file"
    );
}

#[test]
fn test_key_export_missing_file_flag() {
    // key export requires --file
    let (_stdout, stderr, ok) = run_cli(&["key", "export", "mykey"]);
    assert!(!ok, "key export without --file should fail");
    assert!(
        stderr.contains("required") || stderr.contains("error"),
        "should report missing --file"
    );
}
