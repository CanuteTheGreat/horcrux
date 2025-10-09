///! Authentication commands

use crate::api::ApiClient;
use crate::config::Config;
use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

#[derive(Subcommand)]
pub enum AuthCommands {
    /// Login to the Horcrux server
    Login {
        /// Username
        #[arg(short, long)]
        username: String,

        /// Password (will be prompted if not provided)
        #[arg(short, long)]
        password: Option<String>,
    },

    /// Register a new user account
    Register {
        /// Username
        #[arg(short, long)]
        username: String,

        /// Email address
        #[arg(short, long)]
        email: String,

        /// Password (will be prompted if not provided)
        #[arg(short, long)]
        password: Option<String>,
    },

    /// Logout (clear stored credentials)
    Logout,

    /// Show current authentication status
    Status,

    /// Change password
    ChangePassword {
        /// Current password (will be prompted if not provided)
        #[arg(long)]
        old_password: Option<String>,

        /// New password (will be prompted if not provided)
        #[arg(long)]
        new_password: Option<String>,
    },
}

#[derive(Serialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct LoginResponse {
    ticket: String,
    #[allow(dead_code)]
    csrf_token: String,
    username: String,
    roles: Vec<String>,
}

#[derive(Serialize)]
struct RegisterRequest {
    username: String,
    password: String,
    email: String,
}

#[derive(Serialize)]
struct ChangePasswordRequest {
    username: String,
    old_password: String,
    new_password: String,
}

pub async fn handle_auth_command(
    command: AuthCommands,
    api: &ApiClient,
    config: &mut Config,
) -> Result<()> {
    match command {
        AuthCommands::Login { username, password } => {
            let password = if let Some(pwd) = password {
                pwd
            } else {
                // Prompt for password
                use dialoguer::Password;
                Password::new()
                    .with_prompt("Password")
                    .interact()?
            };

            let request = LoginRequest {
                username: username.clone(),
                password,
            };

            let response: LoginResponse = api.post("/api/auth/login", &request).await?;

            // Save token to API client
            api.set_token(response.ticket.clone()).await;

            // Save token to config
            config.token = Some(response.ticket);
            config.username = Some(response.username.clone());
            config.save()?;

            println!("✓ Login successful");
            println!("  Username: {}", response.username);
            println!("  Roles: {}", response.roles.join(", "));
        }

        AuthCommands::Register {
            username,
            email,
            password,
        } => {
            let password = if let Some(pwd) = password {
                pwd
            } else {
                // Prompt for password with confirmation
                use dialoguer::Password;
                Password::new()
                    .with_prompt("Password")
                    .with_confirmation("Confirm password", "Passwords do not match")
                    .interact()?
            };

            let request = RegisterRequest {
                username,
                password,
                email,
            };

            api.post_empty("/api/auth/register", &request).await?;

            println!("✓ Registration successful");
            println!("  You can now login with your credentials");
        }

        AuthCommands::Logout => {
            // Clear token from API client
            api.clear_token().await;

            // Clear token from config
            config.token = None;
            config.username = None;
            config.save()?;

            println!("✓ Logged out successfully");
        }

        AuthCommands::Status => {
            if let Some(token) = &config.token {
                if let Some(username) = &config.username {
                    println!("Authenticated as: {}", username);
                    println!("Token: {}...", &token[..20.min(token.len())]);
                } else {
                    println!("Token present but no username stored");
                }
            } else {
                println!("Not authenticated");
                println!("Use 'horcrux auth login' to authenticate");
            }
        }

        AuthCommands::ChangePassword {
            old_password,
            new_password,
        } => {
            let username = config.username.as_ref()
                .ok_or_else(|| anyhow::anyhow!("Not logged in. Use 'horcrux auth login' first"))?;

            let old_password = if let Some(pwd) = old_password {
                pwd
            } else {
                use dialoguer::Password;
                Password::new()
                    .with_prompt("Current password")
                    .interact()?
            };

            let new_password = if let Some(pwd) = new_password {
                pwd
            } else {
                use dialoguer::Password;
                Password::new()
                    .with_prompt("New password")
                    .with_confirmation("Confirm new password", "Passwords do not match")
                    .interact()?
            };

            let request = ChangePasswordRequest {
                username: username.clone(),
                old_password,
                new_password,
            };

            api.post_empty("/api/auth/password", &request).await?;

            println!("✓ Password changed successfully");
        }
    }

    Ok(())
}
