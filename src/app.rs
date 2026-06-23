use crate::{bank::Bank, identity::IdentityStore};

#[derive(Debug, Default)]
pub struct AppState {
    pub bank: Bank,
    pub identities: IdentityStore,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_parts(bank: Bank, identities: IdentityStore) -> Self {
        Self { bank, identities }
    }
}
