use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    app::AppState,
    domain::current_epoch_seconds,
    storage::{StorageError, app_from_json, app_to_json},
};

const CURRENT_SQLITE_SCHEMA_VERSION: i64 = 1;

#[derive(Debug)]
pub enum SqliteStoreError {
    Sqlite {
        path: PathBuf,
        source: rusqlite::Error,
    },
    Storage(StorageError),
    MissingState,
    UnsupportedSchemaVersion(i64),
}

impl fmt::Display for SqliteStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite { path, source } => write!(formatter, "{}: {}", path.display(), source),
            Self::Storage(source) => write!(formatter, "{source}"),
            Self::MissingState => write!(formatter, "sqlite database does not contain app state"),
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "unsupported sqlite schema version {version}")
            }
        }
    }
}

impl Error for SqliteStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite { source, .. } => Some(source),
            Self::Storage(source) => Some(source),
            Self::MissingState | Self::UnsupportedSchemaVersion(_) => None,
        }
    }
}

pub type SqliteStoreResult<T> = Result<T, SqliteStoreError>;

pub fn initialize_sqlite_file(path: impl AsRef<Path>) -> SqliteStoreResult<()> {
    let path = path.as_ref();
    let connection = open_connection(path)?;
    migrate(&connection, path)
}

pub fn save_app_to_sqlite_file(state: &AppState, path: impl AsRef<Path>) -> SqliteStoreResult<()> {
    let path = path.as_ref();
    let mut connection = open_connection(path)?;
    migrate(&connection, path)?;

    let json = app_to_json(state).map_err(SqliteStoreError::Storage)?;
    let transaction = connection
        .transaction()
        .map_err(|source| sqlite_error(path, source))?;
    transaction
        .execute(
            "INSERT INTO app_state (id, json, updated_at_epoch_seconds)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                json = excluded.json,
                updated_at_epoch_seconds = excluded.updated_at_epoch_seconds",
            params![json, current_epoch_seconds()],
        )
        .map_err(|source| sqlite_error(path, source))?;
    transaction
        .commit()
        .map_err(|source| sqlite_error(path, source))?;

    Ok(())
}

pub fn load_app_from_sqlite_file(path: impl AsRef<Path>) -> SqliteStoreResult<AppState> {
    let path = path.as_ref();
    let connection = open_connection(path)?;
    migrate(&connection, path)?;
    let json = connection
        .query_row("SELECT json FROM app_state WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|source| sqlite_error(path, source))?
        .ok_or(SqliteStoreError::MissingState)?;

    app_from_json(&json).map_err(SqliteStoreError::Storage)
}

pub fn sqlite_schema_version(path: impl AsRef<Path>) -> SqliteStoreResult<i64> {
    let path = path.as_ref();
    let connection = open_connection(path)?;
    migrate(&connection, path)?;
    current_schema_version(&connection, path)
}

fn open_connection(path: &Path) -> SqliteStoreResult<Connection> {
    let connection = Connection::open(path).map_err(|source| sqlite_error(path, source))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|source| sqlite_error(path, source))?;

    Ok(connection)
}

fn migrate(connection: &Connection, path: &Path) -> SqliteStoreResult<()> {
    ensure_migration_table(connection, path)?;
    let version = current_schema_version(connection, path)?;

    if version > CURRENT_SQLITE_SCHEMA_VERSION {
        return Err(SqliteStoreError::UnsupportedSchemaVersion(version));
    }

    if version < 1 {
        apply_migration_1(connection, path)?;
    }

    Ok(())
}

fn ensure_migration_table(connection: &Connection, path: &Path) -> SqliteStoreResult<()> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at_epoch_seconds INTEGER NOT NULL
            );",
        )
        .map_err(|source| sqlite_error(path, source))
}

fn current_schema_version(connection: &Connection, path: &Path) -> SqliteStoreResult<i64> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|source| sqlite_error(path, source))
}

fn apply_migration_1(connection: &Connection, path: &Path) -> SqliteStoreResult<()> {
    connection
        .execute_batch(
            "CREATE TABLE app_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                json TEXT NOT NULL,
                updated_at_epoch_seconds INTEGER NOT NULL
            );
            CREATE TABLE app_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .map_err(|source| sqlite_error(path, source))?;
    connection
        .execute(
            "INSERT INTO schema_migrations (version, name, applied_at_epoch_seconds)
             VALUES (?1, ?2, ?3)",
            params![
                CURRENT_SQLITE_SCHEMA_VERSION,
                "create_app_state_snapshot",
                current_epoch_seconds()
            ],
        )
        .map_err(|source| sqlite_error(path, source))?;

    Ok(())
}

fn sqlite_error(path: &Path, source: rusqlite::Error) -> SqliteStoreError {
    SqliteStoreError::Sqlite {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        AppState,
        audit::{AuditAction, AuditActor, AuditOutcome},
        bank::Bank,
        domain::Money,
        identity::{IdentityStore, Role},
    };

    use super::{
        SqliteStoreError, initialize_sqlite_file, load_app_from_sqlite_file,
        save_app_to_sqlite_file, sqlite_schema_version,
    };

    fn money(input: &str) -> Money {
        input.parse().unwrap()
    }

    fn temp_sqlite_path(name: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!("bank-{name}-{timestamp}.sqlite3"))
    }

    fn sample_state() -> AppState {
        let mut identities = IdentityStore::new();
        identities
            .create_customer(10, "Alice", "alice@example.com")
            .unwrap();
        identities
            .create_user(1, "alice", Role::Customer, Some(10), "correct-password")
            .unwrap();

        let mut bank = Bank::new();
        bank.create_account(
            100,
            Some(10),
            "Alice Checking",
            "alice@example.com",
            money("75.00"),
        )
        .unwrap();
        let mut state = AppState::from_parts(bank, identities);
        state
            .audit_log
            .record(
                AuditActor::System,
                AuditAction::CreateAccount,
                AuditOutcome::Success,
                Some("account:100".to_string()),
                "created",
            )
            .unwrap();
        state
    }

    #[test]
    fn initializes_schema_migrations() {
        let path = temp_sqlite_path("migration");

        initialize_sqlite_file(&path).unwrap();

        assert_eq!(sqlite_schema_version(&path).unwrap(), 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn saves_and_loads_app_state() {
        let path = temp_sqlite_path("round-trip");
        let state = sample_state();

        save_app_to_sqlite_file(&state, &path).unwrap();
        let restored = load_app_from_sqlite_file(&path).unwrap();

        assert_eq!(
            restored.bank.account(100).unwrap().balance(),
            money("75.00")
        );
        assert_eq!(restored.audit_log.entries().len(), 1);
        assert!(
            restored
                .identities
                .authenticate("alice", "correct-password")
                .is_ok()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn loading_empty_database_reports_missing_state() {
        let path = temp_sqlite_path("missing-state");

        initialize_sqlite_file(&path).unwrap();
        let error = load_app_from_sqlite_file(&path).unwrap_err();

        assert!(matches!(error, SqliteStoreError::MissingState));
        fs::remove_file(path).unwrap();
    }
}
