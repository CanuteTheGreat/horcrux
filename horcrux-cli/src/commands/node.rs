//! `horcrux node ...`: one cluster member (`/api/nodes/<node>/...`). Any
//! member answers for any other: the request is forwarded over the
//! cluster's peer channel.
use crate::api::ApiClient;
use crate::output::{self, OutputFormat};
use crate::NodeCommands;
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// The JSON body of `POST /api/nodes/<node>/maintenance`.
pub(crate) fn maintenance_body(
    action: &str,
    return_policy: Option<&str>,
    force: bool,
    no_drain: bool,
) -> Result<Value> {
    let enabled = match action {
        "on" => true,
        "off" => false,
        other => bail!("{other}: expected on, off, status or plan"),
    };
    if let Some(r) = return_policy {
        if !["origin", "rebalance", "none"].contains(&r) {
            bail!("--return {r}: expected origin, rebalance or none");
        }
    }
    let mut b = json!({ "enabled": enabled, "drain": !no_drain, "force": force });
    if let Some(r) = return_policy {
        b["return"] = json!(r);
    }
    Ok(b)
}

fn path_segment(node: &str) -> Result<&str> {
    if node.is_empty() || node.contains(['/', '?', '#', ' ']) {
        bail!("{node:?} is not a node name");
    }
    Ok(node)
}

fn print_plan(plan: &Value) {
    let moves = plan["moves"].as_array().cloned().unwrap_or_default();
    let unplaced = plan["unplaced"].as_array().cloned().unwrap_or_default();
    if moves.is_empty() && unplaced.is_empty() {
        output::print_info("nothing to move");
    }
    for m in &moves {
        println!(
            "  move  {} ({})  {} -> {}  [{}]",
            m["name"].as_str().unwrap_or(""),
            m["guest_id"].as_str().unwrap_or(""),
            m["from"].as_str().unwrap_or(""),
            m["to"].as_str().unwrap_or(""),
            m["reason"].as_str().unwrap_or("")
        );
    }
    for u in &unplaced {
        println!(
            "  stays {} ({}): {}",
            u["name"].as_str().unwrap_or(""),
            u["guest_id"].as_str().unwrap_or(""),
            u["reason"].as_str().unwrap_or("")
        );
    }
}

fn print_state(v: &Value) {
    let node = v["node"].as_str().unwrap_or("?");
    if v["enabled"].as_bool().unwrap_or(false) {
        output::print_info(&format!(
            "{node}: in maintenance since {} (return policy {})",
            v["since"].as_str().unwrap_or("?"),
            v["effective_return_policy"].as_str().unwrap_or("origin")
        ));
    } else {
        output::print_info(&format!("{node}: active (takes guests)"));
    }
    for (what, run) in [("drain", &v["drain"]), ("exit", &v["exit"])] {
        if run.is_object() {
            let results = run["results"].as_array().cloned().unwrap_or_default();
            let done = results.iter().filter(|r| r["status"] == "done").count();
            println!(
                "  last {what}: {} ({done}/{} moved)",
                run["status"].as_str().unwrap_or("?"),
                results.len()
            );
            for r in results.iter().filter(|r| r["status"] == "failed") {
                println!(
                    "    failed {}: {}",
                    r["guest_id"].as_str().unwrap_or(""),
                    r["error"].as_str().unwrap_or("")
                );
            }
        }
    }
}

pub async fn handle_node_command(
    command: NodeCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    let fmt = OutputFormat::from_str(output_format);
    match command {
        NodeCommands::Maintenance {
            node,
            action,
            return_policy,
            force,
            no_drain,
        } => {
            let n = path_segment(&node)?;
            match action.as_str() {
                "status" => {
                    let v: Value = api.get(&format!("/api/nodes/{n}/maintenance")).await?;
                    match fmt {
                        OutputFormat::Table => print_state(&v),
                        _ => output::print_single(&v, fmt)?,
                    }
                }
                "plan" => {
                    let q = match return_policy.as_deref() {
                        Some(r) => format!("?action=exit&return={r}"),
                        None => String::new(),
                    };
                    let v: Value = api
                        .get(&format!("/api/nodes/{n}/maintenance/plan{q}"))
                        .await?;
                    match fmt {
                        OutputFormat::Table => print_plan(&v),
                        _ => output::print_single(&v, fmt)?,
                    }
                }
                a => {
                    let body = maintenance_body(a, return_policy.as_deref(), force, no_drain)?;
                    let v: Value = api
                        .post(&format!("/api/nodes/{n}/maintenance"), &body)
                        .await?;
                    match fmt {
                        OutputFormat::Table => {
                            output::print_success(&if a == "on" {
                                format!("{node} is entering maintenance")
                            } else {
                                format!("{node} left maintenance")
                            });
                            print_state(&v);
                        }
                        _ => output::print_single(&v, fmt)?,
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_body_maps_the_flags() {
        let b = maintenance_body("on", None, false, false).unwrap();
        assert_eq!(b, json!({"enabled": true, "drain": true, "force": false}));
        let b = maintenance_body("off", Some("none"), false, false).unwrap();
        assert_eq!(b["return"], "none");
        assert_eq!(b["enabled"], false);
        let b = maintenance_body("on", Some("rebalance"), true, true).unwrap();
        assert_eq!((b["force"].clone(), b["drain"].clone()), (json!(true), json!(false)));
        assert!(maintenance_body("maybe", None, false, false).is_err());
        assert!(maintenance_body("off", Some("back"), false, false).is_err());
        assert!(path_segment("a/b").is_err());
        assert!(path_segment("node-b").is_ok());
    }
}
