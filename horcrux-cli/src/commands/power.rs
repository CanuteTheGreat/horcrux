//! Guest power commands: Hyper-V-style automatic stop/start actions and
//! manual save state / resume (`horcrux power ...`).
use crate::api::ApiClient;
use crate::output::{self, OutputFormat};
use crate::PowerCommands;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use tabled::Tabled;

#[derive(Tabled, serde::Serialize)]
struct GuestRow {
    kind: String,
    id: String,
    name: String,
    status: String,
    saved: String,
    stop_action: String,
    start_action: String,
    note: String,
}

fn base(kind: &str) -> Result<&'static str> {
    match kind {
        "vm" => Ok("vms"),
        "container" | "ct" => Ok("containers"),
        _ => bail!("kind must be `vm` or `container`"),
    }
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

pub async fn handle_power_command(
    command: PowerCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        PowerCommands::Status => {
            let o: Value = api.get("/api/nodes/guest-power").await?;
            let c = &o["capabilities"];
            output::print_info(&format!(
                "CRIU: {}",
                if c["criu"]["available"].as_bool().unwrap_or(false) {
                    format!(
                        "available ({})",
                        c["criu"]["version"].as_str().unwrap_or("?")
                    )
                } else {
                    format!(
                        "unavailable - {}",
                        c["criu"]["error"].as_str().unwrap_or("")
                    )
                }
            ));
            output::print_info(&format!(
                "Node default: stop={} start={} delay={}s dir={}",
                o["node"]["stop_action"].as_str().unwrap_or("?"),
                o["node"]["start_action"].as_str().unwrap_or("?"),
                o["node"]["start_delay_secs"],
                o["node"]["save_dir"].as_str().unwrap_or("(default)")
            ));
            let rows: Vec<GuestRow> = o["guests"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|g| GuestRow {
                    kind: g["kind"].as_str().unwrap_or("").into(),
                    id: g["id"].as_str().unwrap_or("").into(),
                    name: g["name"].as_str().unwrap_or("").into(),
                    status: g["status"].as_str().unwrap_or("").into(),
                    saved: match g["saved"].as_object() {
                        Some(s) => format!(
                            "{} {}",
                            s.get("size_bytes")
                                .and_then(|b| b.as_u64())
                                .map(|b| format!("{} MB", b / 1024 / 1024))
                                .unwrap_or_default(),
                            s.get("saved_at").and_then(|v| v.as_str()).unwrap_or("")
                        ),
                        None => "-".into(),
                    },
                    stop_action: g["policy"]["stop_action"].as_str().unwrap_or("").into(),
                    start_action: g["policy"]["start_action"].as_str().unwrap_or("").into(),
                    note: g["save_blocker"].as_str().unwrap_or("").into(),
                })
                .collect();
            output::print_output(rows, OutputFormat::from_str(output_format))?;
        }
        PowerCommands::Save { kind, id } => {
            let s: Value = api
                .post(
                    &format!("/api/{}/{}/save-state", base(&kind)?, enc(&id)),
                    &json!({}),
                )
                .await?;
            output::print_success(&format!(
                "{} {} saved ({}, {} bytes)",
                kind,
                id,
                s["mechanism"].as_str().unwrap_or("?"),
                s["size_bytes"]
            ));
        }
        PowerCommands::Resume { kind, id } => {
            api.post::<Value, _>(
                &format!("/api/{}/{}/resume-state", base(&kind)?, enc(&id)),
                &json!({}),
            )
            .await?;
            output::print_success(&format!("{kind} {id} resumed"));
        }
        PowerCommands::Discard { kind, id } => {
            api.delete(&format!("/api/{}/{}/saved-state", base(&kind)?, enc(&id)))
                .await?;
            output::print_success(&format!("saved state of {kind} {id} discarded"));
        }
        PowerCommands::Policy {
            kind,
            id,
            stop,
            start,
            delay,
            order,
            clear,
        } => {
            let path = format!("/api/{}/{}/power-policy", base(&kind)?, enc(&id));
            let body = if clear {
                json!({})
            } else {
                // Keep what is already set unless overridden here.
                let cur: Value = api.get(&path).await?;
                let mut o = cur["override"].clone();
                if let Some(s) = stop {
                    o["stop_action"] = json!(s);
                }
                if let Some(s) = start {
                    o["start_action"] = json!(s);
                }
                if let Some(d) = delay {
                    o["start_delay_secs"] = json!(d);
                }
                if let Some(d) = order {
                    o["start_order"] = json!(d);
                }
                o
            };
            let v: Value = api.put(&path, &body).await?;
            output::print_single(&v["effective"], OutputFormat::from_str(output_format))?;
        }
        PowerCommands::NodeDefault {
            stop,
            start,
            delay,
            dir,
        } => {
            let cur: Value = api.get("/api/nodes/guest-power").await?;
            let mut n = cur["node"].clone();
            if let Some(s) = stop {
                n["stop_action"] = json!(s);
            }
            if let Some(s) = start {
                n["start_action"] = json!(s);
            }
            if let Some(d) = delay {
                n["start_delay_secs"] = json!(d);
            }
            if let Some(d) = dir {
                n["save_dir"] = if d.is_empty() { Value::Null } else { json!(d) };
            }
            let v: Value = api.put("/api/nodes/guest-power", &n).await?;
            output::print_single(&v, OutputFormat::from_str(output_format))?;
        }
    }
    Ok(())
}
