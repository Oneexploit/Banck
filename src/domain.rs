use std::{
    error::Error,
    fmt,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

pub use crate::identity::CustomerId;

pub type AccountId = u32;
pub type TransactionId = u64;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Money {
    cents: i64,
}

impl Money {
    pub const ZERO: Self = Self { cents: 0 };

    pub const fn from_cents(cents: i64) -> Self {
        Self { cents }
    }

    pub const fn cents(self) -> i64 {
        self.cents
    }

    pub const fn is_zero(self) -> bool {
        self.cents == 0
    }

    pub const fn is_negative(self) -> bool {
        self.cents < 0
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.cents.checked_add(other.cents).map(Self::from_cents)
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.cents.checked_sub(other.cents).map(Self::from_cents)
    }

    pub fn checked_percentage(self, rate: InterestRate) -> Option<Self> {
        self.cents
            .checked_mul(i64::from(rate.basis_points()))?
            .checked_div(10_000)
            .map(Self::from_cents)
    }
}

impl fmt::Display for Money {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cents = i128::from(self.cents);
        let sign = if cents < 0 { "-" } else { "" };
        let absolute = if cents < 0 { -cents } else { cents };

        write!(
            formatter,
            "{}{}.{:02}",
            sign,
            absolute / 100,
            absolute % 100
        )
    }
}

impl FromStr for Money {
    type Err = MoneyParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();

        if input.is_empty() {
            return Err(MoneyParseError::Empty);
        }

        let (negative, unsigned) = match input.as_bytes()[0] {
            b'-' => (true, &input[1..]),
            b'+' => (false, &input[1..]),
            _ => (false, input),
        };

        if unsigned.is_empty() {
            return Err(MoneyParseError::InvalidFormat);
        }

        let parts: Vec<&str> = unsigned.split('.').collect();

        if parts.len() > 2 {
            return Err(MoneyParseError::InvalidFormat);
        }

        let whole_part = parts[0];
        let fractional_part = parts.get(1).copied().unwrap_or("");

        if whole_part.is_empty() && fractional_part.is_empty() {
            return Err(MoneyParseError::InvalidFormat);
        }

        if !whole_part
            .chars()
            .all(|character| character.is_ascii_digit())
            || !fractional_part
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            return Err(MoneyParseError::InvalidFormat);
        }

        if fractional_part.len() > 2 {
            return Err(MoneyParseError::TooManyDecimalPlaces);
        }

        let whole = if whole_part.is_empty() {
            0
        } else {
            whole_part
                .parse::<i64>()
                .map_err(|_| MoneyParseError::Overflow)?
        };

        let fractional = match fractional_part.len() {
            0 => 0,
            1 => {
                fractional_part
                    .parse::<i64>()
                    .map_err(|_| MoneyParseError::Overflow)?
                    * 10
            }
            2 => fractional_part
                .parse::<i64>()
                .map_err(|_| MoneyParseError::Overflow)?,
            _ => unreachable!("fractional part length is checked above"),
        };

        let cents = whole
            .checked_mul(100)
            .and_then(|value| value.checked_add(fractional))
            .ok_or(MoneyParseError::Overflow)?;

        let cents = if negative {
            cents.checked_neg().ok_or(MoneyParseError::Overflow)?
        } else {
            cents
        };

        Ok(Self::from_cents(cents))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoneyParseError {
    Empty,
    InvalidFormat,
    TooManyDecimalPlaces,
    Overflow,
}

impl fmt::Display for MoneyParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "money value cannot be empty"),
            Self::InvalidFormat => write!(formatter, "money value must look like 100 or 100.50"),
            Self::TooManyDecimalPlaces => {
                write!(formatter, "money value cannot have more than two decimals")
            }
            Self::Overflow => write!(formatter, "money value is too large"),
        }
    }
}

impl Error for MoneyParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InterestRate {
    basis_points: u32,
}

impl InterestRate {
    pub const fn from_basis_points(basis_points: u32) -> Self {
        Self { basis_points }
    }

    pub const fn basis_points(self) -> u32 {
        self.basis_points
    }

    pub const fn is_zero(self) -> bool {
        self.basis_points == 0
    }
}

impl fmt::Display for InterestRate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}.{:02}%",
            self.basis_points / 100,
            self.basis_points % 100
        )
    }
}

impl FromStr for InterestRate {
    type Err = InterestRateParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();

        if input.is_empty() {
            return Err(InterestRateParseError::Empty);
        }

        if input.starts_with('-') {
            return Err(InterestRateParseError::Negative);
        }

        let input = input.strip_prefix('+').unwrap_or(input);
        let parts: Vec<&str> = input.split('.').collect();

        if parts.len() > 2 || parts[0].is_empty() {
            return Err(InterestRateParseError::InvalidFormat);
        }

        let whole_part = parts[0];
        let fractional_part = parts.get(1).copied().unwrap_or("");

        if !whole_part
            .chars()
            .all(|character| character.is_ascii_digit())
            || !fractional_part
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            return Err(InterestRateParseError::InvalidFormat);
        }

        if fractional_part.len() > 2 {
            return Err(InterestRateParseError::TooManyDecimalPlaces);
        }

        let whole = whole_part
            .parse::<u32>()
            .map_err(|_| InterestRateParseError::Overflow)?;

        let fractional = match fractional_part.len() {
            0 => 0,
            1 => {
                fractional_part
                    .parse::<u32>()
                    .map_err(|_| InterestRateParseError::Overflow)?
                    * 10
            }
            2 => fractional_part
                .parse::<u32>()
                .map_err(|_| InterestRateParseError::Overflow)?,
            _ => unreachable!("fractional part length is checked above"),
        };

        let basis_points = whole
            .checked_mul(100)
            .and_then(|value| value.checked_add(fractional))
            .ok_or(InterestRateParseError::Overflow)?;

        Ok(Self::from_basis_points(basis_points))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterestRateParseError {
    Empty,
    Negative,
    InvalidFormat,
    TooManyDecimalPlaces,
    Overflow,
}

impl fmt::Display for InterestRateParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "interest rate cannot be empty"),
            Self::Negative => write!(formatter, "interest rate cannot be negative"),
            Self::InvalidFormat => write!(formatter, "interest rate must look like 3 or 3.25"),
            Self::TooManyDecimalPlaces => {
                write!(
                    formatter,
                    "interest rate cannot have more than two decimal places"
                )
            }
            Self::Overflow => write!(formatter, "interest rate is too large"),
        }
    }
}

impl Error for InterestRateParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Active,
    Inactive,
    Closed,
}

impl fmt::Display for AccountStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(formatter, "active"),
            Self::Inactive => write!(formatter, "inactive"),
            Self::Closed => write!(formatter, "closed"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    AccountCreated,
    Deposit,
    Withdrawal,
    TransferIn,
    TransferOut,
    Fee,
    Interest,
    LoanRequested,
    LoanPayment,
    ProfileUpdated,
    Activated,
    Deactivated,
    Closed,
}

impl fmt::Display for TransactionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccountCreated => write!(formatter, "account_created"),
            Self::Deposit => write!(formatter, "deposit"),
            Self::Withdrawal => write!(formatter, "withdrawal"),
            Self::TransferIn => write!(formatter, "transfer_in"),
            Self::TransferOut => write!(formatter, "transfer_out"),
            Self::Fee => write!(formatter, "fee"),
            Self::Interest => write!(formatter, "interest"),
            Self::LoanRequested => write!(formatter, "loan_requested"),
            Self::LoanPayment => write!(formatter, "loan_payment"),
            Self::ProfileUpdated => write!(formatter, "profile_updated"),
            Self::Activated => write!(formatter, "activated"),
            Self::Deactivated => write!(formatter, "deactivated"),
            Self::Closed => write!(formatter, "closed"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    id: TransactionId,
    occurred_at_epoch_seconds: u64,
    kind: TransactionKind,
    amount: Money,
    description: String,
}

impl Transaction {
    pub fn new(
        id: TransactionId,
        occurred_at_epoch_seconds: u64,
        kind: TransactionKind,
        amount: Money,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id,
            occurred_at_epoch_seconds,
            kind,
            amount,
            description: description.into(),
        }
    }

    pub const fn id(&self) -> TransactionId {
        self.id
    }

    pub const fn occurred_at_epoch_seconds(&self) -> u64 {
        self.occurred_at_epoch_seconds
    }

    pub const fn kind(&self) -> TransactionKind {
        self.kind
    }

    pub const fn amount(&self) -> Money {
        self.amount
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub(crate) id: AccountId,
    pub(crate) owner_id: Option<CustomerId>,
    pub(crate) name: String,
    pub(crate) email: String,
    pub(crate) balance: Money,
    pub(crate) loan_balance: Money,
    pub(crate) status: AccountStatus,
    pub(crate) transactions: Vec<Transaction>,
}

impl Account {
    pub(crate) fn new(
        id: AccountId,
        owner_id: Option<CustomerId>,
        name: impl Into<String>,
        email: impl Into<String>,
        opening_balance: Money,
    ) -> Self {
        Self {
            id,
            owner_id,
            name: name.into(),
            email: email.into(),
            balance: opening_balance,
            loan_balance: Money::ZERO,
            status: AccountStatus::Active,
            transactions: Vec::new(),
        }
    }

    pub const fn id(&self) -> AccountId {
        self.id
    }

    pub const fn owner_id(&self) -> Option<CustomerId> {
        self.owner_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub const fn balance(&self) -> Money {
        self.balance
    }

    pub const fn loan_balance(&self) -> Money {
        self.loan_balance
    }

    pub const fn status(&self) -> AccountStatus {
        self.status
    }

    pub const fn is_active(&self) -> bool {
        matches!(self.status, AccountStatus::Active)
    }

    pub const fn is_closed(&self) -> bool {
        matches!(self.status, AccountStatus::Closed)
    }

    pub fn transactions(&self) -> &[Transaction] {
        &self.transactions
    }

    pub(crate) fn record(&mut self, transaction: Transaction) {
        self.transactions.push(transaction);
    }
}

pub(crate) fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{InterestRate, Money};

    #[test]
    fn parses_money_without_using_float_precision() {
        assert_eq!("10.05".parse::<Money>().unwrap(), Money::from_cents(1005));
        assert_eq!("0.50".parse::<Money>().unwrap(), Money::from_cents(50));
        assert_eq!(".75".parse::<Money>().unwrap(), Money::from_cents(75));
    }

    #[test]
    fn rejects_money_with_more_than_two_decimals() {
        assert!("10.005".parse::<Money>().is_err());
    }

    #[test]
    fn parses_interest_rate_as_basis_points() {
        assert_eq!(
            "3.25".parse::<InterestRate>().unwrap(),
            InterestRate::from_basis_points(325)
        );
    }
}
