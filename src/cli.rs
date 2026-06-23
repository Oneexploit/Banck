use std::io::{self, Write};

use crate::{
    app::AppState,
    bank::{BankError, BankResult},
    domain::{Account, AccountId, CustomerId, InterestRate, Money},
    identity::{IdentityError, IdentityStore, Permission, Role, Session, UserId},
    storage::{export_bank_statement_file, load_app_from_json_file, save_app_to_json_file},
};

pub fn run() {
    let mut state = AppState::new();
    let mut session = None;
    command_line_interface(&mut state, &mut session);
}

fn command_line_interface(state: &mut AppState, session: &mut Option<Session>) {
    loop {
        println!();
        print_session_summary(session);
        println!("Commands:");
        println!("bootstrap_admin, login, logout, session");
        println!("create_customer, create_user, list_customers, list_users");
        println!("create, update_name, update_email, activate, deactivate, close, delete");
        println!("deposit, withdraw, transfer, fee, monthly_fee, interest");
        println!("loan_request, pay_loan");
        println!("balance, info, list, search, history, statement");
        println!("total_balance, richest, empty_accounts, count");
        println!("save, load, export, exit");

        let command = read_text("Enter command: ").to_lowercase();

        match command.as_str() {
            "bootstrap_admin" => bootstrap_admin(state, session),
            "login" => login(state, session),
            "logout" => {
                *session = None;
                println!("Logged out");
            }
            "session" => print_session_details(session),
            "create_customer" => {
                if !authorize(session, Permission::ManageIdentity) {
                    continue;
                }

                let id = read_customer_id("Customer id: ");
                let full_name = read_text("Full name: ");
                let email = read_text("Email: ");

                match state.identities.create_customer(id, full_name, email) {
                    Ok(_) => println!("Customer created"),
                    Err(error) => print_identity_error("Customer creation failed", error),
                }
            }
            "create_user" => {
                if !authorize(session, Permission::ManageIdentity) {
                    continue;
                }

                let id = read_user_id("User id: ");
                let username = read_text("Username: ");
                let role = read_role("Role (customer, teller, admin): ");
                let customer_id = if role == Role::Customer {
                    Some(read_customer_id("Linked customer id: "))
                } else {
                    None
                };
                let password = read_text("Password: ");

                match state
                    .identities
                    .create_user(id, username, role, customer_id, &password)
                {
                    Ok(_) => println!("User created"),
                    Err(error) => print_identity_error("User creation failed", error),
                }
            }
            "list_customers" => {
                if !authorize(session, Permission::ManageIdentity) {
                    continue;
                }

                list_customers(&state.identities);
            }
            "list_users" => {
                if !authorize(session, Permission::ManageIdentity) {
                    continue;
                }

                list_users(&state.identities);
            }
            "create" => {
                if !authorize(session, Permission::ManageAccounts) {
                    continue;
                }

                let id = read_account_id("Account id: ");
                let owner_id = read_optional_customer_id("Owner customer id (blank for none): ");

                if !customer_exists(&state.identities, owner_id) {
                    continue;
                }

                let name = read_text("Account name: ");
                let email = read_text("Account email: ");
                let opening_balance = read_non_negative_money("Starting balance: ");

                match state
                    .bank
                    .create_account(id, owner_id, name, email, opening_balance)
                {
                    Ok(_) => println!("Account created"),
                    Err(error) => print_bank_error("Create failed", error),
                }
            }
            "update_name" => {
                if !authorize(session, Permission::ManageAccounts) {
                    continue;
                }

                let id = read_account_id("Account id: ");
                let name = read_text("New name: ");
                print_bank_result(state.bank.update_name(id, name), "Name updated");
            }
            "update_email" => {
                if !authorize(session, Permission::ManageAccounts) {
                    continue;
                }

                let id = read_account_id("Account id: ");
                let email = read_text("New email: ");
                print_bank_result(state.bank.update_email(id, email), "Email updated");
            }
            "activate" => {
                if !authorize(session, Permission::ManageAccounts) {
                    continue;
                }

                let id = read_account_id("Account id: ");
                print_bank_result(state.bank.activate_account(id), "Account activated");
            }
            "deactivate" => {
                if !authorize(session, Permission::ManageAccounts) {
                    continue;
                }

                let id = read_account_id("Account id: ");
                print_bank_result(state.bank.deactivate_account(id), "Account deactivated");
            }
            "close" => {
                if !authorize(session, Permission::ManageAccounts) {
                    continue;
                }

                let id = read_account_id("Account id: ");
                print_bank_result(state.bank.close_account(id), "Account closed");
            }
            "delete" => {
                if !authorize(session, Permission::ManageAccounts) {
                    continue;
                }

                let id = read_account_id("Account id: ");

                match state.bank.delete_account(id) {
                    Ok(_) => println!("Account deleted"),
                    Err(error) => print_bank_error("Delete failed", error),
                }
            }
            "deposit" => {
                let id = read_account_id("Account id: ");

                if !authorize_account_money_operation(state, session, id) {
                    continue;
                }

                let amount = read_positive_money("Amount: ");
                print_bank_result(state.bank.deposit(id, amount), "Deposit completed");
            }
            "withdraw" => {
                let id = read_account_id("Account id: ");

                if !authorize_account_money_operation(state, session, id) {
                    continue;
                }

                let amount = read_positive_money("Amount: ");
                print_bank_result(state.bank.withdraw(id, amount), "Withdraw completed");
            }
            "transfer" => {
                let from_id = read_account_id("From account id: ");

                if !authorize_account_money_operation(state, session, from_id) {
                    continue;
                }

                let to_id = read_account_id("To account id: ");
                let amount = read_positive_money("Amount: ");
                print_bank_result(
                    state.bank.transfer(from_id, to_id, amount),
                    "Transfer completed",
                );
            }
            "fee" => {
                if !authorize(session, Permission::MoveMoney) {
                    continue;
                }

                let id = read_account_id("Account id: ");
                let amount = read_positive_money("Fee amount: ");
                print_bank_result(state.bank.apply_fee(id, amount), "Fee applied");
            }
            "monthly_fee" => {
                if !authorize(session, Permission::MoveMoney) {
                    continue;
                }

                let amount = read_positive_money("Monthly fee amount: ");

                for (id, result) in state.bank.apply_monthly_fee(amount) {
                    match result {
                        Ok(()) => println!("Fee applied to account {id}"),
                        Err(error) => {
                            print_bank_error(&format!("Fee failed for account {id}"), error)
                        }
                    }
                }
            }
            "interest" => {
                if !authorize(session, Permission::MoveMoney) {
                    continue;
                }

                let id = read_account_id("Account id: ");
                let rate = read_positive_interest_rate("Interest percent: ");

                match state.bank.apply_interest(id, rate) {
                    Ok(interest) => println!("Interest added: {interest}"),
                    Err(error) => print_bank_error("Interest failed", error),
                }
            }
            "loan_request" => {
                if !authorize(session, Permission::MoveMoney) {
                    continue;
                }

                let id = read_account_id("Account id: ");
                let amount = read_positive_money("Loan amount: ");
                print_bank_result(state.bank.request_loan(id, amount), "Loan received");
            }
            "pay_loan" => {
                let id = read_account_id("Account id: ");

                if !authorize_account_money_operation(state, session, id) {
                    continue;
                }

                let amount = read_positive_money("Payment amount: ");

                match state.bank.pay_loan(id, amount) {
                    Ok(payment) => println!("Loan payment completed: {payment}"),
                    Err(error) => print_bank_error("Loan payment failed", error),
                }
            }
            "balance" => {
                let id = read_account_id("Account id: ");

                if !authorize_account_view(state, session, id) {
                    continue;
                }

                match state.bank.account(id) {
                    Ok(account) => println!("Balance: {}", account.balance()),
                    Err(error) => print_bank_error("Balance lookup failed", error),
                }
            }
            "info" | "search" => {
                let id = read_account_id("Account id: ");

                if !authorize_account_view(state, session, id) {
                    continue;
                }

                match state.bank.account(id) {
                    Ok(account) => println!("{}", account_info(account)),
                    Err(error) => print_bank_error("Account lookup failed", error),
                }
            }
            "list" => list_accounts(state, session),
            "history" | "statement" => {
                let id = read_account_id("Account id: ");

                if !authorize_account_view(state, session, id) {
                    continue;
                }

                match state.bank.account(id) {
                    Ok(account) => print_account_statement(account),
                    Err(error) => print_bank_error("Statement failed", error),
                }
            }
            "total_balance" => {
                if !authorize(session, Permission::ViewAnyAccount) {
                    continue;
                }

                match state.bank.total_balance() {
                    Ok(total) => println!("Total balance: {total}"),
                    Err(error) => print_bank_error("Total balance failed", error),
                }
            }
            "richest" => {
                if !authorize(session, Permission::ViewAnyAccount) {
                    continue;
                }

                match state.bank.richest_account() {
                    Some(account) => println!("Richest account: {}", account_info(account)),
                    None => println!("No accounts found"),
                }
            }
            "empty_accounts" => {
                if !authorize(session, Permission::ViewAnyAccount) {
                    continue;
                }

                print_empty_accounts(state);
            }
            "count" => {
                if !authorize(session, Permission::ViewAnyAccount) {
                    continue;
                }

                println!("Account count: {}", state.bank.account_count());
            }
            "save" => {
                if !authorize(session, Permission::PersistState) {
                    continue;
                }

                let path = read_text("JSON file path: ");

                match save_app_to_json_file(state, &path) {
                    Ok(()) => println!("Application state saved"),
                    Err(error) => println!("Save failed: {error}"),
                }
            }
            "load" => {
                if session.is_some() && !authorize(session, Permission::PersistState) {
                    continue;
                }

                let path = read_text("JSON file path: ");

                match load_app_from_json_file(&path) {
                    Ok(loaded_state) => {
                        *state = loaded_state;
                        *session = None;
                        println!(
                            "Application state loaded. Account count: {}. Please log in again.",
                            state.bank.account_count()
                        );
                    }
                    Err(error) => println!("Load failed: {error}"),
                }
            }
            "export" => {
                if !authorize(session, Permission::ExportStatements) {
                    continue;
                }

                let path = read_text("Statement file path: ");

                match export_bank_statement_file(&state.bank, &path) {
                    Ok(()) => println!("Statement exported"),
                    Err(error) => println!("Export failed: {error}"),
                }
            }
            "exit" => {
                println!("Goodbye");
                break;
            }
            "" => {}
            _ => println!("Unknown command"),
        }
    }
}

fn bootstrap_admin(state: &mut AppState, session: &mut Option<Session>) {
    if state.identities.has_users() {
        println!("Bootstrap is only available before any users exist");
        return;
    }

    let id = read_user_id("Admin user id: ");
    let username = read_text("Admin username: ");
    let password = read_text("Admin password: ");

    match state
        .identities
        .create_user(id, username.clone(), Role::Admin, None, &password)
    {
        Ok(_) => match state.identities.authenticate(&username, &password) {
            Ok(created_session) => {
                *session = Some(created_session);
                println!("Admin created and logged in");
            }
            Err(error) => print_identity_error("Automatic login failed", error),
        },
        Err(error) => print_identity_error("Admin bootstrap failed", error),
    }
}

fn login(state: &AppState, session: &mut Option<Session>) {
    let username = read_text("Username: ");
    let password = read_text("Password: ");

    match state.identities.authenticate(username, &password) {
        Ok(created_session) => {
            println!(
                "Logged in as {} ({})",
                created_session.username(),
                created_session.role()
            );
            *session = Some(created_session);
        }
        Err(error) => print_identity_error("Login failed", error),
    }
}

fn authorize(session: &Option<Session>, permission: Permission) -> bool {
    let Some(session) = session else {
        println!("Please login first");
        return false;
    };

    match IdentityStore::authorize(session, permission) {
        Ok(()) => true,
        Err(error) => {
            print_identity_error("Permission denied", error);
            false
        }
    }
}

fn authorize_account_view(
    state: &AppState,
    session: &Option<Session>,
    account_id: AccountId,
) -> bool {
    authorize_account_access(
        state,
        session,
        account_id,
        Permission::ViewAnyAccount,
        Permission::ViewOwnAccount,
    )
}

fn authorize_account_money_operation(
    state: &AppState,
    session: &Option<Session>,
    account_id: AccountId,
) -> bool {
    authorize_account_access(
        state,
        session,
        account_id,
        Permission::MoveMoney,
        Permission::MoveOwnMoney,
    )
}

fn authorize_account_access(
    state: &AppState,
    session: &Option<Session>,
    account_id: AccountId,
    any_permission: Permission,
    own_permission: Permission,
) -> bool {
    let Some(session) = session else {
        println!("Please login first");
        return false;
    };

    if session.can(any_permission) {
        return true;
    }

    if !session.can(own_permission) {
        print_identity_error(
            "Permission denied",
            IdentityError::PermissionDenied {
                role: session.role(),
                permission: own_permission,
            },
        );
        return false;
    }

    let account = match state.bank.account(account_id) {
        Ok(account) => account,
        Err(error) => {
            print_bank_error("Account lookup failed", error);
            return false;
        }
    };

    if account.owner_id().is_some() && account.owner_id() == session.customer_id() {
        return true;
    }

    println!("Permission denied: account {account_id} is not linked to your customer profile");
    false
}

fn customer_exists(identities: &IdentityStore, customer_id: Option<CustomerId>) -> bool {
    let Some(customer_id) = customer_id else {
        return true;
    };

    match identities.customer(customer_id) {
        Ok(_) => true,
        Err(error) => {
            print_identity_error("Owner lookup failed", error);
            false
        }
    }
}

fn account_info(account: &Account) -> String {
    let owner = account
        .owner_id()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "none".to_string());

    format!(
        "ID: {}, Owner: {}, Name: {}, Email: {}, Balance: {}, Loan: {}, Status: {}",
        account.id(),
        owner,
        account.name(),
        account.email(),
        account.balance(),
        account.loan_balance(),
        account.status()
    )
}

fn list_accounts(state: &AppState, session: &Option<Session>) {
    let Some(session) = session else {
        println!("Please login first");
        return;
    };

    if state.bank.is_empty() {
        println!("No accounts found");
        return;
    }

    if session.can(Permission::ViewAnyAccount) {
        for account in state.bank.accounts() {
            println!("{}", account_info(account));
        }
        return;
    }

    if !session.can(Permission::ViewOwnAccount) {
        print_identity_error(
            "Permission denied",
            IdentityError::PermissionDenied {
                role: session.role(),
                permission: Permission::ViewOwnAccount,
            },
        );
        return;
    }

    let mut found = false;

    for account in state.bank.accounts() {
        if account.owner_id().is_some() && account.owner_id() == session.customer_id() {
            println!("{}", account_info(account));
            found = true;
        }
    }

    if !found {
        println!("No accounts found for this customer");
    }
}

fn list_customers(identities: &IdentityStore) {
    let mut found = false;

    for customer in identities.customers() {
        println!(
            "ID: {}, Name: {}, Email: {}",
            customer.id(),
            customer.full_name(),
            customer.email()
        );
        found = true;
    }

    if !found {
        println!("No customers found");
    }
}

fn list_users(identities: &IdentityStore) {
    let mut found = false;

    for user in identities.users() {
        let customer = user
            .customer_id()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".to_string());

        println!(
            "ID: {}, Username: {}, Role: {}, Customer: {}",
            user.id(),
            user.username(),
            user.role(),
            customer
        );
        found = true;
    }

    if !found {
        println!("No users found");
    }
}

fn print_empty_accounts(state: &AppState) {
    let accounts = state.bank.empty_accounts();

    if accounts.is_empty() {
        println!("No empty accounts found");
        return;
    }

    for account in accounts {
        println!("{}", account_info(account));
    }
}

fn print_account_statement(account: &Account) {
    println!("{}", account_info(account));
    println!("Transactions:");

    if account.transactions().is_empty() {
        println!("No transactions found");
        return;
    }

    for transaction in account.transactions() {
        println!(
            "- {} | Amount: {} | {}",
            transaction.kind(),
            transaction.amount(),
            transaction.description()
        );
    }
}

fn print_session_summary(session: &Option<Session>) {
    match session {
        Some(session) => println!("Logged in: {} ({})", session.username(), session.role()),
        None => println!("Logged in: no"),
    }
}

fn print_session_details(session: &Option<Session>) {
    match session {
        Some(session) => {
            let customer = session
                .customer_id()
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string());

            println!(
                "User: {} | Username: {} | Role: {} | Customer: {}",
                session.user_id(),
                session.username(),
                session.role(),
                customer
            );
        }
        None => println!("No active session"),
    }
}

fn print_bank_result(result: BankResult<()>, success_message: &str) {
    match result {
        Ok(()) => println!("{success_message}"),
        Err(error) => print_bank_error("Operation failed", error),
    }
}

fn print_bank_error(context: &str, error: BankError) {
    println!("{context}: {error}");
}

fn print_identity_error(context: &str, error: IdentityError) {
    println!("{context}: {error}");
}

fn read_text(message: &str) -> String {
    print!("{message}");
    io::stdout().flush().expect("failed to flush stdout");

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("failed to read line");

    input.trim().to_string()
}

fn read_account_id(message: &str) -> AccountId {
    loop {
        let input = read_text(message);

        match input.parse::<AccountId>() {
            Ok(value) => return value,
            Err(_) => println!("Please enter a valid account id"),
        }
    }
}

fn read_customer_id(message: &str) -> CustomerId {
    loop {
        let input = read_text(message);

        match input.parse::<CustomerId>() {
            Ok(value) => return value,
            Err(_) => println!("Please enter a valid customer id"),
        }
    }
}

fn read_user_id(message: &str) -> UserId {
    loop {
        let input = read_text(message);

        match input.parse::<UserId>() {
            Ok(value) => return value,
            Err(_) => println!("Please enter a valid user id"),
        }
    }
}

fn read_optional_customer_id(message: &str) -> Option<CustomerId> {
    loop {
        let input = read_text(message);

        if input.is_empty() {
            return None;
        }

        match input.parse::<CustomerId>() {
            Ok(value) => return Some(value),
            Err(_) => println!("Please enter a valid customer id or leave it blank"),
        }
    }
}

fn read_money(message: &str) -> Money {
    loop {
        let input = read_text(message);

        match input.parse::<Money>() {
            Ok(value) => return value,
            Err(error) => println!("Invalid money value: {error}"),
        }
    }
}

fn read_non_negative_money(message: &str) -> Money {
    loop {
        let value = read_money(message);

        if value >= Money::ZERO {
            return value;
        }

        println!("Amount cannot be negative");
    }
}

fn read_positive_money(message: &str) -> Money {
    loop {
        let value = read_money(message);

        if value > Money::ZERO {
            return value;
        }

        println!("Amount must be positive");
    }
}

fn read_positive_interest_rate(message: &str) -> InterestRate {
    loop {
        let input = read_text(message);

        match input.parse::<InterestRate>() {
            Ok(rate) if !rate.is_zero() => return rate,
            Ok(_) => println!("Interest rate must be positive"),
            Err(error) => println!("Invalid interest rate: {error}"),
        }
    }
}

fn read_role(message: &str) -> Role {
    loop {
        let input = read_text(message).to_lowercase();

        match input.as_str() {
            "customer" => return Role::Customer,
            "teller" => return Role::Teller,
            "admin" => return Role::Admin,
            _ => println!("Please enter customer, teller, or admin"),
        }
    }
}
