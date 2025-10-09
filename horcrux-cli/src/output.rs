///! Output formatting for CLI

use colored::Colorize;
use serde::Serialize;
use tabled::{Table, Tabled};

pub fn print_table<T: Tabled>(data: Vec<T>) {
    if data.is_empty() {
        println!("{}", "No results found".yellow());
        return;
    }

    let table = Table::new(data);
    println!("{}", table);
}

pub fn print_json<T: Serialize>(data: &T) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(data)?;
    println!("{}", json);
    Ok(())
}

pub fn print_success(message: &str) {
    println!("{} {}", "✓".green().bold(), message.green());
}

#[allow(dead_code)]
pub fn print_error(message: &str) {
    eprintln!("{} {}", "✗".red().bold(), message.red());
}

#[allow(dead_code)]
pub fn print_info(message: &str) {
    println!("{} {}", "ℹ".blue().bold(), message);
}

#[allow(dead_code)]
pub fn print_warning(message: &str) {
    println!("{} {}", "⚠".yellow().bold(), message.yellow());
}
