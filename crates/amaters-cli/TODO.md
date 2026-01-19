# amaters-cli TODO

## Phase 1: Basic Commands 📋

- [x] Argument parsing (clap)
- [x] Set command
- [x] Get command
- [x] Delete command
- [x] Configuration loading
- [x] Error messages
- [x] Help text

## Phase 2: Query Commands 📋

- [x] Query execution
- [ ] AQL parser (partially implemented, filter queries need SDK support)
- [x] Range queries
- [x] Filter queries (API ready, needs SDK implementation)
- [ ] Pagination
- [x] Result formatting

## Phase 3: Key Management 📋

- [x] Key generation
- [x] Key import/export
- [x] Key storage
- [x] Key listing
- [ ] Default key selection

## Phase 4: Server Management 📋

- [x] Status command
- [x] Health checks
- [x] Cluster info
- [x] Node management
- [x] Metrics viewing

## Phase 5: Administration 📋

- [x] Backup command
- [x] Restore command
- [x] Compact command
- [x] Statistics
- [x] Logs viewing
- [x] Verify command

## Phase 6: Output Formatting 📋

- [x] JSON output
- [x] YAML output
- [x] Table output
- [x] Pretty printing
- [x] Color support
- [x] Progress bars

## Phase 7: Shell Integration 📋

- [ ] Bash completion
- [ ] Zsh completion
- [ ] Fish completion
- [ ] Interactive mode
- [ ] Command history

## Phase 8: Advanced Features 📋

- [ ] Batch operations
- [ ] Piping support
- [ ] Watch mode
- [ ] Diff command
- [ ] Import/export

## Dependencies

- `amaters-core` - Core types
- `amaters-sdk-rust` - Client SDK
- `clap` - CLI parsing
- `tokio` - Async runtime
- `serde_json` - JSON output
- `comfy-table` - Table formatting

## Notes

- CLI should be user-friendly
- Error messages should be clear
- Support piping for scripting
- Add interactive mode for exploration
