//! Quorum planning for even / mixed-size clusters (e.g. 10 voters).
//!
//! Pure functions, no I/O. An even voter count can split 5/5 and stall, so the
//! plan either gives one stable node an extra vote (odd total) or enables
//! corosync's `auto_tie_breaker`. Weak nodes (witness-class) can still vote.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voter {
    pub name: String,
    pub node_id: u32,
    /// Corosync `quorum_votes` (0 = non-voting).
    pub votes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuorumPlan {
    pub total_votes: u32,
    /// Votes needed for quorum: floor(total/2)+1.
    pub quorum: u32,
    /// Voters that can fail while quorum holds.
    pub tolerated_vote_loss: u32,
    /// True when total votes is even (a clean half/half split is possible).
    pub split_brain_risk: bool,
    pub auto_tie_breaker: bool,
    /// Votes the qdevice adds (0 without one); already included in `total_votes`.
    pub qdevice_votes: u32,
    pub qdevice: Option<QDevice>,
    pub warnings: Vec<String>,
}

/// Vote-split algorithm of the external corosync-qnetd arbiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QAlgorithm {
    /// One extra vote; decides a 50/50 split. Right for an even node count.
    FfSplit,
    /// Last-man-standing: nodes-1 votes, the cluster survives down to one node.
    Lms,
}

/// An external quorum device (corosync-qdevice talking to a qnetd host).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QDevice {
    pub host: String,
    pub algorithm: QAlgorithm,
}

impl QDevice {
    /// Hostname or IP made only of letters, digits, `.`, `-`, `:` (IPv6), so it
    /// can never inject corosync.conf syntax.
    pub fn new(host: &str, algorithm: QAlgorithm) -> Option<QDevice> {
        let ok = !host.is_empty()
            && host.len() <= 253
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
        ok.then(|| QDevice {
            host: host.to_string(),
            algorithm,
        })
    }

    /// `HORCRUX_QDEVICE_HOST` (+ optional `HORCRUX_QDEVICE_ALGO=ffsplit|lms`),
    /// else the persisted setting in [`qdevice_file`] (written by
    /// `PUT /api/cluster/quorum/qdevice`).
    pub fn from_env() -> Option<QDevice> {
        if let Ok(host) = std::env::var("HORCRUX_QDEVICE_HOST") {
            let algo = match std::env::var("HORCRUX_QDEVICE_ALGO").ok().as_deref() {
                Some("lms") => QAlgorithm::Lms,
                _ => QAlgorithm::FfSplit,
            };
            return QDevice::new(host.trim(), algo);
        }
        QDevice::load_from(&qdevice_file())
    }

    /// Parse `"<host> [ffsplit|lms]"` (the persisted file format).
    pub fn parse(line: &str) -> Option<QDevice> {
        let mut it = line.split_whitespace();
        let host = it.next()?;
        let algo = match it.next() {
            Some("lms") => QAlgorithm::Lms,
            Some("ffsplit") | None => QAlgorithm::FfSplit,
            Some(_) => return None,
        };
        QDevice::new(host, algo)
    }

    pub fn algorithm_name(&self) -> &'static str {
        match self.algorithm {
            QAlgorithm::FfSplit => "ffsplit",
            QAlgorithm::Lms => "lms",
        }
    }

    pub fn load_from(path: &std::path::Path) -> Option<QDevice> {
        QDevice::parse(&std::fs::read_to_string(path).ok()?)
    }

    /// Persist (`Some`) or clear (`None`) the setting.
    pub fn save_to(path: &std::path::Path, q: Option<&QDevice>) -> std::io::Result<()> {
        match q {
            Some(q) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(path, format!("{} {}\n", q.host, q.algorithm_name()))
            }
            None => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        }
    }
}

/// Where the qdevice setting is persisted: `HORCRUX_QDEVICE_FILE` or
/// `/etc/horcrux/qdevice.conf`.
pub fn qdevice_file() -> std::path::PathBuf {
    std::env::var_os("HORCRUX_QDEVICE_FILE")
        .map(Into::into)
        .unwrap_or_else(|| "/etc/horcrux/qdevice.conf".into())
}

pub fn quorum_threshold(total_votes: u32) -> u32 {
    total_votes / 2 + 1
}

/// Plan quorum for the given voters. `tie_break_node` (optional) is the lowest
/// node id that wins a tie; with an even total we enable `auto_tie_breaker`.
pub fn plan(voters: &[Voter]) -> QuorumPlan {
    plan_with_qdevice(voters, None)
}

/// Like `plan`, with an optional external qdevice. The arbiter's votes join
/// the total (ffsplit 1, lms voters-1) and replace `auto_tie_breaker`, which
/// corosync-qdevice cannot be combined with.
pub fn plan_with_qdevice(voters: &[Voter], qdevice: Option<&QDevice>) -> QuorumPlan {
    let node_votes: u32 = voters.iter().map(|v| v.votes).sum();
    let voting = voters.iter().filter(|v| v.votes > 0).count() as u32;
    let qdevice_votes = match qdevice {
        None => 0,
        Some(q) if q.algorithm == QAlgorithm::FfSplit => 1,
        Some(_) => voting.saturating_sub(1).max(1),
    };
    let total: u32 = node_votes + qdevice_votes;
    let quorum = quorum_threshold(total);
    let even = total > 0 && total.is_multiple_of(2);
    let mut warnings = Vec::new();
    if total == 0 {
        warnings.push("no voting nodes: cluster can never be quorate".to_string());
    }
    if total == 2 && qdevice.is_none() {
        warnings.push("2 votes tolerate no failure; add a witness/qdevice".to_string());
    }
    if qdevice.is_some() && node_votes == 0 {
        warnings.push("the qdevice alone cannot make a cluster quorate".to_string());
    }
    if even && total > 2 && qdevice.is_none() {
        warnings.push(format!(
            "{} votes is even: a {}/{} partition would stall; auto_tie_breaker enabled",
            total,
            total / 2,
            total / 2
        ));
    }
    if voters.iter().any(|v| v.votes > 1) {
        warnings.push(
            "weighted votes in use: losing a heavy node costs more than one vote".to_string(),
        );
    }
    let max_vote = voters.iter().map(|v| v.votes).max().unwrap_or(0);
    if total > 0 && max_vote >= quorum {
        warnings.push("a single node holds a quorum by itself".to_string());
    }
    QuorumPlan {
        total_votes: total,
        quorum,
        tolerated_vote_loss: total.saturating_sub(quorum),
        split_brain_risk: even && qdevice.is_none(),
        auto_tie_breaker: even && total > 2 && qdevice.is_none(),
        qdevice_votes,
        qdevice: qdevice.cloned(),
        warnings,
    }
}

/// Render the corosync `quorum { }` block for a plan.
pub fn corosync_quorum_block(plan: &QuorumPlan) -> String {
    let mut s = String::from("quorum {\n    provider: corosync_votequorum\n");
    s.push_str(&format!(
        "    expected_votes: {}\n",
        plan.total_votes.max(1)
    ));
    if plan.auto_tie_breaker {
        // lowest node id survives a 50/50 split
        s.push_str("    auto_tie_breaker: 1\n    auto_tie_breaker_node: lowest\n");
    }
    if let Some(q) = &plan.qdevice {
        let algo = q.algorithm_name();
        s.push_str(&format!(
            "    device {{\n        model: net\n        votes: {}\n        net {{\n            host: {}\n            algorithm: {}\n            tls: on\n        }}\n    }}\n",
            plan.qdevice_votes, q.host, algo
        ));
    }
    s.push_str("}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voters(n: u32) -> Vec<Voter> {
        (1..=n)
            .map(|i| Voter {
                name: format!("n{i}"),
                node_id: i,
                votes: 1,
            })
            .collect()
    }

    #[test]
    fn ten_voters_even_gets_tie_breaker() {
        let p = plan(&voters(10));
        assert_eq!(p.total_votes, 10);
        assert_eq!(p.quorum, 6);
        assert_eq!(p.tolerated_vote_loss, 4);
        assert!(p.split_brain_risk);
        assert!(p.auto_tie_breaker);
        let b = corosync_quorum_block(&p);
        assert!(b.contains("expected_votes: 10"));
        assert!(b.contains("auto_tie_breaker: 1"));
    }

    #[test]
    fn odd_needs_no_tie_breaker() {
        let p = plan(&voters(9));
        assert_eq!(p.quorum, 5);
        assert!(!p.auto_tie_breaker);
        assert!(!corosync_quorum_block(&p).contains("auto_tie_breaker"));
    }

    #[test]
    fn non_voting_nodes_do_not_count() {
        let mut v = voters(10);
        v[9].votes = 0;
        let p = plan(&v);
        assert_eq!(p.total_votes, 9);
        assert!(!p.split_brain_risk);
    }

    #[test]
    fn weighted_vote_makes_total_odd() {
        let mut v = voters(10);
        v[0].votes = 2;
        let p = plan(&v);
        assert_eq!(p.total_votes, 11);
        assert!(!p.auto_tie_breaker);
        assert!(p.warnings.iter().any(|w| w.contains("weighted")));
    }

    #[test]
    fn empty_and_two_node_warn() {
        assert!(plan(&[]).warnings.iter().any(|w| w.contains("never")));
        assert!(plan(&voters(2))
            .warnings
            .iter()
            .any(|w| w.contains("witness")));
        assert_eq!(
            corosync_quorum_block(&plan(&[]))
                .matches("expected_votes: 1")
                .count(),
            1
        );
    }

    #[test]
    fn qdevice_ffsplit_adds_one_vote_and_replaces_tie_breaker() {
        let q = QDevice::new("qnetd.lab", QAlgorithm::FfSplit).unwrap();
        let p = plan_with_qdevice(&voters(10), Some(&q));
        assert_eq!(p.qdevice_votes, 1);
        assert_eq!(p.total_votes, 11);
        assert_eq!(p.quorum, 6);
        assert!(!p.auto_tie_breaker && !p.split_brain_risk);
        let b = corosync_quorum_block(&p);
        assert!(b.contains("expected_votes: 11"));
        assert!(!b.contains("auto_tie_breaker"));
        assert!(b.contains("model: net") && b.contains("host: qnetd.lab"));
        assert!(b.contains("algorithm: ffsplit") && b.contains("votes: 1"));
    }

    #[test]
    fn two_nodes_with_qdevice_survive_one_failure_without_warning() {
        let q = QDevice::new("10.0.0.9", QAlgorithm::FfSplit).unwrap();
        let p = plan_with_qdevice(&voters(2), Some(&q));
        assert_eq!((p.total_votes, p.quorum, p.tolerated_vote_loss), (3, 2, 1));
        assert!(!p.warnings.iter().any(|w| w.contains("witness")));
    }

    #[test]
    fn lms_gives_nodes_minus_one_votes() {
        let q = QDevice::new("qnetd", QAlgorithm::Lms).unwrap();
        let p = plan_with_qdevice(&voters(4), Some(&q));
        assert_eq!(p.qdevice_votes, 3);
        assert_eq!(p.total_votes, 7);
        assert!(corosync_quorum_block(&p).contains("algorithm: lms"));
    }

    #[test]
    fn qdevice_host_cannot_inject_config() {
        assert!(QDevice::new("a}\nquorum {", QAlgorithm::FfSplit).is_none());
        assert!(QDevice::new("", QAlgorithm::FfSplit).is_none());
        assert!(QDevice::new("fd00::1", QAlgorithm::FfSplit).is_some());
        assert_eq!(plan(&voters(3)).qdevice_votes, 0);
    }

    #[test]
    fn qdevice_setting_round_trips_through_its_file_and_rejects_junk() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sub").join("qdevice.conf");
        assert!(QDevice::load_from(&f).is_none());
        let q = QDevice::new("qnetd.lab", QAlgorithm::Lms).unwrap();
        QDevice::save_to(&f, Some(&q)).unwrap();
        assert_eq!(QDevice::load_from(&f), Some(q));
        QDevice::save_to(&f, None).unwrap();
        assert!(QDevice::load_from(&f).is_none());
        QDevice::save_to(&f, None).unwrap(); // clearing twice is fine
        assert_eq!(QDevice::parse("10.0.0.9").unwrap().algorithm, QAlgorithm::FfSplit);
        assert!(QDevice::parse("qnetd bogus").is_none());
        assert!(QDevice::parse("a}b").is_none());
        assert!(QDevice::parse("   ").is_none());
    }
}
