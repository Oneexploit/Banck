use std::{collections::BTreeMap, error::Error, fmt};

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};

pub type CustomerId = u32;
pub type UserId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Customer,
    Teller,
    Admin,
}

impl Role {
    pub const fn can(self, permission: Permission) -> bool {
        match self {
            Self::Admin => true,
            Self::Teller => matches!(
                permission,
                Permission::ManageAccounts
                    | Permission::MoveMoney
                    | Permission::ViewAnyAccount
                    | Permission::ExportStatements
                    | Permission::PersistState
            ),
            Self::Customer => matches!(
                permission,
                Permission::MoveOwnMoney | Permission::ViewOwnAccount
            ),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Customer => write!(formatter, "customer"),
            Self::Teller => write!(formatter, "teller"),
            Self::Admin => write!(formatter, "admin"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    ManageIdentity,
    ManageAccounts,
    MoveMoney,
    MoveOwnMoney,
    ViewAnyAccount,
    ViewOwnAccount,
    ExportStatements,
    PersistState,
}

impl fmt::Display for Permission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManageIdentity => write!(formatter, "manage_identity"),
            Self::ManageAccounts => write!(formatter, "manage_accounts"),
            Self::MoveMoney => write!(formatter, "move_money"),
            Self::MoveOwnMoney => write!(formatter, "move_own_money"),
            Self::ViewAnyAccount => write!(formatter, "view_any_account"),
            Self::ViewOwnAccount => write!(formatter, "view_own_account"),
            Self::ExportStatements => write!(formatter, "export_statements"),
            Self::PersistState => write!(formatter, "persist_state"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Customer {
    id: CustomerId,
    full_name: String,
    email: String,
}

impl Customer {
    fn new(id: CustomerId, full_name: String, email: String) -> Self {
        Self {
            id,
            full_name,
            email,
        }
    }

    pub const fn id(&self) -> CustomerId {
        self.id
    }

    pub fn full_name(&self) -> &str {
        &self.full_name
    }

    pub fn email(&self) -> &str {
        &self.email
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    id: UserId,
    username: String,
    role: Role,
    customer_id: Option<CustomerId>,
    password_hash: String,
}

impl User {
    fn new(
        id: UserId,
        username: String,
        role: Role,
        customer_id: Option<CustomerId>,
        password_hash: String,
    ) -> Self {
        Self {
            id,
            username,
            role,
            customer_id,
            password_hash,
        }
    }

    pub const fn id(&self) -> UserId {
        self.id
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub const fn role(&self) -> Role {
        self.role
    }

    pub const fn customer_id(&self) -> Option<CustomerId> {
        self.customer_id
    }

    pub fn password_hash(&self) -> &str {
        &self.password_hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    user_id: UserId,
    username: String,
    role: Role,
    customer_id: Option<CustomerId>,
}

impl Session {
    fn new(user: &User) -> Self {
        Self {
            user_id: user.id,
            username: user.username.clone(),
            role: user.role,
            customer_id: user.customer_id,
        }
    }

    pub const fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub const fn role(&self) -> Role {
        self.role
    }

    pub const fn customer_id(&self) -> Option<CustomerId> {
        self.customer_id
    }

    pub const fn can(&self, permission: Permission) -> bool {
        self.role.can(permission)
    }
}

#[derive(Debug)]
pub enum IdentityError {
    CustomerAlreadyExists(CustomerId),
    CustomerNotFound(CustomerId),
    UserAlreadyExists(UserId),
    UsernameAlreadyExists(String),
    UserNotFound(UserId),
    InvalidCredentials,
    InvalidProfileField(&'static str),
    PasswordTooWeak,
    RoleRequiresCustomer,
    RoleCannotHaveCustomer,
    PermissionDenied { role: Role, permission: Permission },
    PasswordHash(argon2::password_hash::Error),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CustomerAlreadyExists(id) => write!(formatter, "customer {id} already exists"),
            Self::CustomerNotFound(id) => write!(formatter, "customer {id} was not found"),
            Self::UserAlreadyExists(id) => write!(formatter, "user {id} already exists"),
            Self::UsernameAlreadyExists(username) => {
                write!(formatter, "username {username} already exists")
            }
            Self::UserNotFound(id) => write!(formatter, "user {id} was not found"),
            Self::InvalidCredentials => write!(formatter, "invalid username or password"),
            Self::InvalidProfileField(field) => write!(formatter, "{field} is invalid"),
            Self::PasswordTooWeak => {
                write!(formatter, "password must be at least 8 characters long")
            }
            Self::RoleRequiresCustomer => write!(formatter, "customer users require a customer id"),
            Self::RoleCannotHaveCustomer => {
                write!(formatter, "staff users cannot be linked to a customer id")
            }
            Self::PermissionDenied { role, permission } => {
                write!(formatter, "{role} cannot perform {permission}")
            }
            Self::PasswordHash(source) => write!(formatter, "password hashing failed: {source}"),
        }
    }
}

impl Error for IdentityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

pub type IdentityResult<T> = Result<T, IdentityError>;

#[derive(Debug, Default)]
pub struct IdentityStore {
    customers: BTreeMap<CustomerId, Customer>,
    users: BTreeMap<UserId, User>,
    username_index: BTreeMap<String, UserId>,
}

impl IdentityStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_records(customers: Vec<Customer>, users: Vec<User>) -> IdentityResult<Self> {
        let mut store = Self::new();

        for customer in customers {
            validate_customer(&customer)?;
            let customer_id = customer.id;

            if store.customers.insert(customer_id, customer).is_some() {
                return Err(IdentityError::CustomerAlreadyExists(customer_id));
            }
        }

        for user in users {
            validate_imported_user(&store, &user)?;
            let user_id = user.id;
            let username = normalize_username(&user.username)?;

            if store.users.contains_key(&user_id) {
                return Err(IdentityError::UserAlreadyExists(user_id));
            }

            if store
                .username_index
                .insert(username.clone(), user_id)
                .is_some()
            {
                return Err(IdentityError::UsernameAlreadyExists(username));
            }

            store.users.insert(user_id, user);
        }

        Ok(store)
    }

    pub fn has_users(&self) -> bool {
        !self.users.is_empty()
    }

    pub fn customers(&self) -> impl Iterator<Item = &Customer> {
        self.customers.values()
    }

    pub fn users(&self) -> impl Iterator<Item = &User> {
        self.users.values()
    }

    pub fn customer(&self, id: CustomerId) -> IdentityResult<&Customer> {
        self.customers
            .get(&id)
            .ok_or(IdentityError::CustomerNotFound(id))
    }

    pub fn user(&self, id: UserId) -> IdentityResult<&User> {
        self.users.get(&id).ok_or(IdentityError::UserNotFound(id))
    }

    pub fn create_customer(
        &mut self,
        id: CustomerId,
        full_name: impl Into<String>,
        email: impl Into<String>,
    ) -> IdentityResult<&Customer> {
        if self.customers.contains_key(&id) {
            return Err(IdentityError::CustomerAlreadyExists(id));
        }

        let customer = Customer::new(
            id,
            clean_name(full_name.into())?,
            clean_email(email.into())?,
        );

        self.customers.insert(id, customer);
        Ok(self
            .customers
            .get(&id)
            .expect("inserted customer must exist"))
    }

    pub fn create_user(
        &mut self,
        id: UserId,
        username: impl Into<String>,
        role: Role,
        customer_id: Option<CustomerId>,
        password: &str,
    ) -> IdentityResult<&User> {
        if self.users.contains_key(&id) {
            return Err(IdentityError::UserAlreadyExists(id));
        }

        validate_password(password)?;
        self.validate_role_customer_link(role, customer_id)?;

        let username = normalize_username(&username.into())?;

        if self.username_index.contains_key(&username) {
            return Err(IdentityError::UsernameAlreadyExists(username));
        }

        let password_hash = hash_password(password)?;
        let user = User::new(id, username.clone(), role, customer_id, password_hash);

        self.users.insert(id, user);
        self.username_index.insert(username, id);

        Ok(self.users.get(&id).expect("inserted user must exist"))
    }

    pub fn authenticate(
        &self,
        username: impl AsRef<str>,
        password: &str,
    ) -> IdentityResult<Session> {
        let username = normalize_username(username.as_ref())?;
        let user_id = self
            .username_index
            .get(&username)
            .ok_or(IdentityError::InvalidCredentials)?;
        let user = self.user(*user_id)?;
        let parsed_hash =
            PasswordHash::new(user.password_hash()).map_err(IdentityError::PasswordHash)?;

        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| IdentityError::InvalidCredentials)?;

        Ok(Session::new(user))
    }

    pub fn authorize(session: &Session, permission: Permission) -> IdentityResult<()> {
        if session.can(permission) {
            return Ok(());
        }

        Err(IdentityError::PermissionDenied {
            role: session.role(),
            permission,
        })
    }

    fn validate_role_customer_link(
        &self,
        role: Role,
        customer_id: Option<CustomerId>,
    ) -> IdentityResult<()> {
        match (role, customer_id) {
            (Role::Customer, Some(customer_id)) => {
                self.customer(customer_id)?;
                Ok(())
            }
            (Role::Customer, None) => Err(IdentityError::RoleRequiresCustomer),
            (Role::Teller | Role::Admin, Some(_)) => Err(IdentityError::RoleCannotHaveCustomer),
            (Role::Teller | Role::Admin, None) => Ok(()),
        }
    }
}

fn validate_customer(customer: &Customer) -> IdentityResult<()> {
    clean_name(customer.full_name.clone())?;
    clean_email(customer.email.clone())?;

    Ok(())
}

fn validate_imported_user(store: &IdentityStore, user: &User) -> IdentityResult<()> {
    normalize_username(&user.username)?;
    store.validate_role_customer_link(user.role, user.customer_id)?;
    PasswordHash::new(user.password_hash()).map_err(IdentityError::PasswordHash)?;

    Ok(())
}

fn clean_name(name: String) -> IdentityResult<String> {
    let name = name.trim().to_string();

    if name.is_empty() {
        return Err(IdentityError::InvalidProfileField("name"));
    }

    Ok(name)
}

fn clean_email(email: String) -> IdentityResult<String> {
    let email = email.trim().to_lowercase();

    if email.is_empty() || !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(IdentityError::InvalidProfileField("email"));
    }

    Ok(email)
}

fn normalize_username(username: &str) -> IdentityResult<String> {
    let username = username.trim().to_lowercase();

    if username.len() < 3
        || !username.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        return Err(IdentityError::InvalidProfileField("username"));
    }

    Ok(username)
}

fn validate_password(password: &str) -> IdentityResult<()> {
    if password.len() < 8 {
        return Err(IdentityError::PasswordTooWeak);
    }

    Ok(())
}

fn hash_password(password: &str) -> IdentityResult<String> {
    let salt = SaltString::generate(&mut OsRng);

    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(IdentityError::PasswordHash)
}

#[cfg(test)]
mod tests {
    use super::{IdentityError, IdentityStore, Permission, Role};

    #[test]
    fn creates_users_with_hashed_passwords() {
        let mut identities = IdentityStore::new();
        let user = identities
            .create_user(1, "admin", Role::Admin, None, "correct-password")
            .unwrap();

        assert_ne!(user.password_hash(), "correct-password");
        assert!(user.password_hash().starts_with("$argon2"));
    }

    #[test]
    fn authenticates_valid_credentials() {
        let mut identities = IdentityStore::new();
        identities
            .create_user(1, "admin", Role::Admin, None, "correct-password")
            .unwrap();

        let session = identities
            .authenticate("admin", "correct-password")
            .unwrap();

        assert_eq!(session.role(), Role::Admin);
        assert!(session.can(Permission::ManageIdentity));
    }

    #[test]
    fn rejects_invalid_credentials() {
        let mut identities = IdentityStore::new();
        identities
            .create_user(1, "admin", Role::Admin, None, "correct-password")
            .unwrap();

        assert!(matches!(
            identities.authenticate("admin", "wrong-password"),
            Err(IdentityError::InvalidCredentials)
        ));
    }

    #[test]
    fn customer_users_require_existing_customer() {
        let mut identities = IdentityStore::new();

        assert!(matches!(
            identities.create_user(1, "customer", Role::Customer, Some(10), "correct-password"),
            Err(IdentityError::CustomerNotFound(10))
        ));
    }

    #[test]
    fn customer_sessions_keep_customer_identity() {
        let mut identities = IdentityStore::new();
        identities
            .create_customer(10, "Alice", "alice@example.com")
            .unwrap();
        identities
            .create_user(1, "alice", Role::Customer, Some(10), "correct-password")
            .unwrap();

        let session = identities
            .authenticate("alice", "correct-password")
            .unwrap();

        assert_eq!(session.customer_id(), Some(10));
        assert!(session.can(Permission::ViewOwnAccount));
        assert!(!session.can(Permission::ViewAnyAccount));
    }

    #[test]
    fn permissions_are_role_based() {
        assert!(Role::Admin.can(Permission::ManageIdentity));
        assert!(Role::Teller.can(Permission::ViewAnyAccount));
        assert!(!Role::Teller.can(Permission::ManageIdentity));
        assert!(Role::Customer.can(Permission::ViewOwnAccount));
        assert!(!Role::Customer.can(Permission::ViewAnyAccount));
    }
}
