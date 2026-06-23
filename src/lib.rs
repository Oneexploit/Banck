pub mod bank;
pub mod cli;
pub mod domain;
pub mod storage;

pub use bank::{Bank, BankError};
pub use domain::{
    Account, AccountId, AccountStatus, InterestRate, Money, Transaction, TransactionKind,
};
