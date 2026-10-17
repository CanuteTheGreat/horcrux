use crate::api::ApiClient;
use crate::output::{self, OutputFormat};
use crate::UserCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
struct User {
    id: String,
    username: String,
    email: Option<String>,
    role: String,
    enabled: bool,
    /// roles granted on /
    #[serde(default)]
    roles: Vec<String>,
}

#[derive(Tabled, Serialize)]
struct UserRow {
    id: String,
    username: String,
    role: String,
    email: String,
    status: String,
}

impl From<User> for UserRow {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            role: if u.roles.is_empty() {
                "-".to_string()
            } else {
                u.roles.join(",")
            },
            email: u
                .email
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "-".to_string()),
            status: if u.enabled { "Enabled" } else { "Disabled" }.to_string(),
        }
    }
}

#[derive(Tabled, Serialize)]
struct RoleRow {
    name: String,
    description: String,
    permissions: String,
}

impl From<Role> for RoleRow {
    fn from(r: Role) -> Self {
        Self {
            name: r.name,
            description: r.description,
            permissions: r.privileges.join(", "),
        }
    }
}

#[derive(Serialize)]
struct CreateUserRequest {
    username: String,
    password: String,
    role: String,
}

#[derive(Serialize)]
struct ChangePasswordRequest {
    new_password: String,
}

#[derive(Serialize)]
struct GrantRequest {
    path: String,
    subject_type: String,
    subject: String,
    role: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Role {
    name: String,
    description: String,
    privileges: Vec<String>,
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
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<UserRow> = users.into_iter().map(UserRow::from).collect();
            output::print_output(rows, format)?;
        }
        UserCommands::Create {
            username,
            password,
            role,
        } => {
            let request = CreateUserRequest {
                username: username.clone(),
                password,
                role: role.clone(),
            };

            let user: User = api.post("/api/users", &request).await?;
            output::print_created("User", &username, &user.id);
        }
        UserCommands::Delete { username } => {
            // Find user by username first
            let users: Vec<User> = api.get("/api/users").await?;
            let user = users
                .iter()
                .find(|u| u.username == username)
                .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))?;

            api.delete(&format!("/api/users/{}", user.id)).await?;
            output::print_deleted("User", &username);
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
            let user = users
                .iter()
                .find(|u| u.username == username)
                .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))?;

            let request = ChangePasswordRequest {
                new_password: password,
            };
            api.post_empty(&format!("/api/users/{}/password", user.id), &request)
                .await?;
            output::print_success(&format!("Password changed for user '{}'", username));
        }
        UserCommands::Roles => {
            let roles: Vec<Role> = api.get("/api/roles").await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<RoleRow> = roles.into_iter().map(RoleRow::from).collect();
            output::print_output(rows, format)?;
        }
        UserCommands::Grant {
            username,
            permission,
        } => {
            // Find user by username
            let users: Vec<User> = api.get("/api/users").await?;
            let user = users
                .iter()
                .find(|u| u.username == username)
                .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))?;

            let (role, path) = match permission.split_once('@') {
                Some((r, p)) => (r.to_string(), p.to_string()),
                None => (permission.clone(), "/".to_string()),
            };
            let request = GrantRequest {
                path,
                subject_type: "user".into(),
                subject: user.id.clone(),
                role,
            };
            api.post_empty("/api/acl", &request).await?;
            output::print_success(&format!(
                "Permission '{}' granted to '{}'",
                permission, username
            ));
        }
    }
    Ok(())
}
