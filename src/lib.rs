pub mod app;
pub mod bank;
pub mod cli;
pub mod domain;
pub mod identity;
pub mod storage;

pub use app::AppState;
pub use bank::{Bank, BankError};
pub use domain::{
    Account, AccountId, AccountStatus, InterestRate, Money, Transaction, TransactionKind,
};
pub use identity::{
    Customer, CustomerId, IdentityError, IdentityStore, Permission, Role, Session, User, UserId,
};
