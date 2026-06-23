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

Status: complete.

- Add customer identity records.
- Add authentication scaffolding with login/logout sessions.
- Hash secrets with Argon2 instead of storing raw values.
- Add role-based operations for customer, teller, and admin flows.
- Link accounts to customer owners and restrict customer access to owned accounts.

### Phase 4: API and Product Layer

Status: complete.

- Add a REST API with Axum.
- Keep the CLI as the default admin/dev client.
- Add Basic-auth protected endpoints for identity and banking operations.
- Add request validation, structured JSON responses, and API tests.
- Add a `serve` command for running the HTTP API.

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

## Running

Start the CLI:

```bash
cargo run
```

Start the HTTP API:

```bash
cargo run -- serve 127.0.0.1:3000
```

The API starts empty. Use `POST /auth/bootstrap-admin` first, then call protected endpoints with Basic auth.
