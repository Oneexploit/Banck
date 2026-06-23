use std::{
    fs,
    io::{self, Write},
};

use crate::{
    bank::{Bank, BankError, BankResult},
    domain::{Account, AccountId, InterestRate, Money},
};

pub fn run() {
    let mut bank = Bank::new();
    command_line_interface(&mut bank);
}

fn command_line_interface(bank: &mut Bank) {
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
                let id = read_account_id("Account id: ");
                let name = read_text("Name: ");
                let email = read_text("Email: ");
                let opening_balance = read_non_negative_money("Starting balance: ");

                match bank.create_account(id, name, email, opening_balance) {
                    Ok(_) => println!("Account created"),
                    Err(error) => print_error("Create failed", error),
                }
            }
            "update_name" => {
                let id = read_account_id("Account id: ");
                let name = read_text("New name: ");
                print_result(bank.update_name(id, name), "Name updated");
            }
            "update_email" => {
                let id = read_account_id("Account id: ");
                let email = read_text("New email: ");
                print_result(bank.update_email(id, email), "Email updated");
            }
            "activate" => {
                let id = read_account_id("Account id: ");
                print_result(bank.activate_account(id), "Account activated");
            }
            "deactivate" => {
                let id = read_account_id("Account id: ");
                print_result(bank.deactivate_account(id), "Account deactivated");
            }
            "close" => {
                let id = read_account_id("Account id: ");
                print_result(bank.close_account(id), "Account closed");
            }
            "delete" => {
                let id = read_account_id("Account id: ");

                match bank.delete_account(id) {
                    Ok(_) => println!("Account deleted"),
                    Err(error) => print_error("Delete failed", error),
                }
            }
            "deposit" => {
                let id = read_account_id("Account id: ");
                let amount = read_positive_money("Amount: ");
                print_result(bank.deposit(id, amount), "Deposit completed");
            }
            "withdraw" => {
                let id = read_account_id("Account id: ");
                let amount = read_positive_money("Amount: ");
                print_result(bank.withdraw(id, amount), "Withdraw completed");
            }
            "transfer" => {
                let from_id = read_account_id("From account id: ");
                let to_id = read_account_id("To account id: ");
                let amount = read_positive_money("Amount: ");
                print_result(bank.transfer(from_id, to_id, amount), "Transfer completed");
            }
            "fee" => {
                let id = read_account_id("Account id: ");
                let amount = read_positive_money("Fee amount: ");
                print_result(bank.apply_fee(id, amount), "Fee applied");
            }
            "monthly_fee" => {
                let amount = read_positive_money("Monthly fee amount: ");

                for (id, result) in bank.apply_monthly_fee(amount) {
                    match result {
                        Ok(()) => println!("Fee applied to account {id}"),
                        Err(error) => print_error(&format!("Fee failed for account {id}"), error),
                    }
                }
            }
            "interest" => {
                let id = read_account_id("Account id: ");
                let rate = read_positive_interest_rate("Interest percent: ");

                match bank.apply_interest(id, rate) {
                    Ok(interest) => println!("Interest added: {interest}"),
                    Err(error) => print_error("Interest failed", error),
                }
            }
            "loan_request" => {
                let id = read_account_id("Account id: ");
                let amount = read_positive_money("Loan amount: ");
                print_result(bank.request_loan(id, amount), "Loan received");
            }
            "pay_loan" => {
                let id = read_account_id("Account id: ");
                let amount = read_positive_money("Payment amount: ");

                match bank.pay_loan(id, amount) {
                    Ok(payment) => println!("Loan payment completed: {payment}"),
                    Err(error) => print_error("Loan payment failed", error),
                }
            }
            "balance" => {
                let id = read_account_id("Account id: ");

                match bank.account(id) {
                    Ok(account) => println!("Balance: {}", account.balance()),
                    Err(error) => print_error("Balance lookup failed", error),
                }
            }
            "info" | "search" => {
                let id = read_account_id("Account id: ");

                match bank.account(id) {
                    Ok(account) => println!("{}", account_info(account)),
                    Err(error) => print_error("Account lookup failed", error),
                }
            }
            "list" => list_accounts(bank),
            "history" | "statement" => {
                let id = read_account_id("Account id: ");

                match bank.account(id) {
                    Ok(account) => print_account_statement(account),
                    Err(error) => print_error("Statement failed", error),
                }
            }
            "total_balance" => match bank.total_balance() {
                Ok(total) => println!("Total balance: {total}"),
                Err(error) => print_error("Total balance failed", error),
            },
            "richest" => match bank.richest_account() {
                Some(account) => println!("Richest account: {}", account_info(account)),
                None => println!("No accounts found"),
            },
            "empty_accounts" => print_empty_accounts(bank),
            "count" => println!("Account count: {}", bank.account_count()),
            "save" => {
                let path = read_text("File path: ");

                match save_accounts(bank, &path) {
                    Ok(()) => println!("Accounts saved"),
                    Err(error) => println!("Save failed: {error}"),
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

fn account_info(account: &Account) -> String {
    format!(
        "ID: {}, Name: {}, Email: {}, Balance: {}, Loan: {}, Status: {}",
        account.id(),
        account.name(),
        account.email(),
        account.balance(),
        account.loan_balance(),
        account.status()
    )
}

fn list_accounts(bank: &Bank) {
    if bank.is_empty() {
        println!("No accounts found");
        return;
    }

    for account in bank.accounts() {
        println!("{}", account_info(account));
    }
}

fn print_empty_accounts(bank: &Bank) {
    let accounts = bank.empty_accounts();

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

fn save_accounts(bank: &Bank, path: &str) -> io::Result<()> {
    let mut output = String::new();

    for account in bank.accounts() {
        output.push_str(&format!("{}\n", account_info(account)));

        for transaction in account.transactions() {
            output.push_str(&format!(
                "  - {} | {} | {}\n",
                transaction.kind(),
                transaction.amount(),
                transaction.description()
            ));
        }
    }

    fs::write(path, output)
}

fn print_result(result: BankResult<()>, success_message: &str) {
    match result {
        Ok(()) => println!("{success_message}"),
        Err(error) => print_error("Operation failed", error),
    }
}

fn print_error(context: &str, error: BankError) {
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
