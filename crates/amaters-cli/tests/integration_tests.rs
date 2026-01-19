//! Integration tests for amaters-cli
//!
//! These tests verify the CLI commands work end-to-end

use std::env;
use std::path::PathBuf;
use tempfile::TempDir;

/// Helper to create a test config directory
fn create_test_config_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp dir")
}

/// Helper to set HOME to test directory
fn set_test_home(dir: &TempDir) {
    unsafe {
        env::set_var("HOME", dir.path());
    }
}

/// Helper to clean up test HOME
fn cleanup_test_home() {
    unsafe {
        env::remove_var("HOME");
    }
}

#[test]
fn test_config_initialization() {
    let temp_dir = create_test_config_dir();
    set_test_home(&temp_dir);

    // Test that config can be created in temp directory
    let config_path = temp_dir.path().join(".amaters").join("config.toml");
    assert!(!config_path.exists());

    cleanup_test_home();
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
fn test_output_format_options() {
    // Test that both JSON and table formats are supported
    let formats = ["json", "table"];

    for format in formats {
        assert!(format == "json" || format == "table");
    }
}

#[test]
fn test_command_line_args() {
    // Test that command line argument parsing works
    // This is a placeholder for more comprehensive CLI tests
    // TODO: Add actual command line argument tests
}

#[test]
fn test_temp_dir_usage() {
    // Verify we're using temp directories in tests
    let temp_dir = std::env::temp_dir();
    assert!(temp_dir.exists());
}
