use std::{
    fs,
    io::{self, Write},
};

struct Transaction {
    transaction_type: String,
    amount: f64,
    description: String,
}

struct Account {
    id: u32,
    name: String,
    email: String,
    balance: f64,
    is_active: bool,
    is_closed: bool,
    loan_balance: f64,
    transactions: Vec<Transaction>,
}

fn main() {
    let mut accounts: Vec<Account> = Vec::new();
    command_line_interface(&mut accounts);
}

fn create_account(id: u32, name: String, email: String, balance: f64) -> Account {
    let mut account = Account {
        id,
        name,
        email,
        balance,
        is_active: true,
        is_closed: false,
        loan_balance: 0.0,
        transactions: Vec::new(),
    };

    log_transaction(&mut account, "create", balance, "Account created");
    account
}

fn deposit(account: &mut Account, amount: f64) -> bool {
    if !can_do_money_operation(account) {
        return false;
    }

    account.balance += amount;
    log_transaction(account, "deposit", amount, "Money deposited");
    true
}

fn withdraw(account: &mut Account, amount: f64) -> bool {
    if !can_do_money_operation(account) {
        return false;
    }

    if account.balance >= amount {
        account.balance -= amount;
        log_transaction(account, "withdraw", amount, "Money withdrawn");
        true
    } else {
        println!("Insufficient funds");
        false
    }
}

fn transfer(from: &mut Account, to: &mut Account, amount: f64) -> bool {
    if !can_do_money_operation(from) || !can_do_money_operation(to) {
        return false;
    }

    if from.balance >= amount {
        from.balance -= amount;
        to.balance += amount;

        log_transaction(
            from,
            "transfer_out",
            amount,
            &format!("Transferred to account {}", to.id),
        );
        log_transaction(
            to,
            "transfer_in",
            amount,
            &format!("Received from account {}", from.id),
        );

        true
    } else {
        println!("Insufficient funds");
        false
    }
}

fn delete_account(accounts: &mut Vec<Account>, id: u32) -> bool {
    if let Some(index) = accounts.iter().position(|account| account.id == id) {
        accounts.remove(index);
        true
    } else {
        false
    }
}

fn close_account(account: &mut Account) -> bool {
    if account.balance != 0.0 {
        println!("Balance must be zero before closing the account");
        return false;
    }

    account.is_active = false;
    account.is_closed = true;
    log_transaction(account, "close", 0.0, "Account closed");
    true
}

fn activate_account(account: &mut Account) -> bool {
    if account.is_closed {
        println!("Closed accounts cannot be activated");
        return false;
    }

    account.is_active = true;
    log_transaction(account, "activate", 0.0, "Account activated");
    true
}

fn deactivate_account(account: &mut Account) {
    account.is_active = false;
    log_transaction(account, "deactivate", 0.0, "Account deactivated");
}

fn update_name(account: &mut Account, name: String) {
    account.name = name;
    log_transaction(account, "update_name", 0.0, "Name updated");
}

fn update_email(account: &mut Account, email: String) {
    account.email = email;
    log_transaction(account, "update_email", 0.0, "Email updated");
}

fn apply_fee(account: &mut Account, amount: f64) -> bool {
    if !can_do_money_operation(account) {
        return false;
    }

    if account.balance >= amount {
        account.balance -= amount;
        log_transaction(account, "fee", amount, "Bank fee applied");
        true
    } else {
        println!("Insufficient funds for fee");
        false
    }
}

fn apply_interest(account: &mut Account, percent: f64) -> bool {
    if !can_do_money_operation(account) {
        return false;
    }

    let interest = account.balance * percent / 100.0;
    account.balance += interest;
    log_transaction(account, "interest", interest, "Interest added");
    true
}

fn request_loan(account: &mut Account, amount: f64) -> bool {
    if !can_do_money_operation(account) {
        return false;
    }

    account.balance += amount;
    account.loan_balance += amount;
    log_transaction(account, "loan_request", amount, "Loan received");
    true
}

fn pay_loan(account: &mut Account, amount: f64) -> bool {
    if !can_do_money_operation(account) {
        return false;
    }

    if account.loan_balance <= 0.0 {
        println!("This account has no loan");
        return false;
    }

    let payment = amount.min(account.loan_balance);

    if account.balance < payment {
        println!("Insufficient funds for loan payment");
        return false;
    }

    account.balance -= payment;
    account.loan_balance -= payment;
    log_transaction(account, "pay_loan", payment, "Loan payment made");
    true
}

fn get_balance(account: &Account) -> f64 {
    account.balance
}

fn get_account_info(account: &Account) -> String {
    format!(
        "ID: {}, Name: {}, Email: {}, Balance: {}, Loan: {}, Status: {}",
        account.id,
        account.name,
        account.email,
        account.balance,
        account.loan_balance,
        account_status(account)
    )
}

fn list_accounts(accounts: &[Account]) {
    if accounts.is_empty() {
        println!("No accounts found");
        return;
    }

    for account in accounts {
        println!("{}", get_account_info(account));
    }
}

fn search_account(accounts: &[Account], id: u32) -> Option<&Account> {
    accounts.iter().find(|account| account.id == id)
}

fn search_account_mut(accounts: &mut [Account], id: u32) -> Option<&mut Account> {
    accounts.iter_mut().find(|account| account.id == id)
}

fn command_line_interface(accounts: &mut Vec<Account>) {
    loop {
        println!();
        println!("Commands:");
        println!("create, update_name, update_email, activate, deactivate, close, delete");
        println!("deposit, withdraw, transfer, fee, monthly_fee, interest");
        println!("loan_request, pay_loan");
        println!("balance, info, list, search, history, statement");
        println!("total_balance, richest, empty_accounts, count, save, exit");

        let command = read_text("Enter command: ").to_lowercase();

        match command.as_str() {
            "create" => {
                let id = read_u32("Account id: ");

                if search_account(accounts, id).is_some() {
                    println!("Account already exists");
                    continue;
                }

                let name = read_text("Name: ");
                let email = read_text("Email: ");
                let balance = read_f64("Starting balance: ");

                if name.is_empty() || email.is_empty() {
                    println!("Name and email cannot be empty");
                    continue;
                }

                if balance < 0.0 {
                    println!("Balance cannot be negative");
                    continue;
                }

                let account = create_account(id, name, email, balance);
                accounts.push(account);
                println!("Account created");
            }
            "update_name" => {
                let id = read_u32("Account id: ");
                let name = read_text("New name: ");

                if name.is_empty() {
                    println!("Name cannot be empty");
                    continue;
                }

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        update_name(account, name);
                        println!("Name updated");
                    }
                    None => println!("Account not found"),
                }
            }
            "update_email" => {
                let id = read_u32("Account id: ");
                let email = read_text("New email: ");

                if email.is_empty() {
                    println!("Email cannot be empty");
                    continue;
                }

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        update_email(account, email);
                        println!("Email updated");
                    }
                    None => println!("Account not found"),
                }
            }
            "activate" => {
                let id = read_u32("Account id: ");

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        if activate_account(account) {
                            println!("Account activated");
                        }
                    }
                    None => println!("Account not found"),
                }
            }
            "deactivate" => {
                let id = read_u32("Account id: ");

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        deactivate_account(account);
                        println!("Account deactivated");
                    }
                    None => println!("Account not found"),
                }
            }
            "close" => {
                let id = read_u32("Account id: ");

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        if close_account(account) {
                            println!("Account closed");
                        }
                    }
                    None => println!("Account not found"),
                }
            }
            "delete" => {
                let id = read_u32("Account id: ");

                if delete_account(accounts, id) {
                    println!("Account deleted");
                } else {
                    println!("Account not found");
                }
            }
            "deposit" => {
                let id = read_u32("Account id: ");
                let amount = read_positive_f64("Amount: ");

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        if deposit(account, amount) {
                            println!("Deposit completed");
                        }
                    }
                    None => println!("Account not found"),
                }
            }
            "withdraw" => {
                let id = read_u32("Account id: ");
                let amount = read_positive_f64("Amount: ");

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        if withdraw(account, amount) {
                            println!("Withdraw completed");
                        }
                    }
                    None => println!("Account not found"),
                }
            }
            "transfer" => {
                let from_id = read_u32("From account id: ");
                let to_id = read_u32("To account id: ");
                let amount = read_positive_f64("Amount: ");

                transfer_by_id(accounts, from_id, to_id, amount);
            }
            "fee" => {
                let id = read_u32("Account id: ");
                let amount = read_positive_f64("Fee amount: ");

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        if apply_fee(account, amount) {
                            println!("Fee applied");
                        }
                    }
                    None => println!("Account not found"),
                }
            }
            "monthly_fee" => {
                let amount = read_positive_f64("Monthly fee amount: ");
                apply_monthly_fee(accounts, amount);
            }
            "interest" => {
                let id = read_u32("Account id: ");
                let percent = read_positive_f64("Interest percent: ");

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        if apply_interest(account, percent) {
                            println!("Interest added");
                        }
                    }
                    None => println!("Account not found"),
                }
            }
            "loan_request" => {
                let id = read_u32("Account id: ");
                let amount = read_positive_f64("Loan amount: ");

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        if request_loan(account, amount) {
                            println!("Loan received");
                        }
                    }
                    None => println!("Account not found"),
                }
            }
            "pay_loan" => {
                let id = read_u32("Account id: ");
                let amount = read_positive_f64("Payment amount: ");

                match search_account_mut(accounts, id) {
                    Some(account) => {
                        if pay_loan(account, amount) {
                            println!("Loan payment completed");
                        }
                    }
                    None => println!("Account not found"),
                }
            }
            "balance" => {
                let id = read_u32("Account id: ");

                match search_account(accounts, id) {
                    Some(account) => println!("Balance: {}", get_balance(account)),
                    None => println!("Account not found"),
                }
            }
            "info" | "search" => {
                let id = read_u32("Account id: ");

                match search_account(accounts, id) {
                    Some(account) => println!("{}", get_account_info(account)),
                    None => println!("Account not found"),
                }
            }
            "list" => {
                list_accounts(accounts);
            }
            "history" | "statement" => {
                let id = read_u32("Account id: ");

                match search_account(accounts, id) {
                    Some(account) => print_account_statement(account),
                    None => println!("Account not found"),
                }
            }
            "total_balance" => {
                println!("Total balance: {}", total_balance(accounts));
            }
            "richest" => match richest_account(accounts) {
                Some(account) => println!("Richest account: {}", get_account_info(account)),
                None => println!("No accounts found"),
            },
            "empty_accounts" => {
                print_empty_accounts(accounts);
            }
            "count" => {
                println!("Account count: {}", accounts.len());
            }
            "save" => {
                let path = read_text("File path: ");

                match save_accounts(accounts, &path) {
                    Ok(()) => println!("Accounts saved"),
                    Err(error) => println!("Save failed: {}", error),
                }
            }
            "exit" => {
                println!("Goodbye");
                break;
            }
            "" => {}
            _ => {
                println!("Unknown command");
            }
        }
    }
}

fn transfer_by_id(accounts: &mut Vec<Account>, from_id: u32, to_id: u32, amount: f64) {
    if from_id == to_id {
        println!("Cannot transfer to the same account");
        return;
    }

    let from_index = accounts.iter().position(|account| account.id == from_id);
    let to_index = accounts.iter().position(|account| account.id == to_id);

    match (from_index, to_index) {
        (Some(from_index), Some(to_index)) => {
            if from_index < to_index {
                let (left, right) = accounts.split_at_mut(to_index);

                if transfer(&mut left[from_index], &mut right[0], amount) {
                    println!("Transfer completed");
                }
            } else {
                let (left, right) = accounts.split_at_mut(from_index);

                if transfer(&mut right[0], &mut left[to_index], amount) {
                    println!("Transfer completed");
                }
            }
        }
        (None, _) => println!("From account not found"),
        (_, None) => println!("To account not found"),
    }
}

fn apply_monthly_fee(accounts: &mut [Account], amount: f64) {
    for account in accounts {
        if apply_fee(account, amount) {
            println!("Fee applied to account {}", account.id);
        }
    }
}

fn total_balance(accounts: &[Account]) -> f64 {
    accounts.iter().map(|account| account.balance).sum()
}

fn richest_account(accounts: &[Account]) -> Option<&Account> {
    accounts
        .iter()
        .max_by(|a, b| a.balance.partial_cmp(&b.balance).unwrap())
}

fn print_empty_accounts(accounts: &[Account]) {
    let mut found = false;

    for account in accounts {
        if account.balance == 0.0 {
            println!("{}", get_account_info(account));
            found = true;
        }
    }

    if !found {
        println!("No empty accounts found");
    }
}

fn print_account_statement(account: &Account) {
    println!("{}", get_account_info(account));
    println!("Transactions:");

    if account.transactions.is_empty() {
        println!("No transactions found");
        return;
    }

    for transaction in &account.transactions {
        println!(
            "- {} | Amount: {} | {}",
            transaction.transaction_type, transaction.amount, transaction.description
        );
    }
}

fn save_accounts(accounts: &[Account], path: &str) -> io::Result<()> {
    let mut output = String::new();

    for account in accounts {
        output.push_str(&format!("{}\n", get_account_info(account)));

        for transaction in &account.transactions {
            output.push_str(&format!(
                "  - {} | {} | {}\n",
                transaction.transaction_type, transaction.amount, transaction.description
            ));
        }
    }

    fs::write(path, output)
}

fn can_do_money_operation(account: &Account) -> bool {
    if account.is_closed {
        println!("Account is closed");
        return false;
    }

    if !account.is_active {
        println!("Account is inactive");
        return false;
    }

    true
}

fn account_status(account: &Account) -> &'static str {
    if account.is_closed {
        "closed"
    } else if account.is_active {
        "active"
    } else {
        "inactive"
    }
}

fn log_transaction(account: &mut Account, transaction_type: &str, amount: f64, description: &str) {
    account.transactions.push(Transaction {
        transaction_type: transaction_type.to_string(),
        amount,
        description: description.to_string(),
    });
}

fn read_text(message: &str) -> String {
    print!("{}", message);
    io::stdout().flush().expect("Failed to flush stdout");

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("Failed to read line");

    input.trim().to_string()
}

fn read_u32(message: &str) -> u32 {
    loop {
        let input = read_text(message);

        match input.parse::<u32>() {
            Ok(value) => return value,
            Err(_) => println!("Please enter a valid number"),
        }
    }
}

fn read_f64(message: &str) -> f64 {
    loop {
        let input = read_text(message);

        match input.parse::<f64>() {
            Ok(value) => return value,
            Err(_) => println!("Please enter a valid number"),
        }
    }
}

fn read_positive_f64(message: &str) -> f64 {
    loop {
        let value = read_f64(message);

        if value > 0.0 {
            return value;
        }

        println!("Amount must be positive");
    }
}
