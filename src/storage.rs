use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    bank::{Bank, BankError},
    domain::Account,
};

const CURRENT_STORAGE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BankSnapshot {
    version: u32,
    accounts: Vec<Account>,
}

impl BankSnapshot {
    fn from_bank(bank: &Bank) -> Self {
        Self {
            version: CURRENT_STORAGE_VERSION,
            accounts: bank.accounts().cloned().collect(),
        }
    }
}

#[derive(Debug)]
pub enum StorageError {
    Io { path: PathBuf, source: io::Error },
    Json(serde_json::Error),
    UnsupportedVersion(u32),
    InvalidBankState(BankError),
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
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(source) => Some(source),
            Self::InvalidBankState(source) => Some(source),
            Self::UnsupportedVersion(_) => None,
        }
    }
}

pub type StorageResult<T> = Result<T, StorageError>;

pub fn bank_to_json(bank: &Bank) -> StorageResult<String> {
    serde_json::to_string_pretty(&BankSnapshot::from_bank(bank)).map_err(StorageError::Json)
}

pub fn bank_from_json(input: &str) -> StorageResult<Bank> {
    let snapshot: BankSnapshot = serde_json::from_str(input).map_err(StorageError::Json)?;

    if snapshot.version != CURRENT_STORAGE_VERSION {
        return Err(StorageError::UnsupportedVersion(snapshot.version));
    }

    Bank::from_accounts(snapshot.accounts).map_err(StorageError::InvalidBankState)
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

pub fn bank_statement_text(bank: &Bank) -> String {
    let mut output = String::new();

    for account in bank.accounts() {
        output.push_str(&format!("{}\n", account_summary(account)));

        for transaction in account.transactions() {
            output.push_str(&format!(
                "  - {} | {} | {}\n",
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        StorageError, bank_from_json, bank_to_json, load_bank_from_json_file,
        save_bank_to_json_file,
    };
    use crate::{
        bank::{Bank, BankError},
        domain::Money,
    };

    fn money(input: &str) -> Money {
        input.parse().unwrap()
    }

    fn sample_bank() -> Bank {
        let mut bank = Bank::new();
        bank.create_account(1, "Alice", "alice@example.com", money("100.00"))
            .unwrap();
        bank.create_account(2, "Bob", "bob@example.com", money("25.50"))
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
}
