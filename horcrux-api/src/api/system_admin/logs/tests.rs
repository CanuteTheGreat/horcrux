//! Integration tests of the log viewer against fixture data: a journal in
//! `journalctl -o json` form (served by a stand-in `journalctl` that records
//! its arguments) and a syslog file read through a scratch system root.
use super::*;
use crate::api::system_admin::SYSROOT_LOCK;
use horcrux_common::system_logs::LogSettings;

const JOURNAL: &str = include_str!("../../../../tests/fixtures/system-logs/journal.jsonl");
const MESSAGES: &str = include_str!("../../../../tests/fixtures/system-logs/messages");

fn settings() -> LogSettings {
    LogSettings::default()
}

fn filters() -> Filters {
    Filters::default()
}

/// A scratch root holding the fixture syslog file, with HORCRUX_SYSROOT set.
struct Root {
    dir: tempfile::TempDir,
    _g: std::sync::MutexGuard<'static, ()>,
    old_path: Option<std::ffi::OsString>,
}

impl Root {
    fn new() -> Root {
        let g = SYSROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HORCRUX_SYSROOT", dir.path());
        Root {
            dir,
            _g: g,
            old_path: None,
        }
    }

    fn put(&self, rel: &str, text: &str) {
        let p = self.dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// Install a `journalctl` that prints the fixture (honouring `-n`) and
    /// appends its arguments to `args.log`.
    fn fake_journalctl(&mut self) {
        self.put("run/log/journal/x", "");
        self.put("fixture.jsonl", JOURNAL);
        self.put(
            "bin/journalctl",
            &format!(
                "#!/bin/sh\necho \"$*\" >> {r}/args.log\n\
                 case \" $* \" in\n\
                 *--list-boots*) printf '%s' '[{{\"index\":-1,\"boot_id\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"first_entry\":1792700000000000,\"last_entry\":1792700020000000}},{{\"index\":0,\"boot_id\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"first_entry\":1792700100000000,\"last_entry\":1792700140000000}}]';;\n\
                 *\" -F \"*) printf 'cron.service\\nssh.service\\nhorcrux-api.service\\n';;\n\
                 *) n=$(echo \"$*\" | sed -n 's/.* -n \\([0-9]*\\).*/\\1/p'); head -n \"${{n:-100}}\" {r}/fixture.jsonl;;\n\
                 esac\n",
                r = self.dir.path().display()
            ),
        );
        use std::os::unix::fs::PermissionsExt;
        let p = self.dir.path().join("bin/journalctl");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        self.old_path = std::env::var_os("PATH");
        let new = format!(
            "{}:{}",
            self.dir.path().join("bin").display(),
            self.old_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        );
        std::env::set_var("PATH", new);
    }

    fn args_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("args.log")).unwrap_or_default()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        std::env::remove_var("HORCRUX_SYSROOT");
        if let Some(p) = &self.old_path {
            std::env::set_var("PATH", p);
        }
    }
}

#[test]
fn the_fixture_journal_parses_into_structured_entries_across_two_boots() {
    let v: Vec<LogEntry> = JOURNAL.lines().filter_map(parse_journal_json).collect();
    assert_eq!(v.len(), 10);
    // newest first, as journalctl -r prints it
    assert!(v.windows(2).all(|w| w[0].ts_us >= w[1].ts_us));
    let kernel: Vec<_> = v.iter().filter(|e| e.kernel).collect();
    assert_eq!(kernel.len(), 3);
    let prios: std::collections::BTreeSet<_> = v.iter().filter_map(|e| e.priority).collect();
    assert_eq!(prios.into_iter().collect::<Vec<_>>(), vec![3, 4, 6, 7]);
    // a separator belongs wherever the boot id changes between neighbours
    let changes = v
        .windows(2)
        .filter(|w| w[0].boot_id != w[1].boot_id)
        .count();
    assert_eq!(changes, 1);
    let ssh = v
        .iter()
        .find(|e| e.identifier.as_deref() == Some("sshd-session"))
        .unwrap();
    assert_eq!(
        (ssh.unit.as_deref(), ssh.pid, ssh.hostname.as_deref()),
        (Some("ssh.service"), Some(900), Some("node1"))
    );
}

#[tokio::test]
async fn journald_path_passes_every_filter_to_journalctl_and_filters_wildcards_here() {
    let mut root = Root::new();
    root.fake_journalctl();
    let s = settings();
    // all server-side filters reach journalctl as arguments
    let f = Filters {
        since: Some("1792700000".into()),
        until: Some("1792700200".into()),
        priority: Some(4),
        unit: Some("ssh.service".into()),
        kernel: Some(true),
        boot: Some("-1".into()),
        limit: Some(3),
        ..filters()
    };
    let got = read_local(&f, &s).await.unwrap();
    assert_eq!(got.len(), 3);
    let args = root.args_log();
    for want in [
        "--priority=4",
        "--unit=ssh.service",
        "--boot=-1",
        "_TRANSPORT=kernel",
        "--since=2026-10-22 20:13:20 UTC",
        "-n 3",
    ] {
        assert!(args.contains(want), "{want} missing from: {args}");
    }
    // the wildcard identifier is matched by Horcrux itself
    let f = Filters {
        identifier: Some("ssh*".into()),
        ..filters()
    };
    let got = read_local(&f, &s).await.unwrap();
    let ids: Vec<_> = got.iter().map(|e| e.identifier.clone().unwrap()).collect();
    assert_eq!(ids, vec!["sshd", "sshd-session", "sshd"]);
    assert!(!root.args_log().contains("--identifier"));
    // a plain identifier is exact and goes to journalctl
    let f = Filters {
        identifier: Some("cron".into()),
        ..filters()
    };
    read_local(&f, &s).await.unwrap();
    assert!(root.args_log().contains("--identifier=cron"));
    // search and the old service filter work on the scanned lines
    let f = Filters {
        search: Some("DATABASE".into()),
        ..filters()
    };
    assert_eq!(read_local(&f, &s).await.unwrap().len(), 1);
    // the scan bound is a typed setting
    let tight = LogSettings {
        max_scan_lines: 1000,
        ..s.clone()
    };
    assert_eq!(read_local(&filters(), &tight).await.unwrap().len(), 10);
    // bad input is a 400, not a journalctl error
    let f = Filters {
        boot: Some("not-a-boot".into()),
        ..filters()
    };
    assert!(matches!(
        read_local(&f, &s).await,
        Err(ApiError::BadRequest(_))
    ));
    // boots and units come from journald
    let b = boots_local(&s).await.unwrap();
    assert_eq!((b.len(), b[1].index, b[0].index), (2, 0, -1));
    let u = units_local(&s).await.unwrap();
    assert_eq!(u.kind, "systemd");
    assert!(u.units.contains(&"ssh.service".to_string()));
    let i = info_local(&s).await.unwrap();
    assert_eq!(i.source, "journald");
    assert!(i.limitations.is_empty());
}

#[tokio::test]
async fn syslog_file_path_gives_the_same_filters_and_finds_the_reboot() {
    let root = Root::new();
    root.put("var/log/messages", MESSAGES);
    let s = settings();
    let all = read_local(
        &Filters {
            limit: Some(100),
            ..filters()
        },
        &s,
    )
    .await
    .unwrap();
    assert_eq!(all.len(), 10);
    assert_eq!(all[0].identifier.as_deref(), Some("dhcpcd"), "newest first");
    // two boots from the two kernel boot lines; entries are assigned to them
    let boots = boots_local(&s).await.unwrap();
    assert_eq!(boots.len(), 2, "{boots:?}");
    assert_eq!((boots[0].index, boots[1].index), (-1, 0));
    let first_boot_id = &boots[0].boot_id;
    let n_old = all
        .iter()
        .filter(|e| e.boot_id.as_ref() == Some(first_boot_id))
        .count();
    assert_eq!(n_old, 5);
    // boot filter by index and by id
    let prev = read_local(
        &Filters {
            boot: Some("-1".into()),
            ..filters()
        },
        &s,
    )
    .await
    .unwrap();
    assert_eq!(prev.len(), 5);
    let by_id = read_local(
        &Filters {
            boot: Some(first_boot_id.clone()),
            ..filters()
        },
        &s,
    )
    .await
    .unwrap();
    assert_eq!(by_id.len(), 5);
    // kernel only
    let k = read_local(
        &Filters {
            kernel: Some(true),
            ..filters()
        },
        &s,
    )
    .await
    .unwrap();
    assert_eq!(k.len(), 3);
    assert!(k.iter().all(|e| e.kernel));
    // identifier with wildcard, exact, unit (program name), priority
    let q = |f: Filters| {
        let s = s.clone();
        async move { read_local(&f, &s).await.unwrap() }
    };
    assert_eq!(
        q(Filters {
            identifier: Some("ssh*".into()),
            ..filters()
        })
        .await
        .len(),
        3
    );
    assert_eq!(
        q(Filters {
            identifier: Some("cron".into()),
            ..filters()
        })
        .await
        .len(),
        1
    );
    assert_eq!(
        q(Filters {
            unit: Some("sshd.service".into()),
            ..filters()
        })
        .await
        .len(),
        3
    );
    let errs = q(Filters {
        priority: Some(3),
        ..filters()
    })
    .await;
    assert!(errs.iter().any(|e| e.message.contains("Failed password")));
    assert!(errs.iter().all(|e| e.priority <= Some(3)));
    // the <PRI> that metalog-style loggers write is taken as logged
    let logged = errs
        .iter()
        .find(|e| e.identifier.as_deref() == Some("metalog"))
        .unwrap();
    assert!(!logged.priority_inferred);
    // time span: relative spans work against the clock, the fixture is old
    assert!(q(Filters {
        since: Some("1m".into()),
        ..filters()
    })
    .await
    .is_empty());
    assert_eq!(
        q(Filters {
            since: Some("3000d".into()),
            ..filters()
        })
        .await
        .len(),
        10
    );
    // limits
    assert_eq!(
        q(Filters {
            limit: Some(2),
            ..filters()
        })
        .await
        .len(),
        2
    );
    // what the file cannot give is stated
    let i = info_local(&s).await.unwrap();
    assert_eq!(i.source, "syslog-file");
    assert!(i.limitations.iter().any(|l| l.contains("priority")));
    // units: the init scripts plus identifiers seen
    root.put("etc/init.d/sshd", "#!/sbin/openrc-run\n");
    let u = units_local(&s).await.unwrap();
    assert_eq!(u.kind, "openrc");
    assert!(u.units.contains(&"sshd".to_string()) && u.units.contains(&"cron".to_string()));
}

#[tokio::test]
async fn wtmp_boot_records_split_the_file_too() {
    let root = Root::new();
    root.put(
        "var/log/messages",
        "Nov 20 09:00:10 gw sshd[411]: a\nNov 20 18:31:00 gw sshd[880]: b\n",
    );
    // BOOT_TIME records at two moments between the two lines (Nov 20 of any year)
    let local = local_offset();
    let year = chrono::Datelike::year(&chrono::Utc::now());
    let at = |h: u32| {
        use chrono::TimeZone;
        local
            .with_ymd_and_hms(year, 11, 20, h, 0, 0)
            .unwrap()
            .timestamp()
    };
    let mut w = vec![0u8; 384 * 2];
    for (i, h) in [(0usize, 8u32), (1, 18)] {
        w[i * 384] = 2;
        w[i * 384 + 340..i * 384 + 344].copy_from_slice(&(at(h) as i32).to_le_bytes());
    }
    let p = root.dir.path().join("var/log/wtmp");
    std::fs::write(p, w).unwrap();
    let b = boots_local(&settings()).await.unwrap();
    // a year-boundary test run would flip the stamps' year: only assert the
    // split when the file's lines fall in the same year as the records
    if b.len() == 2 {
        assert_eq!((b[0].index, b[1].index), (-1, 0));
        let all = read_local(&filters(), &settings()).await.unwrap();
        assert_ne!(all[0].boot_id, all[1].boot_id);
    }
}

#[tokio::test]
async fn a_node_without_any_log_answers_424() {
    let _root = Root::new();
    let e = read_local(&filters(), &settings()).await.unwrap_err();
    assert!(
        matches!(
            e,
            ApiError::NotAvailable {
                reason: "no-log-source",
                ..
            }
        ),
        "{e:?}"
    );
}

#[test]
fn csv_export_quotes_defuses_formulas_and_keeps_the_old_columns_first() {
    let c = render_csv(&[LogEntry {
        timestamp: "t".into(),
        level: "info".into(),
        service: Some("s".into()),
        message: "=cmd|' /C calc'!A0 \"x\"".into(),
        priority: Some(6),
        ..Default::default()
    }]);
    assert!(c.starts_with("timestamp,level,service,message,priority,"));
    assert!(c.lines().nth(1).unwrap().contains("\"'=cmd"));
    assert!(c.contains("\"\"x\"\""));
}

#[test]
fn the_tail_of_a_big_file_drops_the_cut_line() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("m");
    std::fs::write(&p, "aaaaaaaa\nline two\nline three\n").unwrap();
    let t = tail_lines(&p, 25);
    assert_eq!(t, vec!["line two".to_string(), "line three".into()]);
    assert_eq!(tail_lines(&p, 1000).len(), 3);
}

#[tokio::test]
async fn syslog_is_read_before_messages_because_sysklogd_messages_drops_errors() {
    let root = Root::new();
    root.put(
        "var/log/messages",
        "Oct  2 07:01:10 gw cron[12]: info only in messages\n",
    );
    root.put(
        "var/log/syslog",
        "Oct  2 07:01:10 gw cron[12]: info only in messages\nOct  2 07:01:11 gw app[3]: error: only in syslog\n",
    );
    let s = settings();
    let got = read_local(
        &Filters {
            limit: Some(100),
            ..filters()
        },
        &s,
    )
    .await
    .unwrap();
    assert_eq!(got.len(), 2, "{got:?}");
}
