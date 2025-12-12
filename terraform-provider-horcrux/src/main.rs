//! Terraform Provider for Horcrux
//!
//! This provider implements the Terraform Plugin Protocol v6 for managing
//! Horcrux resources including VMs, containers, storage, and networks.

mod client;
mod provider;
mod resources;
mod schema;

use clap::Parser;
use provider::HorcruxProvider;
use std::io::{self, BufRead, Write};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Terraform Provider for Horcrux
#[derive(Parser, Debug)]
#[command(name = "terraform-provider-horcrux")]
#[command(about = "Terraform provider for Horcrux virtualization platform")]
struct Args {
    /// Enable debug mode
    #[arg(long, env = "TF_LOG")]
    debug: bool,
}

fn main() {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(io::stderr))
        .init();

    let _args = Args::parse();

    tracing::info!("Starting Terraform Provider for Horcrux");

    // Terraform plugin protocol uses stdin/stdout for communication
    // This implements the JSON-RPC based plugin protocol
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    let provider = HorcruxProvider::new();

    for line in stdin.lock().lines() {
        match line {
            Ok(input) => {
                let response = provider.handle_request(&input);
                if let Err(e) = writeln!(stdout_lock, "{}", response) {
                    tracing::error!("Failed to write response: {}", e);
                    break;
                }
                if let Err(e) = stdout_lock.flush() {
                    tracing::error!("Failed to flush stdout: {}", e);
                    break;
                }
            }
            Err(e) => {
                tracing::error!("Failed to read input: {}", e);
                break;
            }
        }
    }

    tracing::info!("Terraform Provider shutting down");
}
