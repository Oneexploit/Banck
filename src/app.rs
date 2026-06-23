use crate::{audit::AuditLog, bank::Bank, identity::IdentityStore};

#[derive(Debug)]
pub struct AppState {
    pub bank: Bank,
    pub identities: IdentityStore,
    pub audit_log: AuditLog,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            bank: Bank::new(),
            identities: IdentityStore::new(),
            audit_log: AuditLog::new(),
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_parts(bank: Bank, identities: IdentityStore) -> Self {
        Self {
            bank,
            identities,
            audit_log: AuditLog::new(),
        }
    }

    pub fn from_parts_with_audit(
        bank: Bank,
        identities: IdentityStore,
        audit_log: AuditLog,
    ) -> Self {
        Self {
            bank,
            identities,
            audit_log,
        }
    }
}
