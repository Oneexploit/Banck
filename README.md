# Bank

A Rust banking practice project that is being upgraded in phases from a simple CLI exercise into a cleaner, testable banking application.

## Phase Roadmap

### Phase 1: Core Banking Foundation

Status: complete.

- Split the application into `domain`, `bank`, and `cli` modules.
- Replaced floating point money with a cent-based `Money` type.
- Added typed banking errors with `Result` instead of domain-level `println!`.
- Added account status, transaction kinds, loan balance, transfer, fee, interest, and statement support in the core service.
- Added unit tests for money parsing and critical banking flows.
- Kept the existing CLI commands working on top of the new service layer.

### Phase 2: Durable Storage

Status: complete.

- Add structured serialization with `serde`.
- Load and save accounts from JSON first, then prepare for SQLite/Postgres.
- Separate exportable statements from internal application state.
- Add tests for persistence and corrupted input handling.
- Add CLI commands for `save`, `load`, and `export`.

### Phase 3: Security and Identity

Planned:

- Add customer identity records.
- Add authentication scaffolding.
- Hash secrets instead of storing raw values.
- Add role-based operations for teller/admin/customer flows.

### Phase 4: API and Product Layer

Planned:

- Add a REST API with a Rust web framework.
- Keep the CLI as an admin/dev client.
- Add request validation, structured responses, and API tests.

### Phase 5: Operational Quality

Planned:

- Add CI for formatting, Clippy, tests, and audit checks.
- Add audit logs and transaction IDs.
- Add migration strategy for durable storage.
- Add documentation for setup, commands, and architecture.

## Development Checks

Run these before shipping a phase:

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
```
