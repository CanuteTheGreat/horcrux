use crate::api::ApiClient;
use crate::output;
use crate::UserCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct User {
    id: String,
    username: String,
    email: Option<String>,
    role: String,
    enabled: bool,
}

#[derive(Serialize)]
struct CreateUserRequest {
    username: String,
    password: String,
    role: String,
}

#[derive(Serialize)]
struct ChangePasswordRequest {
    password: String,
}

#[derive(Serialize)]
struct GrantPermissionRequest {
    permission: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Role {
    name: String,
    description: String,
    permissions: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct Permission {
    permission: String,
    granted_at: String,
}

pub async fn handle_user_command(
    command: UserCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        UserCommands::List => {
            let users: Vec<User> = api.get("/api/users").await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&users)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&users)?);
            } else {
                println!("{:<36} {:<20} {:<15} {:<30} {}",
                    "ID", "USERNAME", "ROLE", "EMAIL", "STATUS");
                println!("{}", "-".repeat(120));
                for user in users {
                    let status = if user.enabled { "Enabled" } else { "Disabled" };
                    let email = user.email.unwrap_or_else(|| "-".to_string());
                    println!("{:<36} {:<20} {:<15} {:<30} {}",
                        user.id, user.username, user.role, email, status);
                }
            }
        }
        UserCommands::Create { username, password, role } => {
            let request = CreateUserRequest {
                username: username.clone(),
                password,
                role: role.clone(),
            };

            let user: User = api.post("/api/users", &request).await?;
            output::print_success(&format!("User '{}' created successfully (ID: {})", username, user.id));
        }
        UserCommands::Delete { username } => {
            // Find user by username first
            let users: Vec<User> = api.get("/api/users").await?;
            let user = users.iter()
                .find(|u| u.username == username)
                .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))?;

            api.delete(&format!("/api/users/{}", user.id)).await?;
            output::print_success(&format!("User '{}' deleted successfully", username));
        }
        UserCommands::Passwd { username } => {
            // Prompt for new password
            use dialoguer::Password;
            let password = Password::new()
                .with_prompt("New password")
                .with_confirmation("Confirm password", "Passwords do not match")
                .interact()?;

            // Find user by username
            let users: Vec<User> = api.get("/api/users").await?;
            let user = users.iter()
                .find(|u| u.username == username)
                .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))?;

            let request = ChangePasswordRequest { password };
            api.post_empty(&format!("/api/users/{}/password", user.id), &request).await?;
            output::print_success(&format!("Password changed for user '{}'", username));
        }
        UserCommands::Roles => {
            let roles: Vec<Role> = api.get("/api/roles").await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&roles)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&roles)?);
            } else {
                println!("{:<20} {:<40} {}",
                    "ROLE", "DESCRIPTION", "PERMISSIONS");
                println!("{}", "-".repeat(100));
                for role in roles {
                    let perms = role.permissions.join(", ");
                    println!("{:<20} {:<40} {}",
                        role.name, role.description, perms);
                }
            }
        }
        UserCommands::Grant { username, permission } => {
            // Find user by username
            let users: Vec<User> = api.get("/api/users").await?;
            let user = users.iter()
                .find(|u| u.username == username)
                .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))?;

            let request = GrantPermissionRequest { permission: permission.clone() };
            api.post_empty(&format!("/api/permissions/{}", user.id), &request).await?;
            output::print_success(&format!("Permission '{}' granted to user '{}'", permission, username));
        }
    }
    Ok(())
}
