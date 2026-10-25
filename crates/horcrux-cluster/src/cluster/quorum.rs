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
    pub warnings: Vec<String>,
}

pub fn quorum_threshold(total_votes: u32) -> u32 {
    total_votes / 2 + 1
}

/// Plan quorum for the given voters. `tie_break_node` (optional) is the lowest
/// node id that wins a tie; with an even total we enable `auto_tie_breaker`.
pub fn plan(voters: &[Voter]) -> QuorumPlan {
    let total: u32 = voters.iter().map(|v| v.votes).sum();
    let quorum = quorum_threshold(total);
    let even = total > 0 && total.is_multiple_of(2);
    let mut warnings = Vec::new();
    if total == 0 {
        warnings.push("no voting nodes: cluster can never be quorate".to_string());
    }
    if total == 2 {
        warnings.push("2 votes tolerate no failure; add a witness/qdevice".to_string());
    }
    if even && total > 2 {
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
        split_brain_risk: even,
        auto_tie_breaker: even && total > 2,
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
}
