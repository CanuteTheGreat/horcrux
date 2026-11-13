//! One interface for the host's init system (owner rule: Horcrux fully supports OpenRC *and* systemd).
//!
//! `InitBackend` is the single place that knows `rc-service`/`rc-update` and `systemctl`. The backend is chosen at runtime
//! ([`detect`]): systemd when `/run/systemd/system` exists (that is how `sd_booted()` decides), OpenRC when `openrc-run` /
//! `rc-service` is present. Code that manages a service (the system-admin UI, cluster join, ...) goes through [`backend`]
//! instead of shelling out to one init system. Commands run through a [`CommandRunner`] so both backends are unit-tested
//! with a recording fake - no init system needed.
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Result of one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput, String>;
}

/// Runs the real command.
pub struct SystemRunner;

#[async_trait]
impl CommandRunner for SystemRunner {
    async fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput, String> {
        let out = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .map_err(|e| format!("{program}: {e}"))?;
        Ok(CmdOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitKind {
    OpenRc,
    Systemd,
}

/// Run state of a service, the same words the UI already shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Active,
    Inactive,
    Failed,
    Unknown,
}

impl ServiceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceState::Active => "active",
            ServiceState::Inactive => "inactive",
            ServiceState::Failed => "failed",
            ServiceState::Unknown => "unknown",
        }
    }
}

/// Service names reach a command line: same rule as the services API.
pub fn valid_service_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 100
        && !n.starts_with('-')
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | ':' | '\\'))
}

#[async_trait]
pub trait InitBackend: Send + Sync {
    fn kind(&self) -> InitKind;
    async fn start(&self, name: &str) -> Result<(), String>;
    async fn stop(&self, name: &str) -> Result<(), String>;
    async fn restart(&self, name: &str) -> Result<(), String>;
    /// Start at boot.
    async fn enable(&self, name: &str) -> Result<(), String>;
    async fn disable(&self, name: &str) -> Result<(), String>;
    async fn state(&self, name: &str) -> Result<ServiceState, String>;
    async fn is_enabled(&self, name: &str) -> Result<bool, String>;
    /// Does this host ship a script/unit by that name?
    fn exists(&self, name: &str) -> bool;
}

fn check(name: &str) -> Result<(), String> {
    if valid_service_name(name) {
        Ok(())
    } else {
        Err(format!("bad service name {name:?}"))
    }
}

async fn ok_or_err(r: &dyn CommandRunner, program: &str, args: &[&str]) -> Result<(), String> {
    let o = r.run(program, args).await?;
    if o.ok() {
        Ok(())
    } else {
        let msg = if o.stderr.trim().is_empty() {
            &o.stdout
        } else {
            &o.stderr
        };
        Err(format!("{program} {}: {}", args.join(" "), msg.trim()))
    }
}

// ---------------------------------------------------------------- OpenRC

pub struct OpenRcBackend<R: CommandRunner = SystemRunner> {
    runner: R,
    root: PathBuf,
}

impl Default for OpenRcBackend<SystemRunner> {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenRcBackend<SystemRunner> {
    pub fn new() -> Self {
        Self {
            runner: SystemRunner,
            root: "/".into(),
        }
    }
}

impl<R: CommandRunner> OpenRcBackend<R> {
    pub fn with(runner: R, root: impl Into<PathBuf>) -> Self {
        Self {
            runner,
            root: root.into(),
        }
    }
}

/// `rc-update show` (or `show default`): is `name` in `runlevel`? Lines look like `   sshd | default`.
pub fn parse_rc_update_show(text: &str, name: &str, runlevel: &str) -> bool {
    text.lines().any(|l| {
        let mut it = l.split('|').map(str::trim);
        match (it.next(), it.next()) {
            (Some(n), Some(rl)) => n == name && rl.split_whitespace().any(|x| x == runlevel),
            _ => false,
        }
    })
}

#[async_trait]
impl<R: CommandRunner> InitBackend for OpenRcBackend<R> {
    fn kind(&self) -> InitKind {
        InitKind::OpenRc
    }
    async fn start(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "rc-service", &[name, "start"]).await
    }
    async fn stop(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "rc-service", &[name, "stop"]).await
    }
    async fn restart(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "rc-service", &[name, "restart"]).await
    }
    async fn enable(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "rc-update", &["add", name, "default"]).await
    }
    async fn disable(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "rc-update", &["del", name]).await
    }
    async fn state(&self, name: &str) -> Result<ServiceState, String> {
        check(name)?;
        let o = self.runner.run("rc-service", &[name, "status"]).await?;
        let text = format!("{}{}", o.stdout, o.stderr);
        // ` * status: started|stopped|crashed|starting|stopping`; the exit code alone cannot tell stopped from crashed
        let word = text
            .lines()
            .find_map(|l| {
                l.trim()
                    .trim_start_matches('*')
                    .trim()
                    .strip_prefix("status:")
                    .map(|s| s.trim().to_string())
            })
            .unwrap_or_default();
        Ok(match word.as_str() {
            "started" | "starting" | "stopping" => ServiceState::Active,
            "stopped" | "inactive" => ServiceState::Inactive,
            "crashed" => ServiceState::Failed,
            _ if o.ok() => ServiceState::Active,
            _ => ServiceState::Unknown,
        })
    }
    async fn is_enabled(&self, name: &str) -> Result<bool, String> {
        check(name)?;
        let o = self.runner.run("rc-update", &["show"]).await?;
        Ok(parse_rc_update_show(&o.stdout, name, "default")
            || parse_rc_update_show(&o.stdout, name, "boot"))
    }
    fn exists(&self, name: &str) -> bool {
        valid_service_name(name) && self.root.join("etc/init.d").join(name).exists()
    }
}

// ---------------------------------------------------------------- systemd

pub struct SystemdBackend<R: CommandRunner = SystemRunner> {
    runner: R,
    root: PathBuf,
}

impl Default for SystemdBackend<SystemRunner> {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemdBackend<SystemRunner> {
    pub fn new() -> Self {
        Self {
            runner: SystemRunner,
            root: "/".into(),
        }
    }
}

impl<R: CommandRunner> SystemdBackend<R> {
    pub fn with(runner: R, root: impl Into<PathBuf>) -> Self {
        Self {
            runner,
            root: root.into(),
        }
    }
}

/// `foo` -> `foo.service`; names that already carry a unit suffix stay.
pub fn unit_name(name: &str) -> String {
    const SUFFIXES: &[&str] = &[
        ".service", ".socket", ".timer", ".target", ".mount", ".path", ".slice",
    ];
    if SUFFIXES.iter().any(|s| name.ends_with(s)) {
        name.to_string()
    } else {
        format!("{name}.service")
    }
}

#[async_trait]
impl<R: CommandRunner> InitBackend for SystemdBackend<R> {
    fn kind(&self) -> InitKind {
        InitKind::Systemd
    }
    async fn start(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "systemctl", &["start", &unit_name(name)]).await
    }
    async fn stop(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "systemctl", &["stop", &unit_name(name)]).await
    }
    async fn restart(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "systemctl", &["restart", &unit_name(name)]).await
    }
    async fn enable(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "systemctl", &["enable", &unit_name(name)]).await
    }
    async fn disable(&self, name: &str) -> Result<(), String> {
        check(name)?;
        ok_or_err(&self.runner, "systemctl", &["disable", &unit_name(name)]).await
    }
    async fn state(&self, name: &str) -> Result<ServiceState, String> {
        check(name)?;
        let o = self
            .runner
            .run("systemctl", &["is-active", &unit_name(name)])
            .await?;
        Ok(match o.stdout.trim() {
            "active" | "activating" | "deactivating" | "reloading" => ServiceState::Active,
            "inactive" => ServiceState::Inactive,
            "failed" => ServiceState::Failed,
            _ => ServiceState::Unknown,
        })
    }
    async fn is_enabled(&self, name: &str) -> Result<bool, String> {
        check(name)?;
        let o = self
            .runner
            .run("systemctl", &["is-enabled", &unit_name(name)])
            .await?;
        Ok(o.ok()
            && matches!(
                o.stdout.trim(),
                "enabled" | "enabled-runtime" | "static" | "alias"
            ))
    }
    fn exists(&self, name: &str) -> bool {
        if !valid_service_name(name) {
            return false;
        }
        let u = unit_name(name);
        [
            "usr/lib/systemd/system",
            "lib/systemd/system",
            "etc/systemd/system",
        ]
        .iter()
        .any(|d| self.root.join(d).join(&u).exists())
    }
}

// ---------------------------------------------------------------- detection

/// systemd when `/run/systemd/system` is a directory (the `sd_booted()` test), else OpenRC when its tools exist.
/// `root` and `has_tool` are parameters so tests need neither.
pub fn detect_with(root: &Path, has_tool: impl Fn(&str) -> bool) -> Option<InitKind> {
    if root.join("run/systemd/system").is_dir() && has_tool("systemctl") {
        Some(InitKind::Systemd)
    } else if has_tool("rc-service") && has_tool("rc-update") {
        Some(InitKind::OpenRc)
    } else {
        None
    }
}

fn which(tool: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
        .unwrap_or(false)
        || ["/sbin", "/usr/sbin", "/bin", "/usr/bin"]
            .iter()
            .any(|d| Path::new(d).join(tool).is_file())
}

pub fn detect() -> Option<InitKind> {
    detect_with(Path::new("/"), which)
}

/// The backend for this host; `None` in a container or chroot with no init system.
pub fn backend() -> Option<Box<dyn InitBackend>> {
    match detect()? {
        InitKind::Systemd => Some(Box::new(SystemdBackend::new())),
        InitKind::OpenRc => Some(Box::new(OpenRcBackend::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every command; answers from a script of (program+args prefix -> output).
    struct Fake {
        calls: Mutex<Vec<String>>,
        answers: Vec<(&'static str, CmdOutput)>,
    }
    fn out(code: i32, stdout: &str, stderr: &str) -> CmdOutput {
        CmdOutput {
            code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }
    impl Fake {
        fn new(answers: Vec<(&'static str, CmdOutput)>) -> Self {
            Self {
                calls: Mutex::new(vec![]),
                answers,
            }
        }
    }
    #[async_trait]
    impl CommandRunner for std::sync::Arc<Fake> {
        async fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput, String> {
            let line = format!("{program} {}", args.join(" "));
            self.calls.lock().unwrap().push(line.clone());
            Ok(self
                .answers
                .iter()
                .find(|(p, _)| line.starts_with(p))
                .map(|(_, o)| o.clone())
                .unwrap_or_else(|| out(0, "", "")))
        }
    }
    fn calls(f: &Fake) -> Vec<String> {
        f.calls.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn openrc_commands() {
        let f = std::sync::Arc::new(Fake::new(vec![]));
        let b = OpenRcBackend::with(f.clone(), "/nonexistent");
        b.start("horcrux-api").await.unwrap();
        b.stop("horcrux-api").await.unwrap();
        b.restart("horcrux-api").await.unwrap();
        b.enable("horcrux-api").await.unwrap();
        b.disable("horcrux-api").await.unwrap();
        assert_eq!(
            calls(&f),
            [
                "rc-service horcrux-api start",
                "rc-service horcrux-api stop",
                "rc-service horcrux-api restart",
                "rc-update add horcrux-api default",
                "rc-update del horcrux-api",
            ]
        );
        assert_eq!(b.kind(), InitKind::OpenRc);
    }

    #[tokio::test]
    async fn openrc_state_and_enabled() {
        let f = std::sync::Arc::new(Fake::new(vec![
            ("rc-service up status", out(0, " * status: started\n", "")),
            ("rc-service down status", out(3, " * status: stopped\n", "")),
            (
                "rc-service dead status",
                out(32, " * status: crashed\n", ""),
            ),
            ("rc-service odd status", out(1, "", "")),
            (
                "rc-update show",
                out(
                    0,
                    "   sshd | default\n   horcrux-api | default boot\n   local | nonetwork\n",
                    "",
                ),
            ),
        ]));
        let b = OpenRcBackend::with(f, "/nonexistent");
        assert_eq!(b.state("up").await.unwrap(), ServiceState::Active);
        assert_eq!(b.state("down").await.unwrap(), ServiceState::Inactive);
        assert_eq!(b.state("dead").await.unwrap(), ServiceState::Failed);
        assert_eq!(b.state("odd").await.unwrap(), ServiceState::Unknown);
        assert!(b.is_enabled("horcrux-api").await.unwrap());
        assert!(b.is_enabled("sshd").await.unwrap());
        assert!(!b.is_enabled("local").await.unwrap());
        assert!(!b.is_enabled("horcrux").await.unwrap());
    }

    #[tokio::test]
    async fn systemd_commands_add_service_suffix_once() {
        let f = std::sync::Arc::new(Fake::new(vec![]));
        let b = SystemdBackend::with(f.clone(), "/nonexistent");
        b.start("horcrux-api").await.unwrap();
        b.stop("horcrux-api.service").await.unwrap();
        b.restart("sshd").await.unwrap();
        b.enable("horcrux-guests").await.unwrap();
        b.disable("backup.timer").await.unwrap();
        assert_eq!(
            calls(&f),
            [
                "systemctl start horcrux-api.service",
                "systemctl stop horcrux-api.service",
                "systemctl restart sshd.service",
                "systemctl enable horcrux-guests.service",
                "systemctl disable backup.timer",
            ]
        );
        assert_eq!(b.kind(), InitKind::Systemd);
    }

    #[tokio::test]
    async fn systemd_state_and_enabled() {
        let f = std::sync::Arc::new(Fake::new(vec![
            ("systemctl is-active up.service", out(0, "active\n", "")),
            ("systemctl is-active down.service", out(3, "inactive\n", "")),
            ("systemctl is-active bad.service", out(3, "failed\n", "")),
            (
                "systemctl is-active start.service",
                out(0, "activating\n", ""),
            ),
            ("systemctl is-active nope.service", out(4, "unknown\n", "")),
            ("systemctl is-enabled on.service", out(0, "enabled\n", "")),
            ("systemctl is-enabled off.service", out(1, "disabled\n", "")),
            (
                "systemctl is-enabled masked.service",
                out(1, "masked\n", ""),
            ),
        ]));
        let b = SystemdBackend::with(f, "/nonexistent");
        assert_eq!(b.state("up").await.unwrap(), ServiceState::Active);
        assert_eq!(b.state("down").await.unwrap(), ServiceState::Inactive);
        assert_eq!(b.state("bad").await.unwrap(), ServiceState::Failed);
        assert_eq!(b.state("start").await.unwrap(), ServiceState::Active);
        assert_eq!(b.state("nope").await.unwrap(), ServiceState::Unknown);
        assert!(b.is_enabled("on").await.unwrap());
        assert!(!b.is_enabled("off").await.unwrap());
        assert!(!b.is_enabled("masked").await.unwrap());
    }

    #[tokio::test]
    async fn failed_command_reports_stderr_and_bad_names_never_run() {
        let f = std::sync::Arc::new(Fake::new(vec![(
            "rc-service x start",
            out(1, "", "no such service"),
        )]));
        let b = OpenRcBackend::with(f.clone(), "/nonexistent");
        let e = b.start("x").await.unwrap_err();
        assert!(e.contains("no such service"), "{e}");
        assert!(b.start("--help").await.is_err());
        assert!(b.stop("a b").await.is_err());
        let s = SystemdBackend::with(f.clone(), "/nonexistent");
        assert!(s.start("-x").await.is_err());
        assert!(s.enable("").await.is_err());
        assert_eq!(calls(&f), ["rc-service x start"]);
    }

    #[test]
    fn exists_looks_in_the_init_systems_own_dirs() {
        let d = std::env::temp_dir().join(format!("hx-init-{}", std::process::id()));
        std::fs::create_dir_all(d.join("etc/init.d")).unwrap();
        std::fs::create_dir_all(d.join("usr/lib/systemd/system")).unwrap();
        std::fs::write(d.join("etc/init.d/horcrux-api"), "").unwrap();
        std::fs::write(d.join("usr/lib/systemd/system/horcrux-api.service"), "").unwrap();
        let f = std::sync::Arc::new(Fake::new(vec![]));
        let o = OpenRcBackend::with(f.clone(), &d);
        let s = SystemdBackend::with(f, &d);
        assert!(o.exists("horcrux-api") && !o.exists("other"));
        assert!(s.exists("horcrux-api") && s.exists("horcrux-api.service") && !s.exists("other"));
        assert!(!o.exists("../etc/passwd"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn detection_prefers_systemd_only_when_booted_with_it() {
        let d = std::env::temp_dir().join(format!("hx-det-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let all = |_: &str| true;
        // both tool sets installed (a systemd box with openrc tools, or the reverse): the running init decides
        assert_eq!(detect_with(&d, all), Some(InitKind::OpenRc));
        std::fs::create_dir_all(d.join("run/systemd/system")).unwrap();
        assert_eq!(detect_with(&d, all), Some(InitKind::Systemd));
        // booted by systemd but no systemctl: fall back to whatever else works
        assert_eq!(
            detect_with(&d, |t| t != "systemctl"),
            Some(InitKind::OpenRc)
        );
        assert_eq!(detect_with(&d, |_| false), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn rc_update_parser() {
        let t = "  a | default\n  b | boot default\n garbage\n";
        assert!(parse_rc_update_show(t, "a", "default"));
        assert!(parse_rc_update_show(t, "b", "boot"));
        assert!(!parse_rc_update_show(t, "a", "boot"));
        assert!(!parse_rc_update_show(t, "garbage", "default"));
    }
}
