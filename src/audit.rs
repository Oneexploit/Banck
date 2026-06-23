use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    domain::current_epoch_seconds,
    identity::{Role, Session, UserId},
};

pub type AuditId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditActor {
    System,
    User {
        user_id: UserId,
        username: String,
        role: Role,
    },
}

impl AuditActor {
    pub fn from_session(session: &Session) -> Self {
        Self::User {
            user_id: session.user_id(),
            username: session.username().to_string(),
            role: session.role(),
        }
    }
}

impl fmt::Display for AuditActor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System => write!(formatter, "system"),
            Self::User {
                user_id,
                username,
                role,
            } => write!(formatter, "{username}#{user_id} ({role})"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    BootstrapAdmin,
    Login,
    Logout,
    CreateCustomer,
    CreateUser,
    CreateAccount,
    UpdateAccountProfile,
    ActivateAccount,
    DeactivateAccount,
    CloseAccount,
    DeleteAccount,
    Deposit,
    Withdraw,
    Transfer,
    Fee,
    Interest,
    LoanRequest,
    LoanPayment,
    SaveState,
    LoadState,
    ExportStatement,
}

impl fmt::Display for AuditAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BootstrapAdmin => write!(formatter, "bootstrap_admin"),
            Self::Login => write!(formatter, "login"),
            Self::Logout => write!(formatter, "logout"),
            Self::CreateCustomer => write!(formatter, "create_customer"),
            Self::CreateUser => write!(formatter, "create_user"),
            Self::CreateAccount => write!(formatter, "create_account"),
            Self::UpdateAccountProfile => write!(formatter, "update_account_profile"),
            Self::ActivateAccount => write!(formatter, "activate_account"),
            Self::DeactivateAccount => write!(formatter, "deactivate_account"),
            Self::CloseAccount => write!(formatter, "close_account"),
            Self::DeleteAccount => write!(formatter, "delete_account"),
            Self::Deposit => write!(formatter, "deposit"),
            Self::Withdraw => write!(formatter, "withdraw"),
            Self::Transfer => write!(formatter, "transfer"),
            Self::Fee => write!(formatter, "fee"),
            Self::Interest => write!(formatter, "interest"),
            Self::LoanRequest => write!(formatter, "loan_request"),
            Self::LoanPayment => write!(formatter, "loan_payment"),
            Self::SaveState => write!(formatter, "save_state"),
            Self::LoadState => write!(formatter, "load_state"),
            Self::ExportStatement => write!(formatter, "export_statement"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Failure,
}

impl fmt::Display for AuditOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(formatter, "success"),
            Self::Failure => write!(formatter, "failure"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    id: AuditId,
    occurred_at_epoch_seconds: u64,
    actor: AuditActor,
    action: AuditAction,
    outcome: AuditOutcome,
    target: Option<String>,
    message: String,
}

impl AuditEntry {
    pub fn new(
        id: AuditId,
        occurred_at_epoch_seconds: u64,
        actor: AuditActor,
        action: AuditAction,
        outcome: AuditOutcome,
        target: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id,
            occurred_at_epoch_seconds,
            actor,
            action,
            outcome,
            target,
            message: message.into(),
        }
    }

    pub const fn id(&self) -> AuditId {
        self.id
    }

    pub const fn occurred_at_epoch_seconds(&self) -> u64 {
        self.occurred_at_epoch_seconds
    }

    pub const fn action(&self) -> AuditAction {
        self.action
    }

    pub const fn outcome(&self) -> AuditOutcome {
        self.outcome
    }

    pub fn actor(&self) -> &AuditActor {
        &self.actor
    }

    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug)]
pub enum AuditError {
    InvalidAuditId(AuditId),
    DuplicateAuditId(AuditId),
    ArithmeticOverflow,
}

impl fmt::Display for AuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAuditId(id) => write!(formatter, "audit id {id} is invalid"),
            Self::DuplicateAuditId(id) => write!(formatter, "audit id {id} is duplicated"),
            Self::ArithmeticOverflow => write!(formatter, "audit id overflowed"),
        }
    }
}

impl Error for AuditError {}

pub type AuditResult<T> = Result<T, AuditError>;

#[derive(Debug)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
    next_id: AuditId,
}

impl Default for AuditLog {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
        }
    }
}

impl AuditLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_entries(entries: Vec<AuditEntry>) -> AuditResult<Self> {
        let mut seen = BTreeSet::new();
        let mut max_id = 0;

        for entry in &entries {
            if entry.id == 0 {
                return Err(AuditError::InvalidAuditId(entry.id));
            }

            if !seen.insert(entry.id) {
                return Err(AuditError::DuplicateAuditId(entry.id));
            }

            max_id = max_id.max(entry.id);
        }

        Ok(Self {
            entries,
            next_id: max_id
                .checked_add(1)
                .ok_or(AuditError::ArithmeticOverflow)?,
        })
    }

    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    pub fn record(
        &mut self,
        actor: AuditActor,
        action: AuditAction,
        outcome: AuditOutcome,
        target: Option<String>,
        message: impl Into<String>,
    ) -> AuditResult<&AuditEntry> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(AuditError::ArithmeticOverflow)?;
        let entry = AuditEntry::new(
            id,
            current_epoch_seconds(),
            actor,
            action,
            outcome,
            target,
            message,
        );

        self.entries.push(entry);
        Ok(self
            .entries
            .last()
            .expect("recorded audit entry must exist"))
    }
}

#[cfg(test)]
mod tests {
    use super::{AuditAction, AuditActor, AuditError, AuditLog, AuditOutcome};

    #[test]
    fn assigns_incrementing_audit_ids() {
        let mut log = AuditLog::new();

        log.record(
            AuditActor::System,
            AuditAction::Login,
            AuditOutcome::Success,
            None,
            "ok",
        )
        .unwrap();
        log.record(
            AuditActor::System,
            AuditAction::Transfer,
            AuditOutcome::Failure,
            Some("account:1".to_string()),
            "denied",
        )
        .unwrap();

        assert_eq!(log.entries()[0].id(), 1);
        assert_eq!(log.entries()[1].id(), 2);
    }

    #[test]
    fn rejects_duplicate_imported_audit_ids() {
        let first = super::AuditEntry::new(
            1,
            1,
            AuditActor::System,
            AuditAction::Login,
            AuditOutcome::Success,
            None,
            "ok",
        );
        let second = super::AuditEntry::new(
            1,
            2,
            AuditActor::System,
            AuditAction::Login,
            AuditOutcome::Failure,
            None,
            "fail",
        );

        assert!(matches!(
            AuditLog::from_entries(vec![first, second]),
            Err(AuditError::DuplicateAuditId(1))
        ));
    }
}
