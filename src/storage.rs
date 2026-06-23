use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    app::AppState,
    audit::{AuditEntry, AuditError, AuditLog},
    bank::{Bank, BankError},
    domain::Account,
    identity::{Customer, CustomerId, IdentityError, IdentityStore, User},
};

const CURRENT_BANK_STORAGE_VERSION: u32 = 1;
const CURRENT_APP_STORAGE_VERSION: u32 = 3;
const MIN_SUPPORTED_APP_STORAGE_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BankSnapshot {
    version: u32,
    accounts: Vec<Account>,
}

impl BankSnapshot {
    fn from_bank(bank: &Bank) -> Self {
        Self {
            version: CURRENT_BANK_STORAGE_VERSION,
            accounts: bank.accounts().cloned().collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AppSnapshot {
    version: u32,
    accounts: Vec<Account>,
    customers: Vec<Customer>,
    users: Vec<User>,
    #[serde(default)]
    audit_entries: Vec<AuditEntry>,
}

impl AppSnapshot {
    fn from_state(state: &AppState) -> Self {
        Self {
            version: CURRENT_APP_STORAGE_VERSION,
            accounts: state.bank.accounts().cloned().collect(),
            customers: state.identities.customers().cloned().collect(),
            users: state.identities.users().cloned().collect(),
            audit_entries: state.audit_log.entries().to_vec(),
        }
    }
}

#[derive(Debug)]
pub enum StorageError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Json(serde_json::Error),
    UnsupportedVersion(u32),
    InvalidBankState(BankError),
    InvalidIdentityState(IdentityError),
    InvalidAuditState(AuditError),
    InvalidAccountOwner {
        account_id: u32,
        customer_id: CustomerId,
    },
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {}", path.display(), source),
            Self::Json(source) => write!(formatter, "invalid JSON: {source}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported bank storage version {version}")
            }
            Self::InvalidBankState(source) => write!(formatter, "invalid bank state: {source}"),
            Self::InvalidIdentityState(source) => {
                write!(formatter, "invalid identity state: {source}")
            }
            Self::InvalidAuditState(source) => write!(formatter, "invalid audit state: {source}"),
            Self::InvalidAccountOwner {
                account_id,
                customer_id,
            } => write!(
                formatter,
                "account {account_id} references missing customer {customer_id}"
            ),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(source) => Some(source),
            Self::InvalidBankState(source) => Some(source),
            Self::InvalidIdentityState(source) => Some(source),
            Self::InvalidAuditState(source) => Some(source),
            Self::UnsupportedVersion(_) => None,
            Self::InvalidAccountOwner { .. } => None,
        }
    }
}

pub type StorageResult<T> = Result<T, StorageError>;

pub fn bank_to_json(bank: &Bank) -> StorageResult<String> {
    serde_json::to_string_pretty(&BankSnapshot::from_bank(bank)).map_err(StorageError::Json)
}

pub fn bank_from_json(input: &str) -> StorageResult<Bank> {
    let snapshot: BankSnapshot = serde_json::from_str(input).map_err(StorageError::Json)?;

    if snapshot.version != CURRENT_BANK_STORAGE_VERSION {
        return Err(StorageError::UnsupportedVersion(snapshot.version));
    }

    Bank::from_accounts(snapshot.accounts).map_err(StorageError::InvalidBankState)
}

pub fn app_to_json(state: &AppState) -> StorageResult<String> {
    serde_json::to_string_pretty(&AppSnapshot::from_state(state)).map_err(StorageError::Json)
}

pub fn app_from_json(input: &str) -> StorageResult<AppState> {
    let snapshot: AppSnapshot = serde_json::from_str(input).map_err(StorageError::Json)?;

    if !(MIN_SUPPORTED_APP_STORAGE_VERSION..=CURRENT_APP_STORAGE_VERSION)
        .contains(&snapshot.version)
    {
        return Err(StorageError::UnsupportedVersion(snapshot.version));
    }

    let identities = IdentityStore::from_records(snapshot.customers, snapshot.users)
        .map_err(StorageError::InvalidIdentityState)?;
    let bank = Bank::from_accounts(snapshot.accounts).map_err(StorageError::InvalidBankState)?;
    let audit_log =
        AuditLog::from_entries(snapshot.audit_entries).map_err(StorageError::InvalidAuditState)?;

    validate_account_owners(&bank, &identities)?;

    Ok(AppState::from_parts_with_audit(bank, identities, audit_log))
}

pub fn save_bank_to_json_file(bank: &Bank, path: impl AsRef<Path>) -> StorageResult<()> {
    let path = path.as_ref();
    let json = bank_to_json(bank)?;

    fs::write(path, json).map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub fn load_bank_from_json_file(path: impl AsRef<Path>) -> StorageResult<Bank> {
    let path = path.as_ref();
    let json = fs::read_to_string(path).map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    bank_from_json(&json)
}

pub fn save_app_to_json_file(state: &AppState, path: impl AsRef<Path>) -> StorageResult<()> {
    let path = path.as_ref();
    let json = app_to_json(state)?;

    fs::write(path, json).map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub fn load_app_from_json_file(path: impl AsRef<Path>) -> StorageResult<AppState> {
    let path = path.as_ref();
    let json = fs::read_to_string(path).map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    app_from_json(&json)
}

pub fn bank_statement_text(bank: &Bank) -> String {
    let mut output = String::new();

    for account in bank.accounts() {
        output.push_str(&format!("{}\n", account_summary(account)));

        for transaction in account.transactions() {
            output.push_str(&format!(
                "  - #{} | {} | {} | {} | {}\n",
                transaction.id(),
                transaction.occurred_at_epoch_seconds(),
                transaction.kind(),
                transaction.amount(),
                transaction.description()
            ));
        }
    }

    output
}

pub fn export_bank_statement_file(bank: &Bank, path: impl AsRef<Path>) -> StorageResult<()> {
    let path = path.as_ref();

    fs::write(path, bank_statement_text(bank)).map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn account_summary(account: &Account) -> String {
    format!(
        "ID: {}, Name: {}, Email: {}, Balance: {}, Loan: {}, Status: {}",
        account.id(),
        account.name(),
        account.email(),
        account.balance(),
        account.loan_balance(),
        account.status()
    )
}

fn validate_account_owners(bank: &Bank, identities: &IdentityStore) -> StorageResult<()> {
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        StorageError, app_from_json, app_to_json, bank_from_json, bank_to_json,
        load_bank_from_json_file, save_bank_to_json_file,
    };
    use crate::{
        app::AppState,
        audit::{AuditAction, AuditActor, AuditOutcome},
        bank::{Bank, BankError},
        domain::Money,
        identity::{IdentityStore, Role},
    };

    fn money(input: &str) -> Money {
        input.parse().unwrap()
    }

    fn sample_bank() -> Bank {
        let mut bank = Bank::new();
        bank.create_account(1, None, "Alice", "alice@example.com", money("100.00"))
            .unwrap();
        bank.create_account(2, None, "Bob", "bob@example.com", money("25.50"))
            .unwrap();
        bank.transfer(1, 2, money("10.25")).unwrap();
        bank.request_loan(2, money("50.00")).unwrap();
        bank
    }

    fn temp_json_path(name: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!("bank-{name}-{timestamp}.json"))
    }

    #[test]
    fn serializes_and_deserializes_bank_state() {
        let bank = sample_bank();
        let json = bank_to_json(&bank).unwrap();
        let restored = bank_from_json(&json).unwrap();

        assert_eq!(restored.account_count(), 2);
        assert_eq!(restored.account(1).unwrap().balance(), money("89.75"));
        assert_eq!(restored.account(2).unwrap().balance(), money("85.75"));
        assert_eq!(restored.account(2).unwrap().loan_balance(), money("50.00"));
        assert_eq!(
            restored.account(1).unwrap().transactions().len(),
            bank.account(1).unwrap().transactions().len()
        );
    }

    #[test]
    fn rejects_corrupted_json() {
        let error = bank_from_json("{ definitely-not-json }").unwrap_err();

        assert!(matches!(error, StorageError::Json(_)));
    }

    #[test]
    fn rejects_unsupported_storage_versions() {
        let error = bank_from_json(r#"{"version":999,"accounts":[]}"#).unwrap_err();

        assert!(matches!(error, StorageError::UnsupportedVersion(999)));
    }

    #[test]
    fn saves_and_loads_bank_state_from_file() {
        let bank = sample_bank();
        let path = temp_json_path("round-trip");

        save_bank_to_json_file(&bank, &path).unwrap();
        let restored = load_bank_from_json_file(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(restored.account_count(), 2);
        assert_eq!(restored.account(1).unwrap().balance(), money("89.75"));
    }

    #[test]
    fn rejects_duplicate_accounts_in_imported_state() {
        let json = r#"
        {
          "version": 1,
          "accounts": [
            {
              "id": 1,
              "name": "Alice",
              "email": "alice@example.com",
              "balance": 0,
              "loan_balance": 0,
              "status": "active",
              "transactions": []
            },
            {
              "id": 1,
              "name": "Alicia",
              "email": "alicia@example.com",
              "balance": 0,
              "loan_balance": 0,
              "status": "active",
              "transactions": []
            }
          ]
        }
        "#;

        let error = bank_from_json(json).unwrap_err();

        assert!(matches!(
            error,
            StorageError::InvalidBankState(BankError::AccountAlreadyExists(1))
        ));
    }

    #[test]
    fn serializes_and_deserializes_full_app_state() {
        let mut identities = IdentityStore::new();
        identities
            .create_customer(10, "Alice", "alice@example.com")
            .unwrap();
        identities
            .create_user(1, "alice", Role::Customer, Some(10), "correct-password")
            .unwrap();

        let mut bank = Bank::new();
        bank.create_account(
            1,
            Some(10),
            "Alice Checking",
            "alice@example.com",
            money("42.00"),
        )
        .unwrap();

        let mut state = AppState::from_parts(bank, identities);
        state
            .audit_log
            .record(
                AuditActor::System,
                AuditAction::CreateAccount,
                AuditOutcome::Success,
                Some("account:1".to_string()),
                "account created",
            )
            .unwrap();
        let json = app_to_json(&state).unwrap();
        let restored = app_from_json(&json).unwrap();

        assert_eq!(restored.bank.account(1).unwrap().owner_id(), Some(10));
        assert_eq!(restored.audit_log.entries().len(), 1);
        assert_eq!(
            restored
                .identities
                .authenticate("alice", "correct-password")
                .unwrap()
                .customer_id(),
            Some(10)
        );
    }

    #[test]
    fn rejects_accounts_with_missing_owners_in_app_state() {
        let json = r#"
        {
          "version": 2,
          "customers": [],
          "users": [],
          "accounts": [
            {
              "id": 1,
              "owner_id": 99,
              "name": "Orphan Account",
              "email": "orphan@example.com",
              "balance": 0,
              "loan_balance": 0,
              "status": "active",
              "transactions": []
            }
          ]
        }
        "#;

        let error = app_from_json(json).unwrap_err();

        assert!(matches!(
            error,
            StorageError::InvalidAccountOwner {
                account_id: 1,
                customer_id: 99
            }
        ));
    }
}
