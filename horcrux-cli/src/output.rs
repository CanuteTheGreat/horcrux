///! Output formatting for CLI
///!
///! This module provides unified output formatting across all CLI commands
///! to ensure consistent user experience.

use colored::Colorize;
use serde::Serialize;
use tabled::{Table, Tabled};

/// Output format options
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Table,
    Json,
    Yaml,
}

impl OutputFormat {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "json" => OutputFormat::Json,
            "yaml" | "yml" => OutputFormat::Yaml,
            _ => OutputFormat::Table,
        }
    }
}

/// Print data in the specified format (table, JSON, or YAML)
pub fn print_output<T: Tabled + Serialize>(data: Vec<T>, format: OutputFormat) -> anyhow::Result<()> {
    match format {
        OutputFormat::Table => print_table(data),
        OutputFormat::Json => print_json(&data)?,
        OutputFormat::Yaml => print_yaml(&data)?,
    }
    Ok(())
}

/// Print a single item in the specified format
pub fn print_single<T: Serialize>(data: &T, format: OutputFormat) -> anyhow::Result<()> {
    match format {
        OutputFormat::Table => {
            // For single items in table format, use JSON pretty print
            print_json(data)?;
        }
        OutputFormat::Json => print_json(data)?,
        OutputFormat::Yaml => print_yaml(data)?,
    }
    Ok(())
}

/// Print data as a table using the tabled crate
pub fn print_table<T: Tabled>(data: Vec<T>) {
    if data.is_empty() {
        println!("{}", "No results found".yellow());
        return;
    }

    let table = Table::new(data);
    println!("{}", table);
}

/// Print data as pretty-printed JSON
pub fn print_json<T: Serialize>(data: &T) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(data)?;
    println!("{}", json);
    Ok(())
}

/// Print data as YAML
pub fn print_yaml<T: Serialize>(data: &T) -> anyhow::Result<()> {
    let yaml = serde_yaml::to_string(data)?;
    println!("{}", yaml);
    Ok(())
}

/// Print a success message with green checkmark
pub fn print_success(message: &str) {
    println!("{} {}", "✓".green().bold(), message.green());
}

/// Print a success message for resource creation
pub fn print_created(resource_type: &str, name: &str, id: &str) {
    println!(
        "{} {} '{}' created (ID: {})",
        "✓".green().bold(),
        resource_type.green(),
        name.green().bold(),
        id.dimmed()
    );
}

/// Print a success message for resource deletion
pub fn print_deleted(resource_type: &str, id: &str) {
    println!(
        "{} {} '{}' deleted",
        "✓".green().bold(),
        resource_type.green(),
        id.green().bold()
    );
}

/// Print a success message for a started resource
pub fn print_started(resource_type: &str, id: &str) {
    println!(
        "{} {} '{}' started",
        "✓".green().bold(),
        resource_type.green(),
        id.green().bold()
    );
}

/// Print a success message for a stopped resource
pub fn print_stopped(resource_type: &str, id: &str) {
    println!(
        "{} {} '{}' stopped",
        "✓".green().bold(),
        resource_type.green(),
        id.green().bold()
    );
}

/// Print an error message with red X
pub fn print_error(message: &str) {
    eprintln!("{} {}", "✗".red().bold(), message.red());
}

/// Print an info message with blue i
pub fn print_info(message: &str) {
    println!("{} {}", "ℹ".blue().bold(), message);
}

/// Print a warning message with yellow triangle
pub fn print_warning(message: &str) {
    println!("{} {}", "⚠".yellow().bold(), message.yellow());
}

/// Format bytes into human-readable size
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format a timestamp as relative time (e.g., "5m ago", "2h ago")
pub fn format_relative_time(timestamp: i64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let diff = now - timestamp;

    if diff < 0 {
        let abs_diff = -diff;
        if abs_diff < 60 {
            format!("in {}s", abs_diff)
        } else if abs_diff < 3600 {
            format!("in {}m", abs_diff / 60)
        } else if abs_diff < 86400 {
            format!("in {}h", abs_diff / 3600)
        } else {
            format!("in {}d", abs_diff / 86400)
        }
    } else if diff < 60 {
        format!("{}s ago", diff)
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

/// Format duration in seconds to human-readable string
pub fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s > 0 {
            format!("{}m {}s", m, s)
        } else {
            format!("{}m", m)
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 {
            format!("{}h {}m", h, m)
        } else {
            format!("{}h", h)
        }
    }
}

/// Truncate a string to max length with ellipsis
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        s[..max_len].to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
