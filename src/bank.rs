use std::{collections::BTreeMap, error::Error, fmt};

use crate::domain::{
    Account, AccountId, AccountStatus, InterestRate, Money, Transaction, TransactionKind,
};

pub type BankResult<T> = Result<T, BankError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BankError {
    AccountAlreadyExists(AccountId),
    AccountNotFound(AccountId),
    SameAccountTransfer(AccountId),
    InvalidAmount,
    InvalidProfileField(&'static str),
    AccountInactive(AccountId),
    AccountClosed(AccountId),
    InsufficientFunds {
        account_id: AccountId,
        available: Money,
        required: Money,
    },
    AccountHasBalance {
        account_id: AccountId,
        balance: Money,
    },
    AccountHasLoan {
        account_id: AccountId,
        loan_balance: Money,
    },
    NoLoan(AccountId),
    ArithmeticOverflow,
}

impl fmt::Display for BankError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccountAlreadyExists(id) => write!(formatter, "account {id} already exists"),
            Self::AccountNotFound(id) => write!(formatter, "account {id} was not found"),
            Self::SameAccountTransfer(id) => {
                write!(formatter, "cannot transfer from account {id} to itself")
            }
            Self::InvalidAmount => write!(formatter, "amount must be positive"),
            Self::InvalidProfileField(field) => write!(formatter, "{field} is invalid"),
            Self::AccountInactive(id) => write!(formatter, "account {id} is inactive"),
            Self::AccountClosed(id) => write!(formatter, "account {id} is closed"),
            Self::InsufficientFunds {
                account_id,
                available,
                required,
            } => write!(
                formatter,
                "account {account_id} has insufficient funds: available {available}, required {required}"
            ),
            Self::AccountHasBalance {
                account_id,
                balance,
            } => write!(
                formatter,
                "account {account_id} must have zero balance before this operation; current balance is {balance}"
            ),
            Self::AccountHasLoan {
                account_id,
                loan_balance,
            } => write!(
                formatter,
                "account {account_id} must repay loans before this operation; current loan balance is {loan_balance}"
            ),
            Self::NoLoan(id) => write!(formatter, "account {id} has no outstanding loan"),
            Self::ArithmeticOverflow => write!(formatter, "calculation overflowed"),
        }
    }
}

impl Error for BankError {}

#[derive(Debug, Default)]
pub struct Bank {
    accounts: BTreeMap<AccountId, Account>,
}

impl Bank {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.values()
    }

    pub fn create_account(
        &mut self,
        id: AccountId,
        name: impl Into<String>,
        email: impl Into<String>,
        opening_balance: Money,
    ) -> BankResult<&Account> {
        if self.accounts.contains_key(&id) {
            return Err(BankError::AccountAlreadyExists(id));
        }

        ensure_non_negative(opening_balance)?;

        let name = clean_name(name.into())?;
        let email = clean_email(email.into())?;
        let account = Account::new(id, name, email, opening_balance);

        self.accounts.insert(id, account);
        Ok(self.accounts.get(&id).expect("inserted account must exist"))
    }

    pub fn account(&self, id: AccountId) -> BankResult<&Account> {
        self.accounts.get(&id).ok_or(BankError::AccountNotFound(id))
    }

    pub fn update_name(&mut self, id: AccountId, name: impl Into<String>) -> BankResult<()> {
        let name = clean_name(name.into())?;
        let account = self.account_mut(id)?;

        account.name = name;
        account.record(Transaction::new(
            TransactionKind::ProfileUpdated,
            Money::ZERO,
            "Name updated",
        ));

        Ok(())
    }

    pub fn update_email(&mut self, id: AccountId, email: impl Into<String>) -> BankResult<()> {
        let email = clean_email(email.into())?;
        let account = self.account_mut(id)?;

        account.email = email;
        account.record(Transaction::new(
            TransactionKind::ProfileUpdated,
            Money::ZERO,
            "Email updated",
        ));

        Ok(())
    }

    pub fn activate_account(&mut self, id: AccountId) -> BankResult<()> {
        let account = self.account_mut(id)?;

        if account.is_closed() {
            return Err(BankError::AccountClosed(id));
        }

        account.status = AccountStatus::Active;
        account.record(Transaction::new(
            TransactionKind::Activated,
            Money::ZERO,
            "Account activated",
        ));

        Ok(())
    }

    pub fn deactivate_account(&mut self, id: AccountId) -> BankResult<()> {
        let account = self.account_mut(id)?;

        if account.is_closed() {
            return Err(BankError::AccountClosed(id));
        }

        account.status = AccountStatus::Inactive;
        account.record(Transaction::new(
            TransactionKind::Deactivated,
            Money::ZERO,
            "Account deactivated",
        ));

        Ok(())
    }

    pub fn close_account(&mut self, id: AccountId) -> BankResult<()> {
        let account = self.account_mut(id)?;

        if !account.balance.is_zero() {
            return Err(BankError::AccountHasBalance {
                account_id: id,
                balance: account.balance,
            });
        }

        if !account.loan_balance.is_zero() {
            return Err(BankError::AccountHasLoan {
                account_id: id,
                loan_balance: account.loan_balance,
            });
        }

        account.status = AccountStatus::Closed;
        account.record(Transaction::new(
            TransactionKind::Closed,
            Money::ZERO,
            "Account closed",
        ));

        Ok(())
    }

    pub fn delete_account(&mut self, id: AccountId) -> BankResult<Account> {
        let account = self.account(id)?;

        if !account.balance.is_zero() {
            return Err(BankError::AccountHasBalance {
                account_id: id,
                balance: account.balance,
            });
        }

        if !account.loan_balance.is_zero() {
            return Err(BankError::AccountHasLoan {
                account_id: id,
                loan_balance: account.loan_balance,
            });
        }

        self.accounts
            .remove(&id)
            .ok_or(BankError::AccountNotFound(id))
    }

    pub fn deposit(&mut self, id: AccountId, amount: Money) -> BankResult<()> {
        ensure_positive(amount)?;
        self.ensure_can_transact(id)?;

        let account = self.account_mut(id)?;
        account.balance = account
            .balance
            .checked_add(amount)
            .ok_or(BankError::ArithmeticOverflow)?;
        account.record(Transaction::new(
            TransactionKind::Deposit,
            amount,
            "Money deposited",
        ));

        Ok(())
    }

    pub fn withdraw(&mut self, id: AccountId, amount: Money) -> BankResult<()> {
        ensure_positive(amount)?;
        self.ensure_can_transact(id)?;
        self.ensure_sufficient_funds(id, amount)?;

        let account = self.account_mut(id)?;
        account.balance = account
            .balance
            .checked_sub(amount)
            .ok_or(BankError::ArithmeticOverflow)?;
        account.record(Transaction::new(
            TransactionKind::Withdrawal,
            amount,
            "Money withdrawn",
        ));

        Ok(())
    }

    pub fn transfer(
        &mut self,
        from_id: AccountId,
        to_id: AccountId,
        amount: Money,
    ) -> BankResult<()> {
        ensure_positive(amount)?;

        if from_id == to_id {
            return Err(BankError::SameAccountTransfer(from_id));
        }

        self.ensure_can_transact(from_id)?;
        self.ensure_can_transact(to_id)?;
        self.ensure_sufficient_funds(from_id, amount)?;

        let from_balance = self.account(from_id)?.balance;
        let to_balance = self.account(to_id)?.balance;
        let next_from_balance = from_balance
            .checked_sub(amount)
            .ok_or(BankError::ArithmeticOverflow)?;
        let next_to_balance = to_balance
            .checked_add(amount)
            .ok_or(BankError::ArithmeticOverflow)?;

        {
            let from_account = self.account_mut(from_id)?;
            from_account.balance = next_from_balance;
            from_account.record(Transaction::new(
                TransactionKind::TransferOut,
                amount,
                format!("Transferred to account {to_id}"),
            ));
        }

        {
            let to_account = self.account_mut(to_id)?;
            to_account.balance = next_to_balance;
            to_account.record(Transaction::new(
                TransactionKind::TransferIn,
                amount,
                format!("Received from account {from_id}"),
            ));
        }

        Ok(())
    }

    pub fn apply_fee(&mut self, id: AccountId, amount: Money) -> BankResult<()> {
        ensure_positive(amount)?;
        self.ensure_can_transact(id)?;
        self.ensure_sufficient_funds(id, amount)?;

        let account = self.account_mut(id)?;
        account.balance = account
            .balance
            .checked_sub(amount)
            .ok_or(BankError::ArithmeticOverflow)?;
        account.record(Transaction::new(
            TransactionKind::Fee,
            amount,
            "Bank fee applied",
        ));

        Ok(())
    }

    pub fn apply_monthly_fee(&mut self, amount: Money) -> Vec<(AccountId, BankResult<()>)> {
        let account_ids: Vec<AccountId> = self.accounts.keys().copied().collect();

        account_ids
            .into_iter()
            .map(|id| (id, self.apply_fee(id, amount)))
            .collect()
    }

    pub fn apply_interest(&mut self, id: AccountId, rate: InterestRate) -> BankResult<Money> {
        if rate.is_zero() {
            return Err(BankError::InvalidAmount);
        }

        self.ensure_can_transact(id)?;

        let interest = self
            .account(id)?
            .balance
            .checked_percentage(rate)
            .ok_or(BankError::ArithmeticOverflow)?;

        let account = self.account_mut(id)?;
        account.balance = account
            .balance
            .checked_add(interest)
            .ok_or(BankError::ArithmeticOverflow)?;
        account.record(Transaction::new(
            TransactionKind::Interest,
            interest,
            format!("Interest added at {rate}"),
        ));

        Ok(interest)
    }

    pub fn request_loan(&mut self, id: AccountId, amount: Money) -> BankResult<()> {
        ensure_positive(amount)?;
        self.ensure_can_transact(id)?;

        let account = self.account_mut(id)?;
        account.balance = account
            .balance
            .checked_add(amount)
            .ok_or(BankError::ArithmeticOverflow)?;
        account.loan_balance = account
            .loan_balance
            .checked_add(amount)
            .ok_or(BankError::ArithmeticOverflow)?;
        account.record(Transaction::new(
            TransactionKind::LoanRequested,
            amount,
            "Loan received",
        ));

        Ok(())
    }

    pub fn pay_loan(&mut self, id: AccountId, amount: Money) -> BankResult<Money> {
        ensure_positive(amount)?;
        self.ensure_can_transact(id)?;

        let account = self.account(id)?;

        if account.loan_balance.is_zero() {
            return Err(BankError::NoLoan(id));
        }

        let payment = amount.min(account.loan_balance);

        if account.balance < payment {
            return Err(BankError::InsufficientFunds {
                account_id: id,
                available: account.balance,
                required: payment,
            });
        }

        let account = self.account_mut(id)?;
        account.balance = account
            .balance
            .checked_sub(payment)
            .ok_or(BankError::ArithmeticOverflow)?;
        account.loan_balance = account
            .loan_balance
            .checked_sub(payment)
            .ok_or(BankError::ArithmeticOverflow)?;
        account.record(Transaction::new(
            TransactionKind::LoanPayment,
            payment,
            "Loan payment made",
        ));

        Ok(payment)
    }

    pub fn total_balance(&self) -> BankResult<Money> {
        self.accounts().try_fold(Money::ZERO, |total, account| {
            total
                .checked_add(account.balance)
                .ok_or(BankError::ArithmeticOverflow)
        })
    }

    pub fn richest_account(&self) -> Option<&Account> {
        self.accounts().max_by_key(|account| account.balance)
    }

    pub fn empty_accounts(&self) -> Vec<&Account> {
        self.accounts()
            .filter(|account| account.balance.is_zero())
            .collect()
    }

    fn account_mut(&mut self, id: AccountId) -> BankResult<&mut Account> {
        self.accounts
            .get_mut(&id)
            .ok_or(BankError::AccountNotFound(id))
    }

    fn ensure_can_transact(&self, id: AccountId) -> BankResult<()> {
        let account = self.account(id)?;

        match account.status {
            AccountStatus::Active => Ok(()),
            AccountStatus::Inactive => Err(BankError::AccountInactive(id)),
            AccountStatus::Closed => Err(BankError::AccountClosed(id)),
        }
    }

    fn ensure_sufficient_funds(&self, id: AccountId, required: Money) -> BankResult<()> {
        let account = self.account(id)?;

        if account.balance < required {
            return Err(BankError::InsufficientFunds {
                account_id: id,
                available: account.balance,
                required,
            });
        }

        Ok(())
    }
}

fn clean_name(name: String) -> BankResult<String> {
    let name = name.trim().to_string();

    if name.is_empty() {
        return Err(BankError::InvalidProfileField("name"));
    }

    Ok(name)
}

fn clean_email(email: String) -> BankResult<String> {
    let email = email.trim().to_lowercase();

    if email.is_empty() || !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(BankError::InvalidProfileField("email"));
    }

    Ok(email)
}

fn ensure_positive(amount: Money) -> BankResult<()> {
    if amount <= Money::ZERO {
        return Err(BankError::InvalidAmount);
    }

    Ok(())
}

fn ensure_non_negative(amount: Money) -> BankResult<()> {
    if amount.is_negative() {
        return Err(BankError::InvalidAmount);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Bank, BankError};
    use crate::domain::{AccountStatus, Money, TransactionKind};

    fn money(input: &str) -> Money {
        input.parse().unwrap()
    }

    fn bank_with_two_accounts() -> Bank {
        let mut bank = Bank::new();
        bank.create_account(1, "Alice", "alice@example.com", money("100.00"))
            .unwrap();
        bank.create_account(2, "Bob", "bob@example.com", money("25.50"))
            .unwrap();
        bank
    }

    #[test]
    fn rejects_duplicate_accounts() {
        let mut bank = Bank::new();
        bank.create_account(1, "Alice", "alice@example.com", Money::ZERO)
            .unwrap();

        assert_eq!(
            bank.create_account(1, "Alice", "alice@example.com", Money::ZERO),
            Err(BankError::AccountAlreadyExists(1))
        );
    }

    #[test]
    fn deposit_and_withdraw_update_exact_cent_balances() {
        let mut bank = bank_with_two_accounts();

        bank.deposit(1, money("10.25")).unwrap();
        bank.withdraw(1, money("0.30")).unwrap();

        assert_eq!(bank.account(1).unwrap().balance(), money("109.95"));
    }

    #[test]
    fn transfer_is_recorded_on_both_accounts() {
        let mut bank = bank_with_two_accounts();

        bank.transfer(1, 2, money("20.00")).unwrap();

        let from = bank.account(1).unwrap();
        let to = bank.account(2).unwrap();

        assert_eq!(from.balance(), money("80.00"));
        assert_eq!(to.balance(), money("45.50"));
        assert_eq!(
            from.transactions().last().unwrap().kind(),
            TransactionKind::TransferOut
        );
        assert_eq!(
            to.transactions().last().unwrap().kind(),
            TransactionKind::TransferIn
        );
    }

    #[test]
    fn transfer_to_same_account_is_rejected() {
        let mut bank = bank_with_two_accounts();

        assert_eq!(
            bank.transfer(1, 1, money("10.00")),
            Err(BankError::SameAccountTransfer(1))
        );
    }

    #[test]
    fn inactive_accounts_cannot_move_money() {
        let mut bank = bank_with_two_accounts();
        bank.deactivate_account(1).unwrap();

        assert_eq!(
            bank.withdraw(1, money("1.00")),
            Err(BankError::AccountInactive(1))
        );
    }

    #[test]
    fn closing_requires_zero_balance_and_no_loan() {
        let mut bank = bank_with_two_accounts();

        assert!(matches!(
            bank.close_account(1),
            Err(BankError::AccountHasBalance { .. })
        ));

        bank.withdraw(1, money("100.00")).unwrap();
        bank.close_account(1).unwrap();

        assert_eq!(bank.account(1).unwrap().status(), AccountStatus::Closed);
    }

    #[test]
    fn loan_payment_is_capped_to_outstanding_balance() {
        let mut bank = Bank::new();
        bank.create_account(1, "Alice", "alice@example.com", money("10.00"))
            .unwrap();
        bank.request_loan(1, money("50.00")).unwrap();

        let paid = bank.pay_loan(1, money("100.00")).unwrap();

        assert_eq!(paid, money("50.00"));
        assert_eq!(bank.account(1).unwrap().loan_balance(), Money::ZERO);
        assert_eq!(bank.account(1).unwrap().balance(), money("10.00"));
    }

    #[test]
    fn total_balance_sums_all_accounts() {
        let bank = bank_with_two_accounts();

        assert_eq!(bank.total_balance().unwrap(), money("125.50"));
    }
}
