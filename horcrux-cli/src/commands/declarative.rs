//! `horcrux plan|apply|export`: declarative cluster configuration. The
//! spec file is sent as text; the server parses, plans and applies it.
use crate::api::ApiClient;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

fn read_spec(file: &str) -> Result<String> {
    if file == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        Ok(s)
    } else {
        std::fs::read_to_string(file).with_context(|| format!("reading {file}"))
    }
}

fn print_plan(plan: &Value) {
    let changes = plan["changes"].as_array().cloned().unwrap_or_default();
    for e in plan["errors"].as_array().into_iter().flatten() {
        println!("error: {}", e.as_str().unwrap_or(""));
    }
    for c in &changes {
        let sym = match c["action"].as_str().unwrap_or("") {
            "create" | "start" => "+",
            "delete" => "-",
            "update" | "stop" => "~",
            _ => "!",
        };
        println!(
            "{sym} {:<8} {:<10} {}  {}",
            c["kind"].as_str().unwrap_or(""),
            c["action"].as_str().unwrap_or(""),
            c["name"].as_str().unwrap_or(""),
            c["details"]
                .as_array()
                .map(|d| d
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join("; "))
                .unwrap_or_default()
        );
    }
    if changes.is_empty() && plan["errors"].as_array().is_none_or(|e| e.is_empty()) {
        println!("in sync: nothing to do");
    }
}

/// `horcrux plan -f spec.yaml [--prune]`; exits 2 when changes are pending.
pub async fn plan(api: &ApiClient, file: &str, prune: bool) -> Result<()> {
    let spec = read_spec(file)?;
    let body = if prune {
        json!({"spec": spec, "prune": true})
    } else {
        json!({"spec": spec})
    };
    let r: Value = api.post("/api/declarative/plan", &body).await?;
    print_plan(&r["plan"]);
    if r["in_sync"] != json!(true) {
        std::process::exit(2);
    }
    Ok(())
}

pub async fn apply(api: &ApiClient, file: &str, prune: bool, dry_run: bool) -> Result<()> {
    let spec = read_spec(file)?;
    let mut body = json!({"spec": spec, "dry_run": dry_run});
    if prune {
        body["prune"] = json!(true);
    }
    let r: Value = api.post("/api/declarative/apply", &body).await?;
    if dry_run {
        print_plan(&r["plan"]);
    }
    for x in r["results"].as_array().into_iter().flatten() {
        println!(
            "{:<8} {:<8} {:<10} {} {}",
            x["status"].as_str().unwrap_or(""),
            x["kind"].as_str().unwrap_or(""),
            x["action"].as_str().unwrap_or(""),
            x["name"].as_str().unwrap_or(""),
            x["message"].as_str().unwrap_or("")
        );
    }
    if r["ok"] != json!(true) {
        for e in r["plan"]["errors"].as_array().into_iter().flatten() {
            eprintln!("error: {}", e.as_str().unwrap_or(""));
        }
        bail!("not everything was applied");
    }
    if !dry_run && r["results"].as_array().is_none_or(|a| a.is_empty()) {
        println!("in sync: nothing to do");
    }
    Ok(())
}

/// `horcrux export [-f out.yaml]`
pub async fn export(api: &ApiClient, file: Option<&str>) -> Result<()> {
    let r: Value = api.get("/api/declarative/export").await?;
    let yaml = r["spec"].as_str().context("no spec in the reply")?;
    match file {
        Some(f) => std::fs::write(f, yaml).with_context(|| format!("writing {f}"))?,
        None => print!("{yaml}"),
    }
    Ok(())
}
