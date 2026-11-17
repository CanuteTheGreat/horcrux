//! GitOps: follow a spec file in a git repository.
//!
//! The node fetches `branch` of `url` with the `git` binary into a bare repo
//! under its data directory, optionally verifies the HEAD commit's signature
//! (SSH or OpenPGP, against signers the operator allowed), reads `path` at
//! that commit, parses it and plans it against the live node.
//!
//! * `mode: auto` applies every change (drift is corrected on each sync too).
//! * `mode: approve` stores the plan under a hash of `commit + spec`; nothing
//!   changes until `POST .../gitops/approve/<hash>`, which applies it only if
//!   the repository head is still that commit (and, with verification on,
//!   still signed). `reject` drops it and remembers the commit.
//!
//! One repository can serve several nodes: `path` may contain `{node}`
//! (replaced by the node's name), e.g. `nodes/{node}.yaml`. That is the
//! chosen multi-node mechanism; there is no `nodes:` section in the spec.
//!
//! Credentials are references, never the secret: `file:/abs/path`,
//! `env:NAME` or `vault:path#key` (the node's Vault integration).
use super::{drift, parse, plan, service, store};
use crate::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

/// Messages that the API maps to a 424 (the node lacks a binary).
pub const GIT_MISSING: &str = "the git binary is not installed on this node (install dev-vcs/git)";
pub const GPG_MISSING: &str = "gpg is not installed on this node (install app-crypt/gnupg)";

static SYNC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Auto,
    Approve,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Signing {
    /// `ssh` (git's SSH signatures, `allowed_signers` format) or `gpg`.
    pub format: String,
    /// Inline signers: `allowed_signers` lines (ssh) or armored public keys (gpg).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_signers: Option<String>,
    /// Or a file on the node with the same content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_signers_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Credentials {
    /// Defaults to `git`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Reference to the password/token: `file:`, `env:` or `vault:path#key`.
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    pub url: String,
    #[serde(default = "main_branch")]
    pub branch: String,
    /// File in the repo; `{node}` is replaced by the node name.
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_mode")]
    pub mode: Mode,
    #[serde(default)]
    pub verify_signatures: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing: Option<Signing>,
    /// HTTPS username/token reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Credentials>,
    /// Reference to an SSH private key for `ssh://` / `git@host:` URLs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<String>,
    /// Reference to the shared secret that signs push webhooks (HMAC-SHA256).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
    /// Overrides the node name used for `{node}` (default: the hostname).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
}

fn yes() -> bool {
    true
}
fn main_branch() -> String {
    "main".into()
}
fn default_path() -> String {
    "horcrux.yaml".into()
}
fn default_interval() -> u64 {
    300
}
fn default_mode() -> Mode {
    Mode::Approve
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitState {
    #[serde(default)]
    pub last_commit: Option<String>,
    #[serde(default)]
    pub last_commit_subject: Option<String>,
    #[serde(default)]
    pub last_commit_author: Option<String>,
    #[serde(default)]
    pub last_commit_time: Option<String>,
    /// Signature of `last_commit`: good | bad | unchecked.
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub signer: Option<String>,
    #[serde(default)]
    pub last_sync: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Human line about the last sync's outcome.
    #[serde(default)]
    pub last_result: Option<String>,
    #[serde(default)]
    pub last_applied_commit: Option<String>,
    /// A commit the operator rejected: not queued again.
    #[serde(default)]
    pub rejected_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub hash: String,
    pub commit: String,
    pub subject: String,
    pub spec: String,
    pub spec_hash: String,
    pub plan: super::Plan,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Schedule,
    Manual,
    Webhook,
}

#[derive(Debug)]
pub enum SyncError {
    /// Fetching failed (network, auth, missing branch).
    Fetch(String),
    /// The commit was refused (signature) or its content is unusable.
    Refused(String),
    Busy(String),
    NotFound(String),
    Conflict(String),
    Other(String),
}

impl SyncError {
    pub fn message(&self) -> &str {
        match self {
            SyncError::Fetch(m)
            | SyncError::Refused(m)
            | SyncError::Busy(m)
            | SyncError::NotFound(m)
            | SyncError::Conflict(m)
            | SyncError::Other(m) => m,
        }
    }
}

// ------------------------------------------------------------ validation

pub fn valid_secret_ref(r: &str) -> Result<(), String> {
    let bad = || {
        Err("secrets are given as references, never plaintext: \
             file:/absolute/path, env:NAME or vault:path#key"
            .to_string())
    };
    if let Some(p) = r.strip_prefix("file:") {
        return if p.starts_with('/') && !p.contains('\0') {
            Ok(())
        } else {
            bad()
        };
    }
    if let Some(n) = r.strip_prefix("env:") {
        return if !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            Ok(())
        } else {
            bad()
        };
    }
    if let Some(v) = r.strip_prefix("vault:") {
        return match v.split_once('#') {
            Some((p, k)) if !p.is_empty() && !k.is_empty() => Ok(()),
            _ => bad(),
        };
    }
    bad()
}

fn url_is_ssh(u: &str) -> bool {
    u.starts_with("ssh://") || (!u.contains("://") && u.contains('@') && u.contains(':'))
}

impl GitConfig {
    pub fn validate(&self) -> Result<(), String> {
        let u = self.url.trim();
        if u.is_empty()
            || u.starts_with('-')
            || u.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(
                "url is required (https://, ssh://, git@host:path, file:// or an absolute path)"
                    .into(),
            );
        }
        let known = ["https://", "http://", "ssh://", "git://", "file://"];
        if !(known.iter().any(|p| u.starts_with(p)) || u.starts_with('/') || url_is_ssh(u)) {
            return Err("url must start with https://, http://, ssh://, git://, file://, be git@host:path or an absolute path".into());
        }
        if let Some(rest) = u
            .strip_prefix("https://")
            .or_else(|| u.strip_prefix("http://"))
        {
            let auth = rest.split('/').next().unwrap_or("");
            if auth.contains('@') {
                return Err(
                    "don't put credentials in the url: use `credentials` with a secret reference"
                        .into(),
                );
            }
        }
        let b = &self.branch;
        if b.is_empty()
            || b.starts_with('-')
            || b.contains("..")
            || !b
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
        {
            return Err("branch: letters, digits and ._/- only".into());
        }
        let p = &self.path;
        if p.is_empty()
            || p.starts_with('/')
            || p.starts_with('-')
            || p.split('/').any(|s| s == ".." || s.is_empty())
            || !p
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._/-{}".contains(c))
            || p.replace("{node}", "").contains(['{', '}'])
        {
            return Err("path: a relative file path; the only placeholder is {node}".into());
        }
        if !(10..=86_400).contains(&self.interval_secs) {
            return Err("interval_secs must be 10-86400".into());
        }
        if self.verify_signatures {
            let s = self.signing.as_ref().ok_or(
                "verify_signatures needs `signing` (format and allowed_signers) so unknown signers are refused",
            )?;
            if !["ssh", "gpg"].contains(&s.format.as_str()) {
                return Err("signing.format must be ssh or gpg".into());
            }
            if s.allowed_signers.as_deref().unwrap_or("").trim().is_empty()
                && s.allowed_signers_file.as_deref().unwrap_or("").is_empty()
            {
                return Err(
                    "signing needs allowed_signers (inline) or allowed_signers_file".into(),
                );
            }
            if let Some(f) = &s.allowed_signers_file {
                if !f.starts_with('/') {
                    return Err("signing.allowed_signers_file must be an absolute path".into());
                }
            }
        }
        for r in [
            self.credentials.as_ref().map(|c| &c.secret),
            self.ssh_key.as_ref(),
            self.webhook_secret.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            valid_secret_ref(r)?;
        }
        if self.credentials.is_some() && !u.starts_with("https://") {
            return Err("credentials are only sent over https://; use ssh_key for ssh".into());
        }
        if self.ssh_key.is_some() && !url_is_ssh(u) {
            return Err("ssh_key needs an ssh:// or git@host:path url".into());
        }
        if let Some(n) = &self.node_name {
            if !valid_node(n) {
                return Err("node_name: letters, digits and ._- only".into());
            }
        }
        Ok(())
    }

    pub fn node(&self) -> String {
        match &self.node_name {
            Some(n) => n.clone(),
            None => sanitize_node(&drift::node_name()),
        }
    }

    /// `path` with `{node}` resolved.
    pub fn resolved_path(&self) -> String {
        self.path.replace("{node}", &self.node())
    }
}

fn valid_node(n: &str) -> bool {
    !n.is_empty()
        && !n.starts_with('-')
        && !n.starts_with('.')
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

fn sanitize_node(n: &str) -> String {
    let s: String = n
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    s.trim_start_matches(['-', '.']).to_string()
}

// ------------------------------------------------------------- secrets

pub async fn resolve_secret(state: &Arc<AppState>, r: &str) -> Result<String, String> {
    valid_secret_ref(r)?;
    if let Some(p) = r.strip_prefix("file:") {
        let t = tokio::fs::read_to_string(p)
            .await
            .map_err(|e| format!("secret {r}: {e}"))?;
        return Ok(t.trim_end_matches(['\n', '\r']).to_string());
    }
    if let Some(n) = r.strip_prefix("env:") {
        return std::env::var(n).map_err(|_| format!("secret {r}: variable not set"));
    }
    let (path, key) = r
        .strip_prefix("vault:")
        .and_then(|v| v.split_once('#'))
        .ok_or("bad vault reference")?;
    let s = state
        .vault_manager
        .read_secret(path)
        .await
        .map_err(|e| format!("secret {r}: {e}"))?;
    s.data
        .get(key)
        .cloned()
        .ok_or_else(|| format!("secret {r}: no key {key:?}"))
}

// ------------------------------------------------------------------ git

fn state_dir(state: &Arc<AppState>) -> PathBuf {
    state
        .config
        .paths
        .data_dir
        .join("declarative")
        .join("gitops")
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

async fn run_git(
    dir: &Path,
    cwd: Option<&Path>,
    args: &[&str],
    envs: &[(String, String)],
) -> Result<Out, String> {
    let mut c = Command::new("git");
    if let Some(cwd) = cwd {
        c.current_dir(cwd);
    }
    c.args(args)
        .env("HOME", dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", "file:git:http:https:ssh")
        .env("LC_ALL", "C")
        .kill_on_drop(true);
    for (k, v) in envs {
        c.env(k, v);
    }
    let fut = c.output();
    let out = tokio::time::timeout(Duration::from_secs(180), fut)
        .await
        .map_err(|_| "git timed out after 180 s".to_string())?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GIT_MISSING.to_string()
            } else {
                format!("running git: {e}")
            }
        })?;
    Ok(Out {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

async fn fetch_env(
    state: &Arc<AppState>,
    cfg: &GitConfig,
    dir: &Path,
) -> Result<Vec<(String, String)>, String> {
    let mut env = Vec::new();
    if let Some(c) = &cfg.credentials {
        let pass = resolve_secret(state, &c.secret).await?;
        let script = dir.join("askpass.sh");
        tokio::fs::write(
            &script,
            "#!/bin/sh\ncase \"$1\" in Username*) printf '%s' \"$HX_GIT_USER\";; *) printf '%s' \"$HX_GIT_PASS\";; esac\n",
        )
        .await
        .map_err(|e| e.to_string())?;
        set_mode(&script, 0o700).await;
        env.push(("GIT_ASKPASS".into(), script.to_string_lossy().to_string()));
        env.push((
            "HX_GIT_USER".into(),
            c.username.clone().unwrap_or_else(|| "git".into()),
        ));
        env.push(("HX_GIT_PASS".into(), pass));
    }
    if let Some(k) = &cfg.ssh_key {
        let key = resolve_secret(state, k).await?;
        let kf = dir.join("ssh_key");
        let mut body = key;
        if !body.ends_with('\n') {
            body.push('\n');
        }
        tokio::fs::write(&kf, body)
            .await
            .map_err(|e| e.to_string())?;
        set_mode(&kf, 0o600).await;
        env.push((
            "GIT_SSH_COMMAND".into(),
            format!(
                "ssh -i {} -o IdentitiesOnly=yes -o BatchMode=yes \
                 -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile={}",
                kf.display(),
                dir.join("known_hosts").display()
            ),
        ));
    }
    Ok(env)
}

async fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = tokio::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).await;
}

/// Hide anything that looks like a secret from messages that come back from git.
fn scrub(msg: &str) -> String {
    msg.lines()
        .map(|l| {
            if let Some(i) = l.find("://") {
                if let Some(at) = l[i + 3..].find('@') {
                    let end = l[i + 3..].find('/').unwrap_or(l.len() - i - 3);
                    if at < end {
                        return format!("{}://***@{}", &l[..i], &l[i + 3 + at + 1..]);
                    }
                }
            }
            l.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct Head {
    pub commit: String,
    pub subject: String,
    pub author: String,
    pub time: String,
}

/// Fetch the branch into the bare repo and return its head.
async fn fetch(state: &Arc<AppState>, cfg: &GitConfig) -> Result<Head, SyncError> {
    let dir = state_dir(state);
    let repo = dir.join("repo");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| SyncError::Other(e.to_string()))?;
    set_mode(&dir, 0o700).await;
    if !repo.join("HEAD").exists() {
        let r = run_git(
            &dir,
            None,
            &["init", "-q", "--bare", repo.to_str().unwrap_or("repo")],
            &[],
        )
        .await
        .map_err(SyncError::Other)?;
        if !r.ok {
            return Err(SyncError::Other(format!("git init: {}", r.stderr)));
        }
    }
    let env = fetch_env(state, cfg, &dir)
        .await
        .map_err(SyncError::Fetch)?;
    let refspec = format!("+refs/heads/{}:refs/horcrux/head", cfg.branch);
    let r = run_git(
        &dir,
        Some(&repo),
        &[
            "-c",
            "credential.helper=",
            "fetch",
            "--quiet",
            "--no-tags",
            "--force",
            "--",
            cfg.url.trim(),
            &refspec,
        ],
        &env,
    )
    .await
    .map_err(SyncError::Fetch)?;
    // the key file only exists for the duration of the fetch
    let _ = tokio::fs::remove_file(dir.join("ssh_key")).await;
    if !r.ok {
        return Err(SyncError::Fetch(format!(
            "git fetch failed: {}",
            scrub(&r.stderr)
        )));
    }
    head_info(&dir, &repo).await
}

async fn head_info(dir: &Path, repo: &Path) -> Result<Head, SyncError> {
    let r = run_git(
        dir,
        Some(repo),
        &[
            "log",
            "-1",
            "--format=%H%n%s%n%an%n%cI",
            "refs/horcrux/head",
        ],
        &[],
    )
    .await
    .map_err(SyncError::Other)?;
    if !r.ok {
        return Err(SyncError::Fetch(format!(
            "reading the fetched head: {}",
            r.stderr
        )));
    }
    let mut l = r.stdout.lines();
    Ok(Head {
        commit: l.next().unwrap_or("").to_string(),
        subject: l.next().unwrap_or("").to_string(),
        author: l.next().unwrap_or("").to_string(),
        time: l.next().unwrap_or("").to_string(),
    })
}

/// Check the commit's signature against the allowed signers. `Ok(signer)` or
/// `Err(why)` (refuse).
async fn verify(state: &Arc<AppState>, cfg: &GitConfig, commit: &str) -> Result<String, String> {
    let dir = state_dir(state);
    let repo = dir.join("repo");
    let s = cfg
        .signing
        .as_ref()
        .ok_or("signature verification has no signers configured")?;
    let keys = match (&s.allowed_signers, &s.allowed_signers_file) {
        (Some(t), _) if !t.trim().is_empty() => t.clone(),
        (_, Some(f)) => tokio::fs::read_to_string(f)
            .await
            .map_err(|e| format!("allowed signers file {f}: {e}"))?,
        _ => return Err("no allowed signers configured".into()),
    };
    let work = dir.join("verify");
    let _ = tokio::fs::remove_dir_all(&work).await;
    tokio::fs::create_dir_all(&work)
        .await
        .map_err(|e| e.to_string())?;
    set_mode(&work, 0o700).await;
    let r = if s.format == "ssh" {
        let f = work.join("allowed_signers");
        tokio::fs::write(&f, keys)
            .await
            .map_err(|e| e.to_string())?;
        run_git(
            &dir,
            Some(&repo),
            &[
                "-c",
                "gpg.format=ssh",
                "-c",
                &format!("gpg.ssh.allowedSignersFile={}", f.display()),
                "verify-commit",
                "--raw",
                commit,
            ],
            &[],
        )
        .await?
    } else {
        let home = work.join("gnupg");
        tokio::fs::create_dir_all(&home)
            .await
            .map_err(|e| e.to_string())?;
        set_mode(&home, 0o700).await;
        let kf = work.join("keys.asc");
        tokio::fs::write(&kf, keys)
            .await
            .map_err(|e| e.to_string())?;
        let imp = Command::new("gpg")
            .args(["--batch", "--quiet", "--homedir"])
            .arg(&home)
            .arg("--import")
            .arg(&kf)
            .output()
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    GPG_MISSING.to_string()
                } else {
                    e.to_string()
                }
            })?;
        if !imp.status.success() {
            return Err(format!(
                "the allowed signers are not valid OpenPGP keys: {}",
                String::from_utf8_lossy(&imp.stderr).trim()
            ));
        }
        run_git(
            &dir,
            Some(&repo),
            &["-c", "gpg.format=openpgp", "verify-commit", "--raw", commit],
            &[("GNUPGHOME".into(), home.to_string_lossy().to_string())],
        )
        .await?
    };
    let _ = tokio::fs::remove_dir_all(&work).await;
    if r.ok {
        let who = r
            .stderr
            .lines()
            .find(|l| l.contains("GOODSIG") || l.contains("Good \""))
            .map(|l| l.trim().to_string())
            .unwrap_or_else(|| "good signature".into());
        Ok(who)
    } else {
        let why = if r.stderr.trim().is_empty() {
            "the commit is not signed".to_string()
        } else {
            scrub(&r.stderr)
        };
        Err(why)
    }
}

async fn read_spec(
    state: &Arc<AppState>,
    cfg: &GitConfig,
    commit: &str,
) -> Result<String, SyncError> {
    let dir = state_dir(state);
    let repo = dir.join("repo");
    let path = cfg.resolved_path();
    let obj = format!("{commit}:{path}");
    let r = run_git(&dir, Some(&repo), &["show", &obj], &[])
        .await
        .map_err(SyncError::Other)?;
    if !r.ok {
        return Err(SyncError::Refused(format!(
            "{path} does not exist at commit {} on {}",
            &commit[..commit.len().min(12)],
            cfg.branch
        )));
    }
    if r.stdout.len() > 1_048_576 {
        return Err(SyncError::Refused(format!("{path} is larger than 1 MiB")));
    }
    Ok(r.stdout)
}

// ------------------------------------------------------------------ sync

#[derive(Debug, Serialize)]
pub struct SyncOutcome {
    pub commit: String,
    pub result: String,
    pub in_sync: bool,
    pub applied: bool,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_id: Option<String>,
}

fn pending_hash(commit: &str, spec_hash: &str) -> String {
    store::sha256_hex(&format!("{commit}\n{spec_hash}"))[..16].to_string()
}

async fn note(state: &Arc<AppState>, f: impl FnOnce(&mut GitState)) {
    let mut s = store::gitops_state(state).await;
    f(&mut s);
    let _ = store::put_gitops_state(state, &s).await;
}

/// One sync: fetch, verify, parse, plan; then apply (auto) or queue (approve).
pub async fn sync(state: &Arc<AppState>, trigger: Trigger) -> Result<SyncOutcome, SyncError> {
    let cfg = store::gitops_config(state)
        .await
        .ok_or_else(|| SyncError::NotFound("GitOps is not configured".into()))?;
    if !cfg.enabled && trigger != Trigger::Manual {
        return Err(SyncError::Other("GitOps is disabled".into()));
    }
    let _g = SYNC_LOCK
        .try_lock()
        .map_err(|_| SyncError::Busy("a sync is already running".into()))?;
    let r = sync_inner(state, &cfg, trigger).await;
    if let Err(e) = &r {
        let m = e.message().to_string();
        note(state, |s| {
            s.last_sync = Some(store::now());
            s.last_error = Some(m);
        })
        .await;
    }
    r
}

async fn sync_inner(
    state: &Arc<AppState>,
    cfg: &GitConfig,
    trigger: Trigger,
) -> Result<SyncOutcome, SyncError> {
    let head = fetch(state, cfg).await?;
    let mut signature = "unchecked".to_string();
    let mut signer = None;
    if cfg.verify_signatures {
        match verify(state, cfg, &head.commit).await {
            Ok(who) => {
                signature = "good".into();
                signer = Some(who);
            }
            Err(why) => {
                let (c, h) = (head.commit.clone(), head);
                note(state, |s| {
                    s.last_commit = Some(c.clone());
                    s.last_commit_subject = Some(h.subject.clone());
                    s.last_commit_author = Some(h.author.clone());
                    s.last_commit_time = Some(h.time.clone());
                    s.signature = Some("bad".into());
                    s.signer = None;
                })
                .await;
                return Err(SyncError::Refused(format!(
                    "signature verification failed for commit {}: {why}; nothing was applied",
                    &c[..c.len().min(12)]
                )));
            }
        }
    }
    let record_head = |s: &mut GitState, h: &Head, sig: &str, who: &Option<String>| {
        s.last_commit = Some(h.commit.clone());
        s.last_commit_subject = Some(h.subject.clone());
        s.last_commit_author = Some(h.author.clone());
        s.last_commit_time = Some(h.time.clone());
        s.signature = Some(sig.to_string());
        s.signer = who.clone();
        s.last_sync = Some(store::now());
    };
    let text = read_spec(state, cfg, &head.commit).await?;
    let spec = match parse(&text) {
        Ok(s) => s,
        Err(e) => {
            note(state, |s| record_head(s, &head, &signature, &signer)).await;
            return Err(SyncError::Refused(format!(
                "the spec in {} at {} is invalid: {e}",
                cfg.resolved_path(),
                &head.commit[..head.commit.len().min(12)]
            )));
        }
    };
    let spec_hash = store::sha256_hex(&text);
    let short = head.commit[..head.commit.len().min(12)].to_string();
    let actor = match trigger {
        Trigger::Manual => "gitops (manual sync)",
        Trigger::Webhook => "gitops (webhook)",
        Trigger::Schedule => "gitops",
    };

    match cfg.mode {
        Mode::Auto => {
            let mut origin = service::Origin::new(actor, "gitops");
            origin.commit = Some(head.commit.clone());
            let rep = service::apply_text(state, &text, None, false, origin)
                .await
                .map_err(|e| match e {
                    service::SvcError::Busy(m) => SyncError::Busy(m),
                    service::SvcError::Invalid(m) => SyncError::Refused(m),
                    service::SvcError::NotFound(m) | service::SvcError::Internal(m) => {
                        SyncError::Other(m)
                    }
                })?;
            let _ =
                store::put_desired(state, &text, actor, "gitops", Some(head.commit.clone())).await;
            let nothing = rep.plan.changes.is_empty() && rep.plan.errors.is_empty();
            let result = if nothing {
                format!("in sync at {short}")
            } else if rep.ok {
                format!("applied {} change(s) from {short}", rep.results.len())
            } else {
                format!("apply of {short} was not complete (see the history entry)")
            };
            let err = (!rep.ok).then(|| {
                if rep.plan.errors.is_empty() {
                    format!("apply of {short}: some changes failed or are blocked")
                } else {
                    format!("apply of {short} refused: {}", rep.plan.errors.join("; "))
                }
            });
            note(state, |s| {
                record_head(s, &head, &signature, &signer);
                s.last_result = Some(result.clone());
                s.last_error = err.clone();
                if rep.ok {
                    s.last_applied_commit = Some(head.commit.clone());
                }
            })
            .await;
            drift::refresh_soon(state.clone());
            Ok(SyncOutcome {
                commit: head.commit,
                result,
                in_sync: nothing,
                applied: !nothing,
                ok: rep.ok,
                pending: None,
                history_id: rep.history_id,
            })
        }
        Mode::Approve => {
            let live = super::exec::gather(state).await;
            let p = plan(&spec, &live, spec.prune);
            let hash = pending_hash(&head.commit, &spec_hash);
            let stored = store::desired(state).await;
            let rejected = store::gitops_state(state).await.rejected_commit;
            if p.in_sync() {
                for old in store::pending(state).await {
                    store::delete_pending(state, &old.hash).await;
                }
                if stored.as_ref().map(|d| d.hash.as_str()) != Some(spec_hash.as_str()) {
                    let _ = store::put_desired(
                        state,
                        &text,
                        actor,
                        "gitops",
                        Some(head.commit.clone()),
                    )
                    .await;
                }
                let result = format!("in sync at {short}");
                note(state, |s| {
                    record_head(s, &head, &signature, &signer);
                    s.last_result = Some(result.clone());
                    s.last_error = None;
                    s.last_applied_commit = Some(head.commit.clone());
                })
                .await;
                drift::refresh_soon(state.clone());
                return Ok(SyncOutcome {
                    commit: head.commit,
                    result,
                    in_sync: true,
                    applied: false,
                    ok: true,
                    pending: None,
                    history_id: None,
                });
            }
            if rejected.as_deref() == Some(head.commit.as_str()) {
                let result = format!("commit {short} was rejected; waiting for a new commit");
                note(state, |s| {
                    record_head(s, &head, &signature, &signer);
                    s.last_result = Some(result.clone());
                    s.last_error = None;
                })
                .await;
                return Ok(SyncOutcome {
                    commit: head.commit,
                    result,
                    in_sync: false,
                    applied: false,
                    ok: true,
                    pending: None,
                    history_id: None,
                });
            }
            for old in store::pending(state).await {
                if old.hash != hash {
                    store::delete_pending(state, &old.hash).await;
                }
            }
            let created = store::pending(state)
                .await
                .into_iter()
                .find(|x| x.hash == hash)
                .map(|x| x.created_at)
                .unwrap_or_else(store::now);
            let pend = Pending {
                hash: hash.clone(),
                commit: head.commit.clone(),
                subject: head.subject.clone(),
                spec: text,
                spec_hash,
                plan: p,
                created_at: created,
            };
            store::put_pending(state, &pend)
                .await
                .map_err(SyncError::Other)?;
            let result = format!(
                "{short} needs approval: {} change(s) pending",
                pend.plan.changes.len()
            );
            note(state, |s| {
                record_head(s, &head, &signature, &signer);
                s.last_result = Some(result.clone());
                s.last_error = None;
            })
            .await;
            Ok(SyncOutcome {
                commit: head.commit,
                result,
                in_sync: false,
                applied: false,
                ok: true,
                pending: Some(hash),
                history_id: None,
            })
        }
    }
}

/// Apply a pending plan if the repository head is still its commit.
pub async fn approve(
    state: &Arc<AppState>,
    hash: &str,
    actor: &str,
) -> Result<super::exec::ApplyReport, SyncError> {
    let cfg = store::gitops_config(state)
        .await
        .ok_or_else(|| SyncError::NotFound("GitOps is not configured".into()))?;
    let pend = store::pending(state)
        .await
        .into_iter()
        .find(|p| p.hash == hash)
        .ok_or_else(|| {
            SyncError::NotFound(format!(
                "no pending approval {hash} (it may have been replaced by a newer commit)"
            ))
        })?;
    let _g = SYNC_LOCK
        .try_lock()
        .map_err(|_| SyncError::Busy("a sync is running; try again in a moment".into()))?;
    let head = fetch(state, &cfg).await?;
    if head.commit != pend.commit {
        // replace the stale plan with one for the new head right away
        let fresh = sync_inner(state, &cfg, Trigger::Manual).await.ok();
        let now = fresh
            .and_then(|o| o.pending)
            .map(|h| format!(" (new plan {h} is waiting)"))
            .unwrap_or_default();
        return Err(SyncError::Conflict(format!(
            "the repository head moved to {} since this plan was made for {}: the old approval is stale{now}",
            &head.commit[..head.commit.len().min(12)],
            &pend.commit[..pend.commit.len().min(12)]
        )));
    }
    if cfg.verify_signatures {
        verify(state, &cfg, &head.commit)
            .await
            .map_err(|why| SyncError::Refused(format!("signature verification failed: {why}")))?;
    }
    let text = read_spec(state, &cfg, &head.commit).await?;
    if store::sha256_hex(&text) != pend.spec_hash {
        return Err(SyncError::Conflict(
            "the spec file changed under this approval".into(),
        ));
    }
    let mut origin = service::Origin::new(actor, "gitops");
    origin.commit = Some(pend.commit.clone());
    let rep = service::apply_text(state, &text, None, false, origin)
        .await
        .map_err(|e| match e {
            service::SvcError::Busy(m) => SyncError::Busy(m),
            service::SvcError::Invalid(m) => SyncError::Refused(m),
            service::SvcError::NotFound(m) | service::SvcError::Internal(m) => SyncError::Other(m),
        })?;
    store::delete_pending(state, hash).await;
    let _ = store::put_desired(state, &text, actor, "gitops", Some(pend.commit.clone())).await;
    let short = pend.commit[..pend.commit.len().min(12)].to_string();
    note(state, |s| {
        s.last_sync = Some(store::now());
        s.last_result = Some(format!("approved by {actor}: {short}"));
        s.last_error =
            (!rep.ok).then(|| format!("apply of {short}: some changes failed or are blocked"));
        if rep.ok {
            s.last_applied_commit = Some(pend.commit.clone());
        }
    })
    .await;
    drift::refresh_soon(state.clone());
    Ok(rep)
}

pub async fn reject(
    state: &Arc<AppState>,
    hash: &str,
    actor: &str,
    reason: Option<&str>,
) -> Result<(), SyncError> {
    let pend = store::pending(state)
        .await
        .into_iter()
        .find(|p| p.hash == hash)
        .ok_or_else(|| SyncError::NotFound(format!("no pending approval {hash}")))?;
    store::delete_pending(state, hash).await;
    let short = pend.commit[..pend.commit.len().min(12)].to_string();
    let why = reason.unwrap_or("").trim().to_string();
    note(state, |s| {
        s.rejected_commit = Some(pend.commit.clone());
        s.last_result = Some(if why.is_empty() {
            format!("{short} rejected by {actor}")
        } else {
            format!("{short} rejected by {actor}: {why}")
        });
    })
    .await;
    Ok(())
}

/// Public status for the API/UI/CLI. Never includes secrets (only references).
pub async fn status(state: &Arc<AppState>) -> Value {
    let cfg = store::gitops_config(state).await;
    let st = store::gitops_state(state).await;
    let pend = store::pending(state).await;
    json!({
        "configured": cfg.is_some(),
        "enabled": cfg.as_ref().map(|c| c.enabled).unwrap_or(false),
        "mode": cfg.as_ref().map(|c| c.mode),
        "url": cfg.as_ref().map(|c| scrub(&c.url)),
        "branch": cfg.as_ref().map(|c| c.branch.clone()),
        "path": cfg.as_ref().map(|c| c.resolved_path()),
        "verify_signatures": cfg.as_ref().map(|c| c.verify_signatures).unwrap_or(false),
        "interval_secs": cfg.as_ref().map(|c| c.interval_secs),
        "node": cfg.as_ref().map(|c| c.node()),
        "last_commit": st.last_commit,
        "last_commit_subject": st.last_commit_subject,
        "last_commit_author": st.last_commit_author,
        "last_commit_time": st.last_commit_time,
        "signature": st.signature,
        "signer": st.signer,
        "last_sync": st.last_sync,
        "last_error": st.last_error,
        "last_result": st.last_result,
        "last_applied_commit": st.last_applied_commit,
        "rejected_commit": st.rejected_commit,
        "pending": pend,
    })
}

/// HMAC-SHA256 check of a push webhook body against the configured secret.
pub async fn verify_webhook(
    state: &Arc<AppState>,
    headers: &axum::http::HeaderMap,
    body: &[u8],
) -> Result<(), (u16, String)> {
    let cfg = store::gitops_config(state)
        .await
        .ok_or((404, "GitOps is not configured".to_string()))?;
    let r = cfg
        .webhook_secret
        .as_ref()
        .ok_or((404, "no webhook secret is configured".to_string()))?;
    let secret = resolve_secret(state, r).await.map_err(|e| (503, e))?;
    let sig = [
        "x-hub-signature-256",
        "x-forgejo-signature",
        "x-gitea-signature",
        "x-horcrux-signature",
    ]
    .iter()
    .find_map(|h| headers.get(*h).and_then(|v| v.to_str().ok()))
    .ok_or((401, "missing signature header".to_string()))?;
    if !horcrux_monitoring::webhooks::verify_signature(&secret, body, sig) {
        return Err((401, "bad signature".into()));
    }
    // a push to another branch is none of our business
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        if let Some(r) = v.get("ref").and_then(|r| r.as_str()) {
            if r != format!("refs/heads/{}", cfg.branch) {
                return Err((200, format!("ignored push to {r}")));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> GitConfig {
        serde_json::from_value(json!({"url": "https://git.example.com/x/y.git"})).unwrap()
    }

    #[test]
    fn defaults_are_the_safe_ones() {
        let c = cfg();
        assert_eq!(c.mode, Mode::Approve);
        assert_eq!(c.branch, "main");
        assert_eq!(c.interval_secs, 300);
        assert!(!c.verify_signatures);
        c.validate().unwrap();
    }

    #[test]
    fn plaintext_secrets_and_bad_values_are_refused() {
        let mut c = cfg();
        c.credentials = Some(Credentials {
            username: None,
            secret: "hunter2".into(),
        });
        assert!(c.validate().unwrap_err().contains("references"));
        c.credentials = Some(Credentials {
            username: None,
            secret: "file:/etc/horcrux/git-token".into(),
        });
        c.validate().unwrap();
        let mut c = cfg();
        c.url = "https://user:pw@git.example.com/x".into();
        assert!(c.validate().unwrap_err().contains("credentials"));
        c.url = "--upload-pack=evil".into();
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.path = "../etc/passwd".into();
        assert!(c.validate().is_err());
        c.path = "nodes/{node}.yaml".into();
        c.validate().unwrap();
        c.path = "nodes/{other}.yaml".into();
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.branch = "a b".into();
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.verify_signatures = true;
        assert!(c.validate().unwrap_err().contains("signing"));
        c.signing = Some(Signing {
            format: "ssh".into(),
            allowed_signers: Some("ci ssh-ed25519 AAAA".into()),
            allowed_signers_file: None,
        });
        c.validate().unwrap();
        let mut c = cfg();
        c.credentials = Some(Credentials {
            username: None,
            secret: "env:TOKEN".into(),
        });
        c.url = "git@host:x/y.git".into();
        assert!(c.validate().unwrap_err().contains("https"));
    }

    #[test]
    fn node_path_is_substituted_and_sanitized() {
        let mut c = cfg();
        c.path = "nodes/{node}.yaml".into();
        c.node_name = Some("pve-1".into());
        assert_eq!(c.resolved_path(), "nodes/pve-1.yaml");
        assert_eq!(sanitize_node("weird name"), "weird-name");
        assert_eq!(sanitize_node("../x"), "x");
    }

    #[test]
    fn scrub_hides_url_userinfo() {
        assert_eq!(
            scrub("fatal: unable to access 'https://tok:abc@host/x.git/'"),
            "fatal: unable to access 'https://***@host/x.git/'"
        );
    }

    #[test]
    fn pending_hash_depends_on_commit_and_spec() {
        assert_ne!(pending_hash("a", "x"), pending_hash("b", "x"));
        assert_ne!(pending_hash("a", "x"), pending_hash("a", "y"));
        assert_eq!(pending_hash("a", "x").len(), 16);
    }
}
