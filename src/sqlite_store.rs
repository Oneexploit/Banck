use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, Transaction as SqliteTransaction, params};

use crate::{
    app::AppState,
    audit::{AuditAction, AuditActor, AuditEntry, AuditLog, AuditOutcome},
    bank::Bank,
    domain::{
        Account, AccountId, AccountStatus, Money, Transaction, TransactionKind,
        current_epoch_seconds,
    },
    identity::{Customer, IdentityStore, Role, User, UserId},
    storage::{StorageError, app_from_json, app_to_json},
};

const CURRENT_SQLITE_SCHEMA_VERSION: i64 = 3;

#[derive(Debug)]
pub enum SqliteStoreError {
    Sqlite {
        path: PathBuf,
        source: rusqlite::Error,
    },
    Storage(StorageError),
    MissingState,
    UnsupportedSchemaVersion(i64),
    InvalidInteger {
        field: &'static str,
        value: i64,
    },
    IntegerOverflow {
        field: &'static str,
        value: u64,
    },
    InvalidEnumValue {
        field: &'static str,
        value: String,
    },
    MissingUserActorFields {
        audit_id: u64,
    },
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
            Self::InvalidInteger { field, value } => {
                write!(formatter, "{field} contains invalid integer {value}")
            }
            Self::IntegerOverflow { field, value } => {
                write!(
                    formatter,
                    "{field} value {value} cannot fit in sqlite INTEGER"
                )
            }
            Self::InvalidEnumValue { field, value } => {
                write!(formatter, "{field} contains invalid enum value {value}")
            }
            Self::MissingUserActorFields { audit_id } => {
                write!(
                    formatter,
                    "audit entry {audit_id} has incomplete user actor fields"
                )
            }
        }
    }
}

impl Error for SqliteStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite { source, .. } => Some(source),
            Self::Storage(source) => Some(source),
            Self::MissingState
            | Self::UnsupportedSchemaVersion(_)
            | Self::InvalidInteger { .. }
            | Self::IntegerOverflow { .. }
            | Self::InvalidEnumValue { .. }
            | Self::MissingUserActorFields { .. } => None,
        }
    }
}

pub type SqliteStoreResult<T> = Result<T, SqliteStoreError>;

pub fn initialize_sqlite_file(path: impl AsRef<Path>) -> SqliteStoreResult<()> {
    let path = path.as_ref();
    let mut connection = open_connection(path)?;
    migrate(&mut connection, path)
}

pub fn save_app_to_sqlite_file(state: &AppState, path: impl AsRef<Path>) -> SqliteStoreResult<()> {
    let path = path.as_ref();
    let mut connection = open_connection(path)?;
    migrate(&mut connection, path)?;

    let transaction = connection
        .transaction()
        .map_err(|source| sqlite_error(path, source))?;
    write_normalized_state(&transaction, path, state)?;
    write_snapshot_state(&transaction, path, state)?;
    transaction
        .commit()
        .map_err(|source| sqlite_error(path, source))?;

    Ok(())
}

pub fn load_app_from_sqlite_file(path: impl AsRef<Path>) -> SqliteStoreResult<AppState> {
    let path = path.as_ref();
    let mut connection = open_connection(path)?;
    migrate(&mut connection, path)?;

    if let Some(state) = read_normalized_state(&connection, path)? {
        return Ok(state);
    }

    read_snapshot_state(&connection, path)?.ok_or(SqliteStoreError::MissingState)
}

pub fn sqlite_schema_version(path: impl AsRef<Path>) -> SqliteStoreResult<i64> {
    let path = path.as_ref();
    let mut connection = open_connection(path)?;
    migrate(&mut connection, path)?;
    current_schema_version(&connection, path)
}

fn open_connection(path: &Path) -> SqliteStoreResult<Connection> {
    let connection = Connection::open(path).map_err(|source| sqlite_error(path, source))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|source| sqlite_error(path, source))?;

    Ok(connection)
}

fn migrate(connection: &mut Connection, path: &Path) -> SqliteStoreResult<()> {
    ensure_migration_table(connection, path)?;
    let version = current_schema_version(connection, path)?;

    if version > CURRENT_SQLITE_SCHEMA_VERSION {
        return Err(SqliteStoreError::UnsupportedSchemaVersion(version));
    }

    if version < 1 {
        apply_migration_1(connection, path)?;
    }

    if version < 2 {
        apply_migration_2(connection, path)?;
    }

    if version < 3 {
        apply_migration_3(connection, path)?;
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
            params![1_i64, "create_app_state_snapshot", current_epoch_i64()?],
        )
        .map_err(|source| sqlite_error(path, source))?;

    Ok(())
}

fn apply_migration_2(connection: &mut Connection, path: &Path) -> SqliteStoreResult<()> {
    let snapshot_state = read_snapshot_state(connection, path)?;
    let transaction = connection
        .transaction()
        .map_err(|source| sqlite_error(path, source))?;

    transaction
        .execute_batch(
            "CREATE TABLE customers (
                id INTEGER PRIMARY KEY,
                full_name TEXT NOT NULL,
                email TEXT NOT NULL
            );
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                role TEXT NOT NULL,
                customer_id INTEGER NULL,
                password_hash TEXT NOT NULL,
                FOREIGN KEY(customer_id) REFERENCES customers(id)
            );
            CREATE TABLE accounts (
                id INTEGER PRIMARY KEY,
                owner_id INTEGER NULL,
                name TEXT NOT NULL,
                email TEXT NOT NULL,
                balance_cents INTEGER NOT NULL,
                loan_balance_cents INTEGER NOT NULL,
                status TEXT NOT NULL,
                FOREIGN KEY(owner_id) REFERENCES customers(id)
            );
            CREATE TABLE transactions (
                id INTEGER PRIMARY KEY,
                account_id INTEGER NOT NULL,
                occurred_at_epoch_seconds INTEGER NOT NULL,
                kind TEXT NOT NULL,
                amount_cents INTEGER NOT NULL,
                description TEXT NOT NULL,
                FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE
            );
            CREATE TABLE audit_entries (
                id INTEGER PRIMARY KEY,
                occurred_at_epoch_seconds INTEGER NOT NULL,
                actor_kind TEXT NOT NULL,
                actor_user_id INTEGER NULL,
                actor_username TEXT NULL,
                actor_role TEXT NULL,
                action TEXT NOT NULL,
                outcome TEXT NOT NULL,
                target TEXT NULL,
                message TEXT NOT NULL
            );",
        )
        .map_err(|source| sqlite_error(path, source))?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, name, applied_at_epoch_seconds)
             VALUES (?1, ?2, ?3)",
            params![
                2_i64,
                "create_normalized_state_tables",
                current_epoch_i64()?
            ],
        )
        .map_err(|source| sqlite_error(path, source))?;

    if let Some(state) = snapshot_state.as_ref() {
        write_normalized_state(&transaction, path, state)?;
    }

    transaction
        .commit()
        .map_err(|source| sqlite_error(path, source))?;

    Ok(())
}

fn apply_migration_3(connection: &Connection, path: &Path) -> SqliteStoreResult<()> {
    connection
        .execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_accounts_owner_id
                ON accounts(owner_id);
            CREATE INDEX IF NOT EXISTS idx_transactions_account_time
                ON transactions(account_id, occurred_at_epoch_seconds DESC, id DESC);
            CREATE INDEX IF NOT EXISTS idx_transactions_kind
                ON transactions(kind);
            CREATE INDEX IF NOT EXISTS idx_audit_entries_action_time
                ON audit_entries(action, occurred_at_epoch_seconds DESC, id DESC);
            CREATE INDEX IF NOT EXISTS idx_audit_entries_outcome_time
                ON audit_entries(outcome, occurred_at_epoch_seconds DESC, id DESC);
            CREATE INDEX IF NOT EXISTS idx_audit_entries_target
                ON audit_entries(target);",
        )
        .map_err(|source| sqlite_error(path, source))?;
    connection
        .execute(
            "INSERT INTO schema_migrations (version, name, applied_at_epoch_seconds)
             VALUES (?1, ?2, ?3)",
            params![3_i64, "add_query_indexes", current_epoch_i64()?],
        )
        .map_err(|source| sqlite_error(path, source))?;

    Ok(())
}

fn write_snapshot_state(
    transaction: &SqliteTransaction<'_>,
    path: &Path,
    state: &AppState,
) -> SqliteStoreResult<()> {
    let json = app_to_json(state).map_err(SqliteStoreError::Storage)?;
    transaction
        .execute(
            "INSERT INTO app_state (id, json, updated_at_epoch_seconds)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                json = excluded.json,
                updated_at_epoch_seconds = excluded.updated_at_epoch_seconds",
            params![json, current_epoch_i64()?],
        )
        .map_err(|source| sqlite_error(path, source))?;

    Ok(())
}

fn write_normalized_state(
    transaction: &SqliteTransaction<'_>,
    path: &Path,
    state: &AppState,
) -> SqliteStoreResult<()> {
    transaction
        .execute_batch(
            "DELETE FROM transactions;
            DELETE FROM accounts;
            DELETE FROM users;
            DELETE FROM customers;
            DELETE FROM audit_entries;",
        )
        .map_err(|source| sqlite_error(path, source))?;

    for customer in state.identities.customers() {
        transaction
            .execute(
                "INSERT INTO customers (id, full_name, email)
                 VALUES (?1, ?2, ?3)",
                params![
                    i64::from(customer.id()),
                    customer.full_name(),
                    customer.email()
                ],
            )
            .map_err(|source| sqlite_error(path, source))?;
    }

    for user in state.identities.users() {
        transaction
            .execute(
                "INSERT INTO users (id, username, role, customer_id, password_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    i64::from(user.id()),
                    user.username(),
                    user.role().to_string(),
                    user.customer_id().map(i64::from),
                    user.password_hash()
                ],
            )
            .map_err(|source| sqlite_error(path, source))?;
    }

    for account in state.bank.accounts() {
        transaction
            .execute(
                "INSERT INTO accounts (
                    id,
                    owner_id,
                    name,
                    email,
                    balance_cents,
                    loan_balance_cents,
                    status
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    i64::from(account.id()),
                    account.owner_id().map(i64::from),
                    account.name(),
                    account.email(),
                    account.balance().cents(),
                    account.loan_balance().cents(),
                    account.status().to_string(),
                ],
            )
            .map_err(|source| sqlite_error(path, source))?;

        for ledger_transaction in account.transactions() {
            transaction
                .execute(
                    "INSERT INTO transactions (
                        id,
                        account_id,
                        occurred_at_epoch_seconds,
                        kind,
                        amount_cents,
                        description
                     )
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        u64_to_i64("transactions.id", ledger_transaction.id())?,
                        i64::from(account.id()),
                        u64_to_i64(
                            "transactions.occurred_at_epoch_seconds",
                            ledger_transaction.occurred_at_epoch_seconds()
                        )?,
                        ledger_transaction.kind().to_string(),
                        ledger_transaction.amount().cents(),
                        ledger_transaction.description(),
                    ],
                )
                .map_err(|source| sqlite_error(path, source))?;
        }
    }

    for entry in state.audit_log.entries() {
        let (actor_kind, actor_user_id, actor_username, actor_role) = actor_to_db(entry.actor());
        transaction
            .execute(
                "INSERT INTO audit_entries (
                    id,
                    occurred_at_epoch_seconds,
                    actor_kind,
                    actor_user_id,
                    actor_username,
                    actor_role,
                    action,
                    outcome,
                    target,
                    message
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    u64_to_i64("audit_entries.id", entry.id())?,
                    u64_to_i64(
                        "audit_entries.occurred_at_epoch_seconds",
                        entry.occurred_at_epoch_seconds()
                    )?,
                    actor_kind,
                    actor_user_id,
                    actor_username,
                    actor_role,
                    entry.action().to_string(),
                    entry.outcome().to_string(),
                    entry.target(),
                    entry.message(),
                ],
            )
            .map_err(|source| sqlite_error(path, source))?;
    }

    Ok(())
}

fn read_normalized_state(
    connection: &Connection,
    path: &Path,
) -> SqliteStoreResult<Option<AppState>> {
    if !has_normalized_rows(connection, path)? {
        return Ok(None);
    }

    let customers = read_customers(connection, path)?;
    let users = read_users(connection, path)?;
    let identities = IdentityStore::from_records(customers, users)
        .map_err(StorageError::InvalidIdentityState)
        .map_err(SqliteStoreError::Storage)?;
    let bank = read_bank(connection, path)?;
    let audit_log = read_audit_log(connection, path)?;

    validate_account_owners(&bank, &identities).map_err(SqliteStoreError::Storage)?;

    Ok(Some(AppState::from_parts_with_audit(
        bank, identities, audit_log,
    )))
}

fn read_snapshot_state(
    connection: &Connection,
    path: &Path,
) -> SqliteStoreResult<Option<AppState>> {
    let json = connection
        .query_row("SELECT json FROM app_state WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|source| sqlite_error(path, source))?;

    json.map(|json| app_from_json(&json).map_err(SqliteStoreError::Storage))
        .transpose()
}

fn has_normalized_rows(connection: &Connection, path: &Path) -> SqliteStoreResult<bool> {
    let count = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM customers)
                + (SELECT COUNT(*) FROM users)
                + (SELECT COUNT(*) FROM accounts)
                + (SELECT COUNT(*) FROM transactions)
                + (SELECT COUNT(*) FROM audit_entries)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|source| sqlite_error(path, source))?;

    Ok(count > 0)
}

#[derive(Debug)]
struct CustomerRecord {
    id: i64,
    full_name: String,
    email: String,
}

fn read_customers(connection: &Connection, path: &Path) -> SqliteStoreResult<Vec<Customer>> {
    let records = {
        let mut statement = connection
            .prepare("SELECT id, full_name, email FROM customers ORDER BY id")
            .map_err(|source| sqlite_error(path, source))?;
        let rows = statement
            .query_map([], |row| {
                Ok(CustomerRecord {
                    id: row.get(0)?,
                    full_name: row.get(1)?,
                    email: row.get(2)?,
                })
            })
            .map_err(|source| sqlite_error(path, source))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|source| sqlite_error(path, source))?
    };

    records
        .into_iter()
        .map(|record| {
            Ok(Customer::from_persisted(
                i64_to_u32("customers.id", record.id)?,
                record.full_name,
                record.email,
            ))
        })
        .collect()
}

#[derive(Debug)]
struct UserRecord {
    id: i64,
    username: String,
    role: String,
    customer_id: Option<i64>,
    password_hash: String,
}

fn read_users(connection: &Connection, path: &Path) -> SqliteStoreResult<Vec<User>> {
    let records = {
        let mut statement = connection
            .prepare("SELECT id, username, role, customer_id, password_hash FROM users ORDER BY id")
            .map_err(|source| sqlite_error(path, source))?;
        let rows = statement
            .query_map([], |row| {
                Ok(UserRecord {
                    id: row.get(0)?,
                    username: row.get(1)?,
                    role: row.get(2)?,
                    customer_id: row.get(3)?,
                    password_hash: row.get(4)?,
                })
            })
            .map_err(|source| sqlite_error(path, source))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|source| sqlite_error(path, source))?
    };

    records
        .into_iter()
        .map(|record| {
            Ok(User::from_persisted(
                i64_to_u32("users.id", record.id)?,
                record.username,
                role_from_db("users.role", &record.role)?,
                optional_i64_to_u32("users.customer_id", record.customer_id)?,
                record.password_hash,
            ))
        })
        .collect()
}

#[derive(Debug)]
struct AccountRecord {
    id: i64,
    owner_id: Option<i64>,
    name: String,
    email: String,
    balance_cents: i64,
    loan_balance_cents: i64,
    status: String,
}

fn read_bank(connection: &Connection, path: &Path) -> SqliteStoreResult<Bank> {
    let account_records = {
        let mut statement = connection
            .prepare(
                "SELECT id, owner_id, name, email, balance_cents, loan_balance_cents, status
                 FROM accounts
                 ORDER BY id",
            )
            .map_err(|source| sqlite_error(path, source))?;
        let rows = statement
            .query_map([], |row| {
                Ok(AccountRecord {
                    id: row.get(0)?,
                    owner_id: row.get(1)?,
                    name: row.get(2)?,
                    email: row.get(3)?,
                    balance_cents: row.get(4)?,
                    loan_balance_cents: row.get(5)?,
                    status: row.get(6)?,
                })
            })
            .map_err(|source| sqlite_error(path, source))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|source| sqlite_error(path, source))?
    };
    let mut accounts = Vec::new();

    for record in account_records {
        let account_id = i64_to_u32("accounts.id", record.id)?;
        let transactions = read_account_transactions(connection, path, account_id)?;
        accounts.push(Account {
            id: account_id,
            owner_id: optional_i64_to_u32("accounts.owner_id", record.owner_id)?,
            name: record.name,
            email: record.email,
            balance: Money::from_cents(record.balance_cents),
            loan_balance: Money::from_cents(record.loan_balance_cents),
            status: account_status_from_db("accounts.status", &record.status)?,
            transactions,
        });
    }

    Bank::from_accounts(accounts)
        .map_err(StorageError::InvalidBankState)
        .map_err(SqliteStoreError::Storage)
}

#[derive(Debug)]
struct TransactionRecord {
    id: i64,
    occurred_at_epoch_seconds: i64,
    kind: String,
    amount_cents: i64,
    description: String,
}

fn read_account_transactions(
    connection: &Connection,
    path: &Path,
    account_id: AccountId,
) -> SqliteStoreResult<Vec<Transaction>> {
    let records = {
        let mut statement = connection
            .prepare(
                "SELECT id, occurred_at_epoch_seconds, kind, amount_cents, description
                 FROM transactions
                 WHERE account_id = ?1
                 ORDER BY id",
            )
            .map_err(|source| sqlite_error(path, source))?;
        let rows = statement
            .query_map(params![i64::from(account_id)], |row| {
                Ok(TransactionRecord {
                    id: row.get(0)?,
                    occurred_at_epoch_seconds: row.get(1)?,
                    kind: row.get(2)?,
                    amount_cents: row.get(3)?,
                    description: row.get(4)?,
                })
            })
            .map_err(|source| sqlite_error(path, source))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|source| sqlite_error(path, source))?
    };

    records
        .into_iter()
        .map(|record| {
            Ok(Transaction::new(
                i64_to_u64("transactions.id", record.id)?,
                i64_to_u64(
                    "transactions.occurred_at_epoch_seconds",
                    record.occurred_at_epoch_seconds,
                )?,
                transaction_kind_from_db("transactions.kind", &record.kind)?,
                Money::from_cents(record.amount_cents),
                record.description,
            ))
        })
        .collect()
}

#[derive(Debug)]
struct AuditRecord {
    id: i64,
    occurred_at_epoch_seconds: i64,
    actor_kind: String,
    actor_user_id: Option<i64>,
    actor_username: Option<String>,
    actor_role: Option<String>,
    action: String,
    outcome: String,
    target: Option<String>,
    message: String,
}

fn read_audit_log(connection: &Connection, path: &Path) -> SqliteStoreResult<AuditLog> {
    let records = {
        let mut statement = connection
            .prepare(
                "SELECT
                    id,
                    occurred_at_epoch_seconds,
                    actor_kind,
                    actor_user_id,
                    actor_username,
                    actor_role,
                    action,
                    outcome,
                    target,
                    message
                 FROM audit_entries
                 ORDER BY id",
            )
            .map_err(|source| sqlite_error(path, source))?;
        let rows = statement
            .query_map([], |row| {
                Ok(AuditRecord {
                    id: row.get(0)?,
                    occurred_at_epoch_seconds: row.get(1)?,
                    actor_kind: row.get(2)?,
                    actor_user_id: row.get(3)?,
                    actor_username: row.get(4)?,
                    actor_role: row.get(5)?,
                    action: row.get(6)?,
                    outcome: row.get(7)?,
                    target: row.get(8)?,
                    message: row.get(9)?,
                })
            })
            .map_err(|source| sqlite_error(path, source))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|source| sqlite_error(path, source))?
    };
    let mut entries = Vec::new();

    for record in records {
        let audit_id = i64_to_u64("audit_entries.id", record.id)?;
        let actor = actor_from_db(
            audit_id,
            &record.actor_kind,
            record.actor_user_id,
            record.actor_username,
            record.actor_role,
        )?;
        entries.push(AuditEntry::new(
            audit_id,
            i64_to_u64(
                "audit_entries.occurred_at_epoch_seconds",
                record.occurred_at_epoch_seconds,
            )?,
            actor,
            audit_action_from_db("audit_entries.action", &record.action)?,
            audit_outcome_from_db("audit_entries.outcome", &record.outcome)?,
            record.target,
            record.message,
        ));
    }

    AuditLog::from_entries(entries)
        .map_err(StorageError::InvalidAuditState)
        .map_err(SqliteStoreError::Storage)
}

fn validate_account_owners(bank: &Bank, identities: &IdentityStore) -> Result<(), StorageError> {
    for account in bank.accounts() {
        if let Some(customer_id) = account.owner_id() {
            identities
                .customer(customer_id)
                .map_err(|_| StorageError::InvalidAccountOwner {
                    account_id: account.id(),
                    customer_id,
                })?;
        }
    }

    Ok(())
}

fn actor_to_db(actor: &AuditActor) -> (&'static str, Option<i64>, Option<&str>, Option<String>) {
    match actor {
        AuditActor::System => ("system", None, None, None),
        AuditActor::User {
            user_id,
            username,
            role,
        } => (
            "user",
            Some(i64::from(*user_id)),
            Some(username.as_str()),
            Some(role.to_string()),
        ),
    }
}

fn actor_from_db(
    audit_id: u64,
    actor_kind: &str,
    user_id: Option<i64>,
    username: Option<String>,
    role: Option<String>,
) -> SqliteStoreResult<AuditActor> {
    match actor_kind {
        "system" => Ok(AuditActor::System),
        "user" => {
            let user_id = user_id.ok_or(SqliteStoreError::MissingUserActorFields { audit_id })?;
            let username = username.ok_or(SqliteStoreError::MissingUserActorFields { audit_id })?;
            let role = role.ok_or(SqliteStoreError::MissingUserActorFields { audit_id })?;

            Ok(AuditActor::User {
                user_id: i64_to_u32("audit_entries.actor_user_id", user_id)? as UserId,
                username,
                role: role_from_db("audit_entries.actor_role", &role)?,
            })
        }
        value => Err(SqliteStoreError::InvalidEnumValue {
            field: "audit_entries.actor_kind",
            value: value.to_string(),
        }),
    }
}

fn role_from_db(field: &'static str, value: &str) -> SqliteStoreResult<Role> {
    match value {
        "customer" => Ok(Role::Customer),
        "teller" => Ok(Role::Teller),
        "admin" => Ok(Role::Admin),
        value => Err(SqliteStoreError::InvalidEnumValue {
            field,
            value: value.to_string(),
        }),
    }
}

fn account_status_from_db(field: &'static str, value: &str) -> SqliteStoreResult<AccountStatus> {
    match value {
        "active" => Ok(AccountStatus::Active),
        "inactive" => Ok(AccountStatus::Inactive),
        "closed" => Ok(AccountStatus::Closed),
        value => Err(SqliteStoreError::InvalidEnumValue {
            field,
            value: value.to_string(),
        }),
    }
}

fn transaction_kind_from_db(
    field: &'static str,
    value: &str,
) -> SqliteStoreResult<TransactionKind> {
    match value {
        "account_created" => Ok(TransactionKind::AccountCreated),
        "deposit" => Ok(TransactionKind::Deposit),
        "withdrawal" => Ok(TransactionKind::Withdrawal),
        "transfer_in" => Ok(TransactionKind::TransferIn),
        "transfer_out" => Ok(TransactionKind::TransferOut),
        "fee" => Ok(TransactionKind::Fee),
        "interest" => Ok(TransactionKind::Interest),
        "loan_requested" => Ok(TransactionKind::LoanRequested),
        "loan_payment" => Ok(TransactionKind::LoanPayment),
        "profile_updated" => Ok(TransactionKind::ProfileUpdated),
        "activated" => Ok(TransactionKind::Activated),
        "deactivated" => Ok(TransactionKind::Deactivated),
        "closed" => Ok(TransactionKind::Closed),
        value => Err(SqliteStoreError::InvalidEnumValue {
            field,
            value: value.to_string(),
        }),
    }
}

fn audit_action_from_db(field: &'static str, value: &str) -> SqliteStoreResult<AuditAction> {
    match value {
        "bootstrap_admin" => Ok(AuditAction::BootstrapAdmin),
        "login" => Ok(AuditAction::Login),
        "create_customer" => Ok(AuditAction::CreateCustomer),
        "create_user" => Ok(AuditAction::CreateUser),
        "create_account" => Ok(AuditAction::CreateAccount),
        "update_account_profile" => Ok(AuditAction::UpdateAccountProfile),
        "activate_account" => Ok(AuditAction::ActivateAccount),
        "deactivate_account" => Ok(AuditAction::DeactivateAccount),
        "close_account" => Ok(AuditAction::CloseAccount),
        "delete_account" => Ok(AuditAction::DeleteAccount),
        "deposit" => Ok(AuditAction::Deposit),
        "withdraw" => Ok(AuditAction::Withdraw),
        "transfer" => Ok(AuditAction::Transfer),
        "fee" => Ok(AuditAction::Fee),
        "interest" => Ok(AuditAction::Interest),
        "loan_request" => Ok(AuditAction::LoanRequest),
        "loan_payment" => Ok(AuditAction::LoanPayment),
        "save_state" => Ok(AuditAction::SaveState),
        "load_state" => Ok(AuditAction::LoadState),
        "export_statement" => Ok(AuditAction::ExportStatement),
        value => Err(SqliteStoreError::InvalidEnumValue {
            field,
            value: value.to_string(),
        }),
    }
}

fn audit_outcome_from_db(field: &'static str, value: &str) -> SqliteStoreResult<AuditOutcome> {
    match value {
        "success" => Ok(AuditOutcome::Success),
        "failure" => Ok(AuditOutcome::Failure),
        value => Err(SqliteStoreError::InvalidEnumValue {
            field,
            value: value.to_string(),
        }),
    }
}

fn optional_i64_to_u32(field: &'static str, value: Option<i64>) -> SqliteStoreResult<Option<u32>> {
    value.map(|value| i64_to_u32(field, value)).transpose()
}

fn i64_to_u32(field: &'static str, value: i64) -> SqliteStoreResult<u32> {
    u32::try_from(value).map_err(|_| SqliteStoreError::InvalidInteger { field, value })
}

fn i64_to_u64(field: &'static str, value: i64) -> SqliteStoreResult<u64> {
    u64::try_from(value).map_err(|_| SqliteStoreError::InvalidInteger { field, value })
}

fn u64_to_i64(field: &'static str, value: u64) -> SqliteStoreResult<i64> {
    i64::try_from(value).map_err(|_| SqliteStoreError::IntegerOverflow { field, value })
}

fn current_epoch_i64() -> SqliteStoreResult<i64> {
    u64_to_i64("current_epoch_seconds", current_epoch_seconds())
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
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use rusqlite::{Connection, params};

    use crate::{
        AppState,
        audit::{AuditAction, AuditActor, AuditOutcome},
        bank::Bank,
        domain::Money,
        identity::{IdentityStore, Role},
        storage::app_to_json,
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

    fn create_v1_snapshot_database(path: &Path, state: &AppState) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at_epoch_seconds INTEGER NOT NULL
                );
                CREATE TABLE app_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    json TEXT NOT NULL,
                    updated_at_epoch_seconds INTEGER NOT NULL
                );
                CREATE TABLE app_metadata (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO schema_migrations (version, name, applied_at_epoch_seconds)
                 VALUES (1, 'create_app_state_snapshot', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO app_state (id, json, updated_at_epoch_seconds)
                 VALUES (1, ?1, 1)",
                params![app_to_json(state).unwrap()],
            )
            .unwrap();
    }

    fn table_count(path: &Path, table_name: &str) -> i64 {
        let connection = Connection::open(path).unwrap();
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table_name}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn index_exists(path: &Path, index_name: &str) -> bool {
        let connection = Connection::open(path).unwrap();
        connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM sqlite_master
                    WHERE type = 'index' AND name = ?1
                )",
                params![index_name],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    }

    #[test]
    fn initializes_schema_migrations() {
        let path = temp_sqlite_path("migration");

        initialize_sqlite_file(&path).unwrap();

        assert_eq!(sqlite_schema_version(&path).unwrap(), 3);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn initializes_query_indexes() {
        let path = temp_sqlite_path("indexes");

        initialize_sqlite_file(&path).unwrap();

        assert!(index_exists(&path, "idx_accounts_owner_id"));
        assert!(index_exists(&path, "idx_transactions_account_time"));
        assert!(index_exists(&path, "idx_transactions_kind"));
        assert!(index_exists(&path, "idx_audit_entries_action_time"));
        assert!(index_exists(&path, "idx_audit_entries_outcome_time"));
        assert!(index_exists(&path, "idx_audit_entries_target"));
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
    fn saves_state_to_normalized_tables() {
        let path = temp_sqlite_path("normalized-save");
        let state = sample_state();

        save_app_to_sqlite_file(&state, &path).unwrap();

        assert_eq!(table_count(&path, "customers"), 1);
        assert_eq!(table_count(&path, "users"), 1);
        assert_eq!(table_count(&path, "accounts"), 1);
        assert_eq!(table_count(&path, "transactions"), 1);
        assert_eq!(table_count(&path, "audit_entries"), 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn loads_normalized_tables_before_snapshot() {
        let path = temp_sqlite_path("normalized-first");
        let state = sample_state();

        save_app_to_sqlite_file(&state, &path).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE app_state SET json = ?1 WHERE id = 1",
                params!["not valid json"],
            )
            .unwrap();
        drop(connection);

        let restored = load_app_from_sqlite_file(&path).unwrap();

        assert_eq!(
            restored.bank.account(100).unwrap().balance(),
            money("75.00")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn migrates_v1_snapshot_to_normalized_tables() {
        let path = temp_sqlite_path("v1-migration");
        let state = sample_state();
        create_v1_snapshot_database(&path, &state);

        assert_eq!(sqlite_schema_version(&path).unwrap(), 3);
        assert_eq!(table_count(&path, "customers"), 1);
        assert_eq!(table_count(&path, "users"), 1);
        assert_eq!(table_count(&path, "accounts"), 1);
        assert_eq!(table_count(&path, "transactions"), 1);
        assert_eq!(table_count(&path, "audit_entries"), 1);

        let restored = load_app_from_sqlite_file(&path).unwrap();
        assert_eq!(
            restored.bank.account(100).unwrap().balance(),
            money("75.00")
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
