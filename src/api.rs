use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, sync::RwLock};

use crate::{
    app::AppState,
    audit::{AuditAction, AuditActor, AuditEntry, AuditOutcome},
    bank::{BankError, BankResult},
    domain::{Account, AccountId, CustomerId, Money, Transaction},
    identity::{Customer, IdentityError, IdentityStore, Permission, Role, Session, User, UserId},
};

#[derive(Clone)]
pub struct ApiState {
    app: Arc<RwLock<AppState>>,
}

impl ApiState {
    pub fn new(app: AppState) -> Self {
        Self {
            app: Arc::new(RwLock::new(app)),
        }
    }
}

pub fn router(app: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/auth/bootstrap-admin", post(bootstrap_admin))
        .route("/auth/login", post(login))
        .route("/customers", get(list_customers).post(create_customer))
        .route("/users", get(list_users).post(create_user))
        .route("/accounts", get(list_accounts).post(create_account))
        .route("/accounts/{id}", get(get_account))
        .route("/accounts/{id}/deposit", post(deposit))
        .route("/accounts/{id}/withdraw", post(withdraw))
        .route("/accounts/{id}/fees", post(apply_fee))
        .route("/accounts/{id}/loans", post(request_loan))
        .route("/accounts/{id}/loan-payments", post(pay_loan))
        .route("/transfers", post(transfer))
        .route("/audit", get(list_audit))
        .with_state(ApiState::new(app))
}

pub async fn serve(app: AppState, address: SocketAddr) -> std::io::Result<()> {
    let listener = TcpListener::bind(address).await?;
    axum::serve(listener, router(app)).await
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn bootstrap_admin(
    State(state): State<ApiState>,
    Json(payload): Json<CreateBootstrapAdminRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    let mut app = state.app.write().await;

    if app.identities.has_users() {
        record_audit(
            &mut app,
            AuditActor::System,
            AuditAction::BootstrapAdmin,
            AuditOutcome::Failure,
            None,
            "bootstrap attempted after users already exist",
        )?;
        return Err(ApiError::conflict(
            "bootstrap is only available before users exist",
        ));
    }

    if let Err(error) = app.identities.create_user(
        payload.user_id,
        payload.username.clone(),
        Role::Admin,
        None,
        &payload.password,
    ) {
        let message = error.to_string();
        record_audit(
            &mut app,
            AuditActor::System,
            AuditAction::BootstrapAdmin,
            AuditOutcome::Failure,
            Some(format!("user:{}", payload.user_id)),
            message.clone(),
        )?;
        return Err(ApiError::from_identity(error));
    }

    let session = app
        .identities
        .authenticate(payload.username, &payload.password)
        .map_err(ApiError::from_identity)?;
    record_audit(
        &mut app,
        AuditActor::from_session(&session),
        AuditAction::BootstrapAdmin,
        AuditOutcome::Success,
        Some(format!("user:{}", session.user_id())),
        "admin bootstrapped",
    )?;

    Ok(Json(SessionResponse::from_session(&session)))
}

async fn login(
    State(state): State<ApiState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    let mut app = state.app.write().await;
    let session = match app
        .identities
        .authenticate(&payload.username, &payload.password)
    {
        Ok(session) => {
            record_audit(
                &mut app,
                AuditActor::from_session(&session),
                AuditAction::Login,
                AuditOutcome::Success,
                Some(format!("user:{}", session.user_id())),
                "login succeeded",
            )?;
            session
        }
        Err(error) => {
            let message = error.to_string();
            record_audit(
                &mut app,
                AuditActor::System,
                AuditAction::Login,
                AuditOutcome::Failure,
                Some(format!("username:{}", payload.username)),
                message,
            )?;
            return Err(ApiError::from_identity(error));
        }
    };

    Ok(Json(SessionResponse::from_session(&session)))
}

async fn create_customer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(payload): Json<CreateCustomerRequest>,
) -> Result<Json<CustomerResponse>, ApiError> {
    let mut app = state.app.write().await;
    let session = authenticate_request(&app, &headers)?;
    authorize(&session, Permission::ManageIdentity)?;
    let response = {
        let customer = app
            .identities
            .create_customer(payload.id, payload.full_name, payload.email)
            .map_err(ApiError::from_identity)?;
        CustomerResponse::from_customer(customer)
    };
    record_audit(
        &mut app,
        AuditActor::from_session(&session),
        AuditAction::CreateCustomer,
        AuditOutcome::Success,
        Some(format!("customer:{}", response.id)),
        "customer created",
    )?;

    Ok(Json(response))
}

async fn list_audit(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AuditEntryResponse>>, ApiError> {
    let app = state.app.read().await;
    let session = authenticate_request(&app, &headers)?;
    authorize(&session, Permission::ManageIdentity)?;
    let entries = app
        .audit_log
        .entries()
        .iter()
        .map(AuditEntryResponse::from_entry)
        .collect();

    Ok(Json(entries))
}

async fn list_customers(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<CustomerResponse>>, ApiError> {
    let app = state.app.read().await;
    let session = authenticate_request(&app, &headers)?;
    authorize(&session, Permission::ManageIdentity)?;
    let customers = app
        .identities
        .customers()
        .map(CustomerResponse::from_customer)
        .collect();

    Ok(Json(customers))
}

async fn create_user(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(payload): Json<CreateUserRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    let mut app = state.app.write().await;
    let session = authenticate_request(&app, &headers)?;
    authorize(&session, Permission::ManageIdentity)?;
    let response = {
        let user = app
            .identities
            .create_user(
                payload.id,
                payload.username,
                payload.role,
                payload.customer_id,
                &payload.password,
            )
            .map_err(ApiError::from_identity)?;
        UserResponse::from_user(user)
    };
    record_audit(
        &mut app,
        AuditActor::from_session(&session),
        AuditAction::CreateUser,
        AuditOutcome::Success,
        Some(format!("user:{}", response.id)),
        "user created",
    )?;

    Ok(Json(response))
}

async fn list_users(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<UserResponse>>, ApiError> {
    let app = state.app.read().await;
    let session = authenticate_request(&app, &headers)?;
    authorize(&session, Permission::ManageIdentity)?;
    let users = app
        .identities
        .users()
        .map(UserResponse::from_user)
        .collect();

    Ok(Json(users))
}

async fn create_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(payload): Json<CreateAccountRequest>,
) -> Result<Json<AccountResponse>, ApiError> {
    let mut app = state.app.write().await;
    let session = authenticate_request(&app, &headers)?;
    authorize(&session, Permission::ManageAccounts)?;

    if let Some(customer_id) = payload.owner_id {
        app.identities
            .customer(customer_id)
            .map_err(ApiError::from_identity)?;
    }

    let opening_balance = non_negative_money_from_cents(payload.opening_balance_cents)?;
    app.bank
        .create_account(
            payload.id,
            payload.owner_id,
            payload.name,
            payload.email,
            opening_balance,
        )
        .map_err(ApiError::from_bank)?;
    let account = app.bank.account(payload.id).map_err(ApiError::from_bank)?;
    let response = AccountResponse::from_account(account);
    record_audit(
        &mut app,
        AuditActor::from_session(&session),
        AuditAction::CreateAccount,
        AuditOutcome::Success,
        Some(format!("account:{}", response.id)),
        "account created",
    )?;

    Ok(Json(response))
}

async fn list_accounts(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AccountResponse>>, ApiError> {
    let app = state.app.read().await;
    let session = authenticate_request(&app, &headers)?;
    let accounts = visible_accounts(&app, &session)?;

    Ok(Json(accounts))
}

async fn get_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(account_id): Path<AccountId>,
) -> Result<Json<AccountResponse>, ApiError> {
    let app = state.app.read().await;
    let session = authenticate_request(&app, &headers)?;
    ensure_account_view(&app, &session, account_id)?;
    let account = app.bank.account(account_id).map_err(ApiError::from_bank)?;

    Ok(Json(AccountResponse::from_account(account)))
}

async fn deposit(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(account_id): Path<AccountId>,
    Json(payload): Json<MoneyOperationRequest>,
) -> Result<Json<AccountResponse>, ApiError> {
    mutate_account_money(
        state,
        headers,
        account_id,
        payload.amount_cents,
        AuditAction::Deposit,
        |app, id, amount| app.bank.deposit(id, amount),
    )
    .await
}

async fn withdraw(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(account_id): Path<AccountId>,
    Json(payload): Json<MoneyOperationRequest>,
) -> Result<Json<AccountResponse>, ApiError> {
    mutate_account_money(
        state,
        headers,
        account_id,
        payload.amount_cents,
        AuditAction::Withdraw,
        |app, id, amount| app.bank.withdraw(id, amount),
    )
    .await
}

async fn apply_fee(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(account_id): Path<AccountId>,
    Json(payload): Json<MoneyOperationRequest>,
) -> Result<Json<AccountResponse>, ApiError> {
    let mut app = state.app.write().await;
    let session = authenticate_request(&app, &headers)?;
    authorize(&session, Permission::MoveMoney)?;
    let amount = money_from_cents(payload.amount_cents)?;
    app.bank
        .apply_fee(account_id, amount)
        .map_err(ApiError::from_bank)?;
    let account = app.bank.account(account_id).map_err(ApiError::from_bank)?;
    let response = AccountResponse::from_account(account);
    record_audit(
        &mut app,
        AuditActor::from_session(&session),
        AuditAction::Fee,
        AuditOutcome::Success,
        Some(format!("account:{account_id}")),
        "fee applied",
    )?;

    Ok(Json(response))
}

async fn request_loan(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(account_id): Path<AccountId>,
    Json(payload): Json<MoneyOperationRequest>,
) -> Result<Json<AccountResponse>, ApiError> {
    let mut app = state.app.write().await;
    let session = authenticate_request(&app, &headers)?;
    authorize(&session, Permission::MoveMoney)?;
    let amount = money_from_cents(payload.amount_cents)?;
    app.bank
        .request_loan(account_id, amount)
        .map_err(ApiError::from_bank)?;
    let account = app.bank.account(account_id).map_err(ApiError::from_bank)?;
    let response = AccountResponse::from_account(account);
    record_audit(
        &mut app,
        AuditActor::from_session(&session),
        AuditAction::LoanRequest,
        AuditOutcome::Success,
        Some(format!("account:{account_id}")),
        "loan requested",
    )?;

    Ok(Json(response))
}

async fn pay_loan(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(account_id): Path<AccountId>,
    Json(payload): Json<MoneyOperationRequest>,
) -> Result<Json<AccountResponse>, ApiError> {
    let mut app = state.app.write().await;
    let session = authenticate_request(&app, &headers)?;
    ensure_account_money_access(&app, &session, account_id)?;
    let amount = money_from_cents(payload.amount_cents)?;
    app.bank
        .pay_loan(account_id, amount)
        .map_err(ApiError::from_bank)?;
    let account = app.bank.account(account_id).map_err(ApiError::from_bank)?;
    let response = AccountResponse::from_account(account);
    record_audit(
        &mut app,
        AuditActor::from_session(&session),
        AuditAction::LoanPayment,
        AuditOutcome::Success,
        Some(format!("account:{account_id}")),
        "loan payment made",
    )?;

    Ok(Json(response))
}

async fn transfer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(payload): Json<TransferRequest>,
) -> Result<Json<TransferResponse>, ApiError> {
    let mut app = state.app.write().await;
    let session = authenticate_request(&app, &headers)?;
    ensure_account_money_access(&app, &session, payload.from_account_id)?;
    let amount = money_from_cents(payload.amount_cents)?;
    app.bank
        .transfer(payload.from_account_id, payload.to_account_id, amount)
        .map_err(ApiError::from_bank)?;
    let from_account = app
        .bank
        .account(payload.from_account_id)
        .map_err(ApiError::from_bank)?;
    let to_account = app
        .bank
        .account(payload.to_account_id)
        .map_err(ApiError::from_bank)?;
    let response = TransferResponse {
        from: AccountResponse::from_account(from_account),
        to: AccountResponse::from_account(to_account),
    };
    record_audit(
        &mut app,
        AuditActor::from_session(&session),
        AuditAction::Transfer,
        AuditOutcome::Success,
        Some(format!(
            "account:{}->account:{}",
            payload.from_account_id, payload.to_account_id
        )),
        "transfer completed",
    )?;

    Ok(Json(response))
}

async fn mutate_account_money(
    state: ApiState,
    headers: HeaderMap,
    account_id: AccountId,
    amount_cents: i64,
    action: AuditAction,
    operation: impl FnOnce(&mut AppState, AccountId, Money) -> BankResult<()>,
) -> Result<Json<AccountResponse>, ApiError> {
    let mut app = state.app.write().await;
    let session = authenticate_request(&app, &headers)?;
    ensure_account_money_access(&app, &session, account_id)?;
    let amount = money_from_cents(amount_cents)?;
    operation(&mut app, account_id, amount).map_err(ApiError::from_bank)?;
    let account = app.bank.account(account_id).map_err(ApiError::from_bank)?;
    let response = AccountResponse::from_account(account);
    record_audit(
        &mut app,
        AuditActor::from_session(&session),
        action,
        AuditOutcome::Success,
        Some(format!("account:{account_id}")),
        format!("{action} completed"),
    )?;

    Ok(Json(response))
}

fn authenticate_request(app: &AppState, headers: &HeaderMap) -> Result<Session, ApiError> {
    let (username, password) = basic_credentials(headers)?;

    app.identities
        .authenticate(username, &password)
        .map_err(ApiError::from_identity)
}

fn basic_credentials(headers: &HeaderMap) -> Result<(String, String), ApiError> {
    let value = headers
        .get("authorization")
        .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?
        .to_str()
        .map_err(|_| ApiError::unauthorized("invalid Authorization header"))?;

    let encoded = value
        .strip_prefix("Basic ")
        .ok_or_else(|| ApiError::unauthorized("Authorization header must use Basic auth"))?;
    let decoded = BASE64
        .decode(encoded)
        .map_err(|_| ApiError::unauthorized("invalid Basic auth payload"))?;
    let credentials = String::from_utf8(decoded)
        .map_err(|_| ApiError::unauthorized("invalid Basic auth encoding"))?;
    let (username, password) = credentials
        .split_once(':')
        .ok_or_else(|| ApiError::unauthorized("Basic auth must be username:password"))?;

    Ok((username.to_string(), password.to_string()))
}

fn authorize(session: &Session, permission: Permission) -> Result<(), ApiError> {
    IdentityStore::authorize(session, permission).map_err(ApiError::from_identity)
}

fn ensure_account_view(
    app: &AppState,
    session: &Session,
    account_id: AccountId,
) -> Result<(), ApiError> {
    ensure_account_access(
        app,
        session,
        account_id,
        Permission::ViewAnyAccount,
        Permission::ViewOwnAccount,
    )
}

fn ensure_account_money_access(
    app: &AppState,
    session: &Session,
    account_id: AccountId,
) -> Result<(), ApiError> {
    ensure_account_access(
        app,
        session,
        account_id,
        Permission::MoveMoney,
        Permission::MoveOwnMoney,
    )
}

fn ensure_account_access(
    app: &AppState,
    session: &Session,
    account_id: AccountId,
    any_permission: Permission,
    own_permission: Permission,
) -> Result<(), ApiError> {
    if session.can(any_permission) {
        return Ok(());
    }

    authorize(session, own_permission)?;
    let account = app.bank.account(account_id).map_err(ApiError::from_bank)?;

    if account.owner_id().is_some() && account.owner_id() == session.customer_id() {
        return Ok(());
    }

    Err(ApiError::forbidden(
        "account is not linked to this customer",
    ))
}

fn visible_accounts(app: &AppState, session: &Session) -> Result<Vec<AccountResponse>, ApiError> {
    if session.can(Permission::ViewAnyAccount) {
        return Ok(app
            .bank
            .accounts()
            .map(AccountResponse::from_account)
            .collect());
    }

    authorize(session, Permission::ViewOwnAccount)?;

    Ok(app
        .bank
        .accounts()
        .filter(|account| {
            account.owner_id().is_some() && account.owner_id() == session.customer_id()
        })
        .map(AccountResponse::from_account)
        .collect())
}

fn money_from_cents(cents: i64) -> Result<Money, ApiError> {
    if cents <= 0 {
        return Err(ApiError::bad_request("amount_cents must be positive"));
    }

    Ok(Money::from_cents(cents))
}

fn record_audit(
    app: &mut AppState,
    actor: AuditActor,
    action: AuditAction,
    outcome: AuditOutcome,
    target: Option<String>,
    message: impl Into<String>,
) -> Result<(), ApiError> {
    app.audit_log
        .record(actor, action, outcome, target, message)
        .map(|_| ())
        .map_err(|error| ApiError::internal_server_error(error.to_string()))
}

fn non_negative_money_from_cents(cents: i64) -> Result<Money, ApiError> {
    if cents < 0 {
        return Err(ApiError::bad_request(
            "opening_balance_cents cannot be negative",
        ));
    }

    Ok(Money::from_cents(cents))
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn internal_server_error(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    fn from_bank(error: BankError) -> Self {
        let status = match error {
            BankError::AccountNotFound(_) => StatusCode::NOT_FOUND,
            BankError::AccountAlreadyExists(_) => StatusCode::CONFLICT,
            BankError::AccountInactive(_) | BankError::AccountClosed(_) => StatusCode::CONFLICT,
            BankError::InsufficientFunds { .. } => StatusCode::CONFLICT,
            BankError::AccountHasBalance { .. } | BankError::AccountHasLoan { .. } => {
                StatusCode::CONFLICT
            }
            BankError::ArithmeticOverflow => StatusCode::INTERNAL_SERVER_ERROR,
            BankError::DuplicateTransactionId(_) | BankError::InvalidTransactionId(_) => {
                StatusCode::BAD_REQUEST
            }
            BankError::SameAccountTransfer(_)
            | BankError::InvalidAmount
            | BankError::InvalidProfileField(_)
            | BankError::NoLoan(_) => StatusCode::BAD_REQUEST,
        };

        Self {
            status,
            message: error.to_string(),
        }
    }

    fn from_identity(error: IdentityError) -> Self {
        let status = match error {
            IdentityError::InvalidCredentials => StatusCode::UNAUTHORIZED,
            IdentityError::PermissionDenied { .. } => StatusCode::FORBIDDEN,
            IdentityError::CustomerNotFound(_) | IdentityError::UserNotFound(_) => {
                StatusCode::NOT_FOUND
            }
            IdentityError::CustomerAlreadyExists(_)
            | IdentityError::UserAlreadyExists(_)
            | IdentityError::UsernameAlreadyExists(_) => StatusCode::CONFLICT,
            IdentityError::PasswordHash(_) => StatusCode::INTERNAL_SERVER_ERROR,
            IdentityError::InvalidProfileField(_)
            | IdentityError::PasswordTooWeak
            | IdentityError::RoleRequiresCustomer
            | IdentityError::RoleCannotHaveCustomer => StatusCode::BAD_REQUEST,
        };

        Self {
            status,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorResponse {
            error: self.message,
        });

        (self.status, body).into_response()
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Deserialize)]
struct CreateBootstrapAdminRequest {
    user_id: UserId,
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct CreateCustomerRequest {
    id: CustomerId,
    full_name: String,
    email: String,
}

#[derive(Debug, Deserialize)]
struct CreateUserRequest {
    id: UserId,
    username: String,
    role: Role,
    customer_id: Option<CustomerId>,
    password: String,
}

#[derive(Debug, Deserialize)]
struct CreateAccountRequest {
    id: AccountId,
    owner_id: Option<CustomerId>,
    name: String,
    email: String,
    opening_balance_cents: i64,
}

#[derive(Debug, Deserialize)]
struct MoneyOperationRequest {
    amount_cents: i64,
}

#[derive(Debug, Deserialize)]
struct TransferRequest {
    from_account_id: AccountId,
    to_account_id: AccountId,
    amount_cents: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionResponse {
    user_id: UserId,
    username: String,
    role: Role,
    customer_id: Option<CustomerId>,
}

impl SessionResponse {
    fn from_session(session: &Session) -> Self {
        Self {
            user_id: session.user_id(),
            username: session.username().to_string(),
            role: session.role(),
            customer_id: session.customer_id(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CustomerResponse {
    id: CustomerId,
    full_name: String,
    email: String,
}

impl CustomerResponse {
    fn from_customer(customer: &Customer) -> Self {
        Self {
            id: customer.id(),
            full_name: customer.full_name().to_string(),
            email: customer.email().to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct UserResponse {
    id: UserId,
    username: String,
    role: Role,
    customer_id: Option<CustomerId>,
}

impl UserResponse {
    fn from_user(user: &User) -> Self {
        Self {
            id: user.id(),
            username: user.username().to_string(),
            role: user.role(),
            customer_id: user.customer_id(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct AccountResponse {
    id: AccountId,
    owner_id: Option<CustomerId>,
    name: String,
    email: String,
    balance_cents: i64,
    balance: String,
    loan_balance_cents: i64,
    loan_balance: String,
    status: String,
    transactions: Vec<TransactionResponse>,
}

impl AccountResponse {
    fn from_account(account: &Account) -> Self {
        Self {
            id: account.id(),
            owner_id: account.owner_id(),
            name: account.name().to_string(),
            email: account.email().to_string(),
            balance_cents: account.balance().cents(),
            balance: account.balance().to_string(),
            loan_balance_cents: account.loan_balance().cents(),
            loan_balance: account.loan_balance().to_string(),
            status: account.status().to_string(),
            transactions: account
                .transactions()
                .iter()
                .map(TransactionResponse::from_transaction)
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TransactionResponse {
    id: u64,
    occurred_at_epoch_seconds: u64,
    kind: String,
    amount_cents: i64,
    amount: String,
    description: String,
}

impl TransactionResponse {
    fn from_transaction(transaction: &Transaction) -> Self {
        Self {
            id: transaction.id(),
            occurred_at_epoch_seconds: transaction.occurred_at_epoch_seconds(),
            kind: transaction.kind().to_string(),
            amount_cents: transaction.amount().cents(),
            amount: transaction.amount().to_string(),
            description: transaction.description().to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TransferResponse {
    from: AccountResponse,
    to: AccountResponse,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuditEntryResponse {
    id: u64,
    occurred_at_epoch_seconds: u64,
    actor: String,
    action: String,
    outcome: String,
    target: Option<String>,
    message: String,
}

impl AuditEntryResponse {
    fn from_entry(entry: &AuditEntry) -> Self {
        Self {
            id: entry.id(),
            occurred_at_epoch_seconds: entry.occurred_at_epoch_seconds(),
            actor: entry.actor().to_string(),
            action: entry.action().to_string(),
            outcome: entry.outcome().to_string(),
            target: entry.target().map(ToString::to_string),
            message: entry.message().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::router;
    use crate::AppState;

    fn basic(username: &str, password: &str) -> String {
        format!("Basic {}", BASE64.encode(format!("{username}:{password}")))
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn request(
        app: axum::Router,
        method: &str,
        uri: &str,
        auth: Option<(&str, &str)>,
        body: Value,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");

        if let Some((username, password)) = auth {
            builder = builder.header("authorization", basic(username, password));
        }

        let request = builder.body(Body::from(body.to_string())).unwrap();

        app.oneshot(request)
            .await
            .unwrap_or_else(|_| panic!("request failed: {method} {uri}"))
    }

    #[tokio::test]
    async fn bootstraps_admin_and_creates_account() {
        let app = router(AppState::new());

        let response = request(
            app.clone(),
            "POST",
            "/auth/bootstrap-admin",
            None,
            json!({
                "user_id": 1,
                "username": "admin",
                "password": "correct-password"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = request(
            app.clone(),
            "POST",
            "/customers",
            Some(("admin", "correct-password")),
            json!({
                "id": 10,
                "full_name": "Alice",
                "email": "alice@example.com"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = request(
            app.clone(),
            "POST",
            "/accounts",
            Some(("admin", "correct-password")),
            json!({
                "id": 100,
                "owner_id": 10,
                "name": "Alice Checking",
                "email": "alice@example.com",
                "opening_balance_cents": 5000
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["balance_cents"], 5000);
        assert_eq!(body["owner_id"], 10);
    }

    #[tokio::test]
    async fn rejects_protected_requests_without_auth() {
        let app = router(AppState::new());

        let response = request(app, "GET", "/accounts", None, json!({})).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn customer_can_only_see_owned_accounts() {
        let app = router(AppState::new());

        request(
            app.clone(),
            "POST",
            "/auth/bootstrap-admin",
            None,
            json!({
                "user_id": 1,
                "username": "admin",
                "password": "correct-password"
            }),
        )
        .await;
        request(
            app.clone(),
            "POST",
            "/customers",
            Some(("admin", "correct-password")),
            json!({"id": 10, "full_name": "Alice", "email": "alice@example.com"}),
        )
        .await;
        request(
            app.clone(),
            "POST",
            "/customers",
            Some(("admin", "correct-password")),
            json!({"id": 20, "full_name": "Bob", "email": "bob@example.com"}),
        )
        .await;
        request(
            app.clone(),
            "POST",
            "/users",
            Some(("admin", "correct-password")),
            json!({
                "id": 2,
                "username": "alice",
                "role": "customer",
                "customer_id": 10,
                "password": "correct-password"
            }),
        )
        .await;
        request(
            app.clone(),
            "POST",
            "/accounts",
            Some(("admin", "correct-password")),
            json!({
                "id": 100,
                "owner_id": 10,
                "name": "Alice Checking",
                "email": "alice@example.com",
                "opening_balance_cents": 5000
            }),
        )
        .await;
        request(
            app.clone(),
            "POST",
            "/accounts",
            Some(("admin", "correct-password")),
            json!({
                "id": 200,
                "owner_id": 20,
                "name": "Bob Checking",
                "email": "bob@example.com",
                "opening_balance_cents": 9000
            }),
        )
        .await;

        let response = request(
            app.clone(),
            "GET",
            "/accounts",
            Some(("alice", "correct-password")),
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["id"], 100);

        let response = request(
            app,
            "GET",
            "/accounts/200",
            Some(("alice", "correct-password")),
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn transfer_updates_balances() {
        let app = router(AppState::new());

        request(
            app.clone(),
            "POST",
            "/auth/bootstrap-admin",
            None,
            json!({
                "user_id": 1,
                "username": "admin",
                "password": "correct-password"
            }),
        )
        .await;
        request(
            app.clone(),
            "POST",
            "/accounts",
            Some(("admin", "correct-password")),
            json!({
                "id": 100,
                "owner_id": null,
                "name": "Operations",
                "email": "ops@example.com",
                "opening_balance_cents": 10000
            }),
        )
        .await;
        request(
            app.clone(),
            "POST",
            "/accounts",
            Some(("admin", "correct-password")),
            json!({
                "id": 200,
                "owner_id": null,
                "name": "Reserve",
                "email": "reserve@example.com",
                "opening_balance_cents": 500
            }),
        )
        .await;

        let response = request(
            app,
            "POST",
            "/transfers",
            Some(("admin", "correct-password")),
            json!({
                "from_account_id": 100,
                "to_account_id": 200,
                "amount_cents": 2500
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["from"]["balance_cents"], 7500);
        assert_eq!(body["to"]["balance_cents"], 3000);
    }

    #[tokio::test]
    async fn audit_endpoint_returns_recorded_events() {
        let app = router(AppState::new());

        request(
            app.clone(),
            "POST",
            "/auth/bootstrap-admin",
            None,
            json!({
                "user_id": 1,
                "username": "admin",
                "password": "correct-password"
            }),
        )
        .await;
        request(
            app.clone(),
            "POST",
            "/accounts",
            Some(("admin", "correct-password")),
            json!({
                "id": 100,
                "owner_id": null,
                "name": "Operations",
                "email": "ops@example.com",
                "opening_balance_cents": 10000
            }),
        )
        .await;

        let response = request(
            app,
            "GET",
            "/audit",
            Some(("admin", "correct-password")),
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let entries = body.as_array().unwrap();

        assert!(entries.len() >= 2);
        assert_eq!(entries[0]["action"], "bootstrap_admin");
        assert_eq!(entries[1]["action"], "create_account");
    }
}
