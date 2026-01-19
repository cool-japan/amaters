# amaters-cli

Command-line interface for AmateRS

## Overview

`amaters-cli` provides a convenient command-line tool for interacting with AmateRS servers. It's useful for administration, debugging, and quick operations.

**Current Status**: Phase 1-6 implemented - Basic commands (Set, Get, Delete), Query commands (Range, Filter), Key Management, Server Management, Administration, Configuration management, and Output formatting (JSON, YAML, Table) with progress bars are fully functional.

## Installation

```bash
# Build from source
cargo install --path crates/amaters-cli

# Or use cargo install (once published)
cargo install amaters-cli
```

## Quick Start

```bash
# Initialize configuration (optional)
amaters-cli config init

# Set environment variables (optional)
export AMATERS_SERVER_URL="http://localhost:50051"
export AMATERS_COLLECTION="default"
export AMATERS_OUTPUT_FORMAT="table"

# Set a value
amaters-cli set my_key "my encrypted value"

# Get a value
amaters-cli get my_key

# Delete a value
amaters-cli delete my_key

# Range query
amaters-cli range start_key end_key

# Query with filter (API ready, needs SDK implementation)
amaters-cli query "age > 18"

# Show configuration
amaters-cli config show
```

## Commands

### ✅ Data Operations (Implemented)

```bash
# Set a key-value pair
amaters-cli set <key> <value>

# Get a value by key
amaters-cli get <key>

# Delete a key
amaters-cli delete <key>
```

### ✅ Query Operations (Implemented)

```bash
# Range query
amaters-cli range <start-key> <end-key>

# Execute filter query (API ready, needs SDK implementation)
amaters-cli query <filter-expression>
```

### ✅ Configuration (Implemented)

```bash
# Initialize configuration file
amaters-cli config init

# Show current configuration
amaters-cli config show
```

### ✅ Key Management (Implemented - Phase 3)

```bash
# Generate new FHE keys
amaters-cli key generate <key-name> --description "My key"

# Import key from file
amaters-cli key import <key-name> --file <path> --description "Imported key"

# Export key to file
amaters-cli key export <key-name> --file <output-path>

# List all keys
amaters-cli key list

# Delete a key
amaters-cli key delete <key-name>
```

### ✅ Server Management (Implemented - Phase 4)

```bash
# Show detailed server status
amaters-cli server status

# Perform health check
amaters-cli server health

# Show server metrics
amaters-cli server metrics

# Show cluster information
amaters-cli server cluster

# Show node information
amaters-cli server nodes
```

### ✅ Administration (Implemented - Phase 5)

```bash
# Create a database backup
amaters-cli admin backup <destination-dir> [--incremental]

# Restore from backup
amaters-cli admin restore <source-dir>

# Trigger manual compaction
amaters-cli admin compact [--collection <name>]

# Show database statistics
amaters-cli admin stats

# Verify database integrity
amaters-cli admin verify

# Show server logs
amaters-cli admin logs [-n <lines>] [--follow]
```

## Configuration

Configuration file is automatically created at `~/.amaters/config.toml`:

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

You can initialize it with:
```bash
amaters-cli config init
```

### Command-line Options

Override configuration with flags:

```bash
amaters-cli -s <server-url> -c <collection> -f <format> <command>
```

Options:
- `-s, --server <URL>`: Server URL
- `-c, --collection <NAME>`: Collection name
- `-f, --format <FORMAT>`: Output format (json, yaml, table)

### Environment Variables

Environment variables override config file:

```bash
# Server URL
export AMATERS_SERVER_URL="http://localhost:50051"

# Default collection
export AMATERS_COLLECTION="default"

# Output format
export AMATERS_OUTPUT_FORMAT="json"

# Enable/disable color
export AMATERS_COLOR="true"
```

## Output Formats

### Table (default)
```bash
amaters-cli get my_key
# Or explicitly:
amaters-cli -f table get my_key
```

Output:
```
┌────────────┬──────────────────────┐
│ Property   │ Value                │
├────────────┼──────────────────────┤
│ Key        │ my_key               │
│ Size       │ 1024 bytes           │
│ Checksum   │ 0xa1b2c3d4           │
│ Created At │ 2026-01-17T12:00:00Z │
└────────────┴──────────────────────┘
```

### JSON
```bash
amaters-cli -f json get my_key
```

Output:
```json
{
  "status": "success",
  "operation": "get",
  "key": "my_key",
  "value": {
    "size": 1024,
    "checksum": 2712847316,
    "created_at": "2026-01-17T12:00:00Z"
  }
}
```

### YAML
```bash
amaters-cli -f yaml get my_key
```

Output:
```yaml
status: success
operation: get
key: my_key
value:
  size: 1024
  checksum: 2712847316
  created_at: '2026-01-17T12:00:00Z'
```

## Examples

### Basic Operations

```bash
# Set a value
amaters-cli set user:123 "John Doe"
# ✓ Successfully set key: user:123

# Get a value
amaters-cli get user:123
# Shows detailed table

# Delete a value
amaters-cli delete user:123
# ✓ Successfully deleted key: user:123
```

### Using Different Output Formats

```bash
# JSON output
amaters-cli -f json get user:123

# Table output (default)
amaters-cli get user:123
```

### Range Queries

```bash
# Query a range of keys
amaters-cli range user:000 user:999

# With JSON output
amaters-cli -f json range user:000 user:999
```

### Configuration

```bash
# Initialize config
amaters-cli config init
# ✓ Configuration initialized at: ~/.amaters/config.toml

# View config
amaters-cli config show
```

### Using Environment Variables

```bash
# Override server URL
export AMATERS_SERVER_URL="http://production.example.com:50051"
amaters-cli get user:123

# Override collection
export AMATERS_COLLECTION="production"
amaters-cli set key value
```

### Key Management

```bash
# Generate a new key
amaters-cli key generate my_key --description "Production key"
# ⠋ Generating FHE keys (this may take a while)...
# ✓ Generated key 'my_key' (12345 bytes)

# List all keys
amaters-cli key list
# Shows JSON/YAML/Table with all keys

# Export a key
amaters-cli key export my_key --file ./backup/my_key.key
# ✓ Exported key 'my_key' to "./backup/my_key.key"

# Import a key
amaters-cli key import restored_key --file ./backup/my_key.key
# ✓ Imported key 'restored_key' from "./backup/my_key.key"

# Delete a key
amaters-cli key delete old_key
# ✓ Deleted key 'old_key'
```

### Server Management

```bash
# Check server status
amaters-cli server status
# Shows version, uptime, connections, memory usage, etc.

# Health check
amaters-cli server health
# Shows database, consensus, and network health

# View metrics
amaters-cli server metrics
# Shows QPS, latency, cache hit rate, etc.

# Cluster information
amaters-cli server cluster
# Shows cluster ID, nodes, leader, replication factor

# Node information
amaters-cli server nodes
# Shows details about each node in the cluster
```

### Administration

```bash
# Create a backup
amaters-cli admin backup /backups/db_$(date +%Y%m%d)
# ⠋ Creating backup...
# Shows backup metadata (ID, size, key count, etc.)

# Create incremental backup
amaters-cli admin backup /backups/incremental --incremental

# Restore from backup
amaters-cli admin restore /backups/db_20260117
# ⠋ Restoring from backup...
# Shows restore result (keys restored, duration, etc.)

# Trigger compaction
amaters-cli admin compact
# ⠋ Running compaction...
# Shows compaction result (bytes reclaimed, duration, etc.)

# Compact specific collection
amaters-cli admin compact --collection users

# View database statistics
amaters-cli admin stats
# Shows detailed DB stats (SSTables, memtables, WAL, compaction)

# Verify database integrity
amaters-cli admin verify
# ⠋ Verifying database integrity...
# Shows verification result (verified keys, errors found, etc.)

# View server logs
amaters-cli admin logs
# Shows last 100 lines

# View last 500 lines
amaters-cli admin logs -n 500

# Follow logs (tail -f style)
amaters-cli admin logs --follow
```

## Shell Completion (Planned - Phase 7)

Shell completion will be available in a future release.

## License

Licensed under MIT OR Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
