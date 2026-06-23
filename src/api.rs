use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{
        Arc, Once,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use tokio::{
    net::TcpListener,
    sync::{Mutex, RwLock},
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::{
    app::AppState,
    audit::{AuditAction, AuditActor, AuditEntry, AuditOutcome},
    bank::{BankError, BankResult},
    domain::{Account, AccountId, CustomerId, Money, Transaction, current_epoch_seconds},
    identity::{Customer, IdentityError, IdentityStore, Permission, Role, Session, User, UserId},
};

const REQUEST_ID_HEADER: &str = "x-request-id";
const FAILED_LOGIN_LIMIT: usize = 5;
const FAILED_LOGIN_WINDOW_SECONDS: u64 = 15 * 60;
const FAILED_LOGIN_LOCKOUT_SECONDS: u64 = 15 * 60;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
static TRACING_INIT: Once = Once::new();

#[derive(Clone)]
pub struct ApiState {
    app: Arc<RwLock<AppState>>,
    auth_throttle: Arc<Mutex<AuthThrottle>>,
}

impl ApiState {
    pub fn new(app: AppState) -> Self {
        Self {
            app: Arc::new(RwLock::new(app)),
            auth_throttle: Arc::new(Mutex::new(AuthThrottle::new())),
        }
    }
}

pub fn init_tracing() {
    TRACING_INIT.call_once(|| {
        let filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("bank=info"));
        let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    });
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
        .route(
            "/accounts/{id}/transactions",
            get(list_account_transactions),
        )
        .route("/accounts/{id}/deposit", post(deposit))
        .route("/accounts/{id}/withdraw", post(withdraw))
        .route("/accounts/{id}/fees", post(apply_fee))
        .route("/accounts/{id}/loans", post(request_loan))
        .route("/accounts/{id}/loan-payments", post(pay_loan))
        .route("/transfers", post(transfer))
        .route("/audit", get(list_audit))
        .with_state(ApiState::new(app))
        .layer(middleware::from_fn(request_context))
}

pub async fn serve(app: AppState, address: SocketAddr) -> std::io::Result<()> {
    let listener = TcpListener::bind(address).await?;
    info!(%address, "api server listening");
    axum::serve(listener, router(app)).await
}

async fn request_context(request: Request<Body>, next: Next) -> Response {
    let request_id = request_id_from_headers(request.headers()).unwrap_or_else(next_request_id);
    let method = request.method().clone();
    let uri = request.uri().clone();
    let started = Instant::now();
    let mut response = next.run(request).await;
    let status = response.status();
    let latency_ms = started.elapsed().as_millis();

    if let Ok(header_value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(REQUEST_ID_HEADER, header_value);
    }

    info!(
        request_id = %request_id,
        method = %method,
        uri = %uri,
        status = status.as_u16(),
        latency_ms,
        "request completed"
    );

    response
}

fn request_id_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn next_request_id() -> String {
    let sequence = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("req-{}-{sequence}", current_epoch_seconds())
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
    let username = payload.username.clone();
    state
        .auth_throttle
        .lock()
        .await
        .ensure_allowed(&username, current_epoch_seconds())?;

    let mut app = state.app.write().await;
    let session = match app
        .identities
        .authenticate(&payload.username, &payload.password)
    {
        Ok(session) => {
            state.auth_throttle.lock().await.record_success(&username);
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
            state
                .auth_throttle
                .lock()
                .await
                .record_failure(&username, current_epoch_seconds());
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
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEntryResponse>>, ApiError> {
    let app = state.app.read().await;
    let session = authenticate_request(&app, &headers)?;
    authorize(&session, Permission::ManageIdentity)?;
    let limit = validated_limit(query.limit)?;
    validate_time_window(query.from_epoch_seconds, query.to_epoch_seconds)?;
    let entries = app
        .audit_log
        .entries()
        .iter()
        .filter(|entry| audit_matches_query(entry, &query));
    let entries = ordered(entries, query.order.unwrap_or(QueryOrder::Desc))
        .into_iter()
        .take(limit)
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

async fn list_account_transactions(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(account_id): Path<AccountId>,
    Query(query): Query<TransactionQuery>,
) -> Result<Json<Vec<TransactionResponse>>, ApiError> {
    let app = state.app.read().await;
    let session = authenticate_request(&app, &headers)?;
    ensure_account_view(&app, &session, account_id)?;
    let limit = validated_limit(query.limit)?;
    validate_time_window(query.from_epoch_seconds, query.to_epoch_seconds)?;
    let account = app.bank.account(account_id).map_err(ApiError::from_bank)?;
    let transactions = account
        .transactions()
        .iter()
        .filter(|transaction| transaction_matches_query(transaction, &query));
    let transactions = ordered(transactions, query.order.unwrap_or(QueryOrder::Desc))
        .into_iter()
        .take(limit)
        .map(TransactionResponse::from_transaction)
        .collect();

    Ok(Json(transactions))
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

fn validated_limit(limit: Option<usize>) -> Result<usize, ApiError> {
    let limit = limit.unwrap_or(100);

    if !(1..=500).contains(&limit) {
        return Err(ApiError::bad_request("limit must be between 1 and 500"));
    }

    Ok(limit)
}

fn validate_time_window(from: Option<u64>, to: Option<u64>) -> Result<(), ApiError> {
    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        return Err(ApiError::bad_request(
            "from_epoch_seconds must be less than or equal to to_epoch_seconds",
        ));
    }

    Ok(())
}

fn ordered<T>(items: impl Iterator<Item = T>, order: QueryOrder) -> Vec<T> {
    let mut items: Vec<T> = items.collect();

    if order == QueryOrder::Desc {
        items.reverse();
    }

    items
}

fn transaction_matches_query(transaction: &Transaction, query: &TransactionQuery) -> bool {
    if let Some(kind) = query.kind.as_deref()
        && kind != transaction.kind().to_string()
    {
        return false;
    }

    timestamp_matches_query(
        transaction.occurred_at_epoch_seconds(),
        query.from_epoch_seconds,
        query.to_epoch_seconds,
    )
}

fn audit_matches_query(entry: &AuditEntry, query: &AuditQuery) -> bool {
    if let Some(action) = query.action.as_deref()
        && action != entry.action().to_string()
    {
        return false;
    }

    if let Some(outcome) = query.outcome.as_deref()
        && outcome != entry.outcome().to_string()
    {
        return false;
    }

    if let Some(target) = query.target.as_deref()
        && entry.target() != Some(target)
    {
        return false;
    }

    timestamp_matches_query(
        entry.occurred_at_epoch_seconds(),
        query.from_epoch_seconds,
        query.to_epoch_seconds,
    )
}

fn timestamp_matches_query(
    timestamp: u64,
    from_epoch_seconds: Option<u64>,
    to_epoch_seconds: Option<u64>,
) -> bool {
    if let Some(from_epoch_seconds) = from_epoch_seconds
        && timestamp < from_epoch_seconds
    {
        return false;
    }

    if let Some(to_epoch_seconds) = to_epoch_seconds
        && timestamp > to_epoch_seconds
    {
        return false;
    }

    true
}

#[derive(Debug, Default)]
struct AuthThrottle {
    failures_by_username: BTreeMap<String, FailedLoginRecord>,
}

impl AuthThrottle {
    fn new() -> Self {
        Self::default()
    }

    fn ensure_allowed(&mut self, username: &str, now: u64) -> Result<(), ApiError> {
        let username = throttle_key(username);
        self.expire_if_needed(&username, now);

        if let Some(record) = self.failures_by_username.get(&username)
            && let Some(locked_until) = record.locked_until_epoch_seconds
            && now < locked_until
        {
            return Err(ApiError::too_many_requests(
                "too many failed login attempts; try again later",
            ));
        }

        Ok(())
    }

    fn record_success(&mut self, username: &str) {
        self.failures_by_username.remove(&throttle_key(username));
    }

    fn record_failure(&mut self, username: &str, now: u64) {
        let username = throttle_key(username);
        self.expire_if_needed(&username, now);
        let record = self
            .failures_by_username
            .entry(username.clone())
            .or_insert_with(|| FailedLoginRecord::new(now));
        record.failed_attempts += 1;

        if record.failed_attempts >= FAILED_LOGIN_LIMIT {
            let locked_until = now.saturating_add(FAILED_LOGIN_LOCKOUT_SECONDS);
            record.locked_until_epoch_seconds = Some(locked_until);
            warn!(
                username = %username,
                failed_attempts = record.failed_attempts,
                locked_until_epoch_seconds = locked_until,
                "login temporarily locked"
            );
        }
    }

    fn expire_if_needed(&mut self, username: &str, now: u64) {
        let should_expire = self
            .failures_by_username
            .get(username)
            .map(|record| {
                record
                    .locked_until_epoch_seconds
                    .is_some_and(|locked_until| now >= locked_until)
                    || now.saturating_sub(record.first_failed_at_epoch_seconds)
                        >= FAILED_LOGIN_WINDOW_SECONDS
            })
            .unwrap_or(false);

        if should_expire {
            self.failures_by_username.remove(username);
        }
    }
}

#[derive(Debug)]
struct FailedLoginRecord {
    first_failed_at_epoch_seconds: u64,
    failed_attempts: usize,
    locked_until_epoch_seconds: Option<u64>,
}

impl FailedLoginRecord {
    fn new(now: u64) -> Self {
        Self {
            first_failed_at_epoch_seconds: now,
            failed_attempts: 0,
            locked_until_epoch_seconds: None,
        }
    }
}

fn throttle_key(username: &str) -> String {
    username.trim().to_ascii_lowercase()
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

    fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum QueryOrder {
    Asc,
    Desc,
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    action: Option<String>,
    outcome: Option<String>,
    target: Option<String>,
    from_epoch_seconds: Option<u64>,
    to_epoch_seconds: Option<u64>,
    limit: Option<usize>,
    order: Option<QueryOrder>,
}

#[derive(Debug, Deserialize)]
struct TransactionQuery {
    kind: Option<String>,
    from_epoch_seconds: Option<u64>,
    to_epoch_seconds: Option<u64>,
    limit: Option<usize>,
    order: Option<QueryOrder>,
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
    async fn responses_include_request_id_header() {
        let app = router(AppState::new());
        let request = Request::builder()
            .method("GET")
            .uri("/health")
            .header("x-request-id", "test-request-id")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-request-id")
                .unwrap()
                .to_str()
                .unwrap(),
            "test-request-id"
        );
    }

    #[tokio::test]
    async fn login_is_temporarily_limited_after_repeated_failures() {
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

        for _ in 0..5 {
            let response = request(
                app.clone(),
                "POST",
                "/auth/login",
                None,
                json!({
                    "username": "admin",
                    "password": "wrong-password"
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let response = request(
            app,
            "POST",
            "/auth/login",
            None,
            json!({
                "username": "admin",
                "password": "wrong-password"
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
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
            "/audit?order=asc",
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

    #[tokio::test]
    async fn audit_endpoint_filters_recorded_events() {
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
            "/audit?action=create_account&outcome=success&target=account%3A100&limit=1",
            Some(("admin", "correct-password")),
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let entries = body.as_array().unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["action"], "create_account");
        assert_eq!(entries[0]["target"], "account:100");
    }

    #[tokio::test]
    async fn account_transactions_endpoint_filters_orders_and_limits() {
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
            "/accounts/100/deposit",
            Some(("admin", "correct-password")),
            json!({"amount_cents": 2500}),
        )
        .await;
        request(
            app.clone(),
            "POST",
            "/accounts/100/withdraw",
            Some(("admin", "correct-password")),
            json!({"amount_cents": 500}),
        )
        .await;

        let response = request(
            app.clone(),
            "GET",
            "/accounts/100/transactions?kind=deposit&limit=1",
            Some(("admin", "correct-password")),
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let transactions = body.as_array().unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0]["kind"], "deposit");
        assert_eq!(transactions[0]["amount_cents"], 2500);

        let response = request(
            app,
            "GET",
            "/accounts/100/transactions?limit=2&order=desc",
            Some(("admin", "correct-password")),
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let transactions = body.as_array().unwrap();
        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0]["kind"], "withdrawal");
        assert_eq!(transactions[1]["kind"], "deposit");
    }
}
