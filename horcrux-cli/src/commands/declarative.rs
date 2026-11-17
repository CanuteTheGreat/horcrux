//! `horcrux plan|apply|export|desired|drift|gitops|history`: declarative
//! cluster configuration. The spec file is sent as text; the server parses,
//! plans and applies it, keeps the apply history and the GitOps state.
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
    let mut body = json!({"spec": spec, "dry_run": dry_run, "source": "cli"});
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

/// `horcrux export [-f out.yaml] [--include-hashes]`
pub async fn export(api: &ApiClient, file: Option<&str>, include_hashes: bool) -> Result<()> {
    let path = if include_hashes {
        "/api/declarative/export?include_hashes=true"
    } else {
        "/api/declarative/export"
    };
    let r: Value = api.get(path).await?;
    let yaml = r["spec"].as_str().context("no spec in the reply")?;
    match file {
        Some(f) => {
            write_private(f, yaml, include_hashes).with_context(|| format!("writing {f}"))?
        }
        None => print!("{yaml}"),
    }
    Ok(())
}

/// Write `text`; a file with password hashes in it is 0600 from the start.
fn write_private(path: &str, text: &str, private: bool) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let _ = private;
    o.open(path)?.write_all(text.as_bytes())
}

// --------------------------------------------------------- desired state

pub async fn desired_set(api: &ApiClient, file: &str) -> Result<()> {
    let spec = read_spec(file)?;
    let r: Value = api
        .put(
            "/api/declarative/desired",
            &json!({"spec": spec, "source": "cli"}),
        )
        .await?;
    println!(
        "stored desired spec {} ({})",
        short(r["hash"].as_str().unwrap_or("")),
        if r["in_sync"] == json!(true) {
            "node is in sync".to_string()
        } else {
            format!("{} difference(s): `horcrux drift`", r["drift_count"])
        }
    );
    Ok(())
}

pub async fn desired_get(api: &ApiClient, file: Option<&str>) -> Result<()> {
    let r: Value = api.get("/api/declarative/desired").await?;
    if r["configured"] != json!(true) {
        bail!("no desired spec is stored (`horcrux desired set -f spec.yaml`)");
    }
    let spec = r["spec"].as_str().context("no spec in the reply")?;
    match file {
        Some(f) => std::fs::write(f, spec).with_context(|| format!("writing {f}"))?,
        None => print!("{spec}"),
    }
    Ok(())
}

pub async fn desired_validate(api: &ApiClient, file: &str) -> Result<()> {
    let spec = read_spec(file)?;
    let r: Value = api
        .post("/api/declarative/desired/validate", &json!({"spec": spec}))
        .await?;
    if r["ok"] != json!(true) {
        if let Some(e) = r["error"].as_str() {
            bail!("{e}");
        }
        print_plan(&r["plan"]);
        bail!("the spec cannot be applied");
    }
    print_plan(&r["plan"]);
    Ok(())
}

pub async fn desired_apply(api: &ApiClient, dry_run: bool) -> Result<()> {
    let r: Value = api
        .post(
            "/api/declarative/desired/apply",
            &json!({"dry_run": dry_run, "source": "cli"}),
        )
        .await?;
    print_report(&r, dry_run)
}

pub async fn desired_clear(api: &ApiClient) -> Result<()> {
    api.delete("/api/declarative/desired").await?;
    println!("desired spec removed");
    Ok(())
}

fn short(s: &str) -> &str {
    &s[..s.len().min(12)]
}

/// `horcrux drift`: exit 2 when the node differs from the desired spec.
pub async fn drift(api: &ApiClient, refresh: bool) -> Result<()> {
    let path = if refresh {
        "/api/declarative/drift?refresh=true"
    } else {
        "/api/declarative/drift"
    };
    let r: Value = api.get(path).await?;
    if r["configured"] != json!(true) {
        println!("no desired spec is stored (`horcrux desired set -f spec.yaml`)");
        return Ok(());
    }
    println!("checked {}", r["checked_at"].as_str().unwrap_or(""));
    if let Some(e) = r["error"].as_str() {
        println!("error: {e}");
    }
    for e in r["errors"].as_array().into_iter().flatten() {
        println!("error: {}", e.as_str().unwrap_or(""));
    }
    for i in r["items"].as_array().into_iter().flatten() {
        println!(
            "{} {:<12} {:<20} {:<14} expected {}  actual {}{}",
            if i["blocked"] == json!(true) {
                "!"
            } else {
                "~"
            },
            i["kind"].as_str().unwrap_or(""),
            i["name"].as_str().unwrap_or(""),
            i["field"].as_str().unwrap_or(""),
            i["expected"].as_str().unwrap_or(""),
            i["actual"].as_str().unwrap_or(""),
            i["note"]
                .as_str()
                .filter(|n| !n.is_empty())
                .map(|n| format!("  ({n})"))
                .unwrap_or_default()
        );
    }
    if r["in_sync"] == json!(true) {
        println!("in sync");
        return Ok(());
    }
    println!("{} difference(s), {} blocked", r["count"], r["blocked"]);
    std::process::exit(2);
}

// ----------------------------------------------------------------- gitops

pub async fn gitops_status(api: &ApiClient) -> Result<()> {
    let r: Value = api.get("/api/declarative/gitops/status").await?;
    if r["configured"] != json!(true) {
        println!("GitOps is not configured (`horcrux gitops config -f gitops.yaml`)");
        return Ok(());
    }
    let g = |k: &str| r[k].as_str().unwrap_or("-").to_string();
    println!("repository   {} ({})", g("url"), g("branch"));
    println!("file         {}", g("path"));
    println!(
        "mode         {} every {} s, signatures {}",
        r["mode"].as_str().unwrap_or("-"),
        r["interval_secs"],
        if r["verify_signatures"] == json!(true) {
            "verified"
        } else {
            "not checked"
        }
    );
    println!(
        "last commit  {} {}",
        short(&g("last_commit")),
        g("last_commit_subject")
    );
    println!("signature    {} {}", g("signature"), g("signer"));
    println!("last sync    {}", g("last_sync"));
    println!("result       {}", g("last_result"));
    if let Some(e) = r["last_error"].as_str() {
        println!("last error   {e}");
    }
    for p in r["pending"].as_array().into_iter().flatten() {
        println!(
            "pending      {}  commit {}  {} change(s): {}",
            p["hash"].as_str().unwrap_or(""),
            short(p["commit"].as_str().unwrap_or("")),
            p["plan"]["changes"]
                .as_array()
                .map(|c| c.len())
                .unwrap_or(0),
            p["subject"].as_str().unwrap_or("")
        );
        print_plan(&p["plan"]);
    }
    Ok(())
}

pub async fn gitops_config(api: &ApiClient, file: Option<&str>) -> Result<()> {
    match file {
        None => {
            let r: Value = api.get("/api/declarative/gitops/config").await?;
            if r["configured"] != json!(true) {
                println!("GitOps is not configured");
            } else {
                print!("{}", serde_yaml::to_string(&r["config"])?);
            }
        }
        Some(f) => {
            let text = read_spec(f)?;
            let body: Value =
                serde_yaml::from_str(&text).context("the config is not valid YAML/JSON")?;
            let _: Value = api.put("/api/declarative/gitops/config", &body).await?;
            println!("GitOps configuration saved; `horcrux gitops sync` fetches now");
        }
    }
    Ok(())
}

pub async fn gitops_disable(api: &ApiClient) -> Result<()> {
    api.delete("/api/declarative/gitops/config").await?;
    println!("GitOps configuration removed");
    Ok(())
}

pub async fn gitops_sync(api: &ApiClient) -> Result<()> {
    let r: Value = api.post("/api/declarative/gitops/sync", &json!({})).await?;
    println!("{}", r["result"].as_str().unwrap_or(""));
    if let Some(h) = r["pending"].as_str() {
        println!("approve with: horcrux gitops approve {h}");
    }
    if r["ok"] != json!(true) {
        bail!("the commit was not applied cleanly (see `horcrux history`)");
    }
    Ok(())
}

pub async fn gitops_approve(api: &ApiClient, hash: &str) -> Result<()> {
    let r: Value = api
        .post(
            &format!("/api/declarative/gitops/approve/{hash}"),
            &json!({}),
        )
        .await?;
    print_report(&r, false)
}

pub async fn gitops_reject(api: &ApiClient, hash: &str, reason: Option<&str>) -> Result<()> {
    let _: Value = api
        .post(
            &format!("/api/declarative/gitops/reject/{hash}"),
            &json!({"reason": reason}),
        )
        .await?;
    println!("rejected {hash}");
    Ok(())
}

// ---------------------------------------------------------------- history

pub async fn history_list(api: &ApiClient, limit: usize) -> Result<()> {
    let r: Value = api
        .get(&format!("/api/declarative/history?limit={limit}"))
        .await?;
    let items = r["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("no applies recorded yet");
        return Ok(());
    }
    for i in items {
        println!(
            "{}  {}  {:<9} {:<10} {} {}",
            i["id"].as_str().unwrap_or(""),
            i["time"].as_str().unwrap_or(""),
            i["source"].as_str().unwrap_or(""),
            i["actor"].as_str().unwrap_or(""),
            if i["ok"] == json!(true) {
                "ok  "
            } else {
                "FAIL"
            },
            i["summary"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

pub async fn history_show(api: &ApiClient, id: &str, spec: bool) -> Result<()> {
    let r: Value = api.get(&format!("/api/declarative/history/{id}")).await?;
    if spec {
        print!("{}", r["spec"].as_str().unwrap_or(""));
        return Ok(());
    }
    println!(
        "{} at {} by {} via {}{}",
        id,
        r["time"].as_str().unwrap_or(""),
        r["actor"].as_str().unwrap_or(""),
        r["source"].as_str().unwrap_or(""),
        r["commit"]
            .as_str()
            .map(|c| format!(" (commit {})", short(c)))
            .unwrap_or_default()
    );
    println!("{}", r["summary"].as_str().unwrap_or(""));
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
    Ok(())
}

pub async fn history_rollback(
    api: &ApiClient,
    id: &str,
    to: Option<&str>,
    set_desired: bool,
    dry_run: bool,
) -> Result<()> {
    let r: Value = api
        .post(
            &format!("/api/declarative/history/{id}/rollback"),
            &json!({"dry_run": dry_run, "to": to, "set_desired": set_desired}),
        )
        .await?;
    println!(
        "{} {} using the spec of {}",
        if dry_run {
            "would roll back"
        } else {
            "rolled back"
        },
        r["rollback_of"].as_str().unwrap_or(id),
        r["restored_from"].as_str().unwrap_or("?")
    );
    print_report(&r["report"], dry_run)
}

/// `horcrux apply --rollback [--id ID] [--to ID]`: the latest apply by default.
pub async fn rollback_latest(
    api: &ApiClient,
    id: Option<&str>,
    to: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let id = match id {
        Some(i) => i.to_string(),
        None => {
            let r: Value = api.get("/api/declarative/history?limit=1").await?;
            r["items"][0]["id"]
                .as_str()
                .context("nothing has been applied yet: there is nothing to roll back")?
                .to_string()
        }
    };
    history_rollback(api, &id, to, false, dry_run).await
}

/// Print an apply report (plan for a dry run, results otherwise); error when
/// not everything was applied.
fn print_report(r: &Value, dry_run: bool) -> Result<()> {
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
