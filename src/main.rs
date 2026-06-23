use std::{env, net::SocketAddr};

use bank::AppState;

#[tokio::main]
async fn main() {
    let command = env::args().nth(1);

    if command.as_deref() == Some("serve") {
        let address = env::args()
            .nth(2)
            .unwrap_or_else(|| "127.0.0.1:3000".to_string())
            .parse::<SocketAddr>()
            .expect("server address must look like 127.0.0.1:3000");

        println!("Serving API on http://{address}");

        if let Err(error) = bank::api::serve(AppState::new(), address).await {
            eprintln!("API server failed: {error}");
        }

        return;
    }

    bank::cli::run();
}
