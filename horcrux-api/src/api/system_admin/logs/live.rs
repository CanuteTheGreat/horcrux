//! Live mode of the log viewer: a WebSocket that sends new entries as they
//! appear, with the same filters as the list. Bounded by typed settings
//! (`LogSettings`: streams at once, entries, lifetime, idle time).
//!
//! * `POST /api/system/logs/stream/ticket` -> `{ticket, ws_path}` (60 s, single use)
//! * `GET  /api/system/logs/stream/ws/:ticket?<filters>` upgrades. Text
//!   frames from the server are JSON: `{"type":"hello",...}`,
//!   `{"type":"entries","entries":[...oldest first]}`,
//!   `{"type":"closed","reason":"idle|max-time|max-entries|error",...}`.
//!   The client may send anything (it counts as a keep-alive) and closes
//!   the socket to stop.
use super::*;
use axum::extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    Path,
};
use axum::response::Response;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const TICKET_TTL: Duration = Duration::from_secs(60);

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/system/logs/stream/ticket", post(mint))
        .route("/api/system/logs/stream/ws/:ticket", get(upgrade))
}

type Tickets = Mutex<HashMap<String, (String, Instant)>>;

fn tickets() -> &'static Tickets {
    static T: OnceLock<Tickets> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Serialize)]
struct TicketResponse {
    ticket: String,
    ws_path: String,
}

async fn mint(user: AuthExt) -> Result<Json<TicketResponse>, ApiError> {
    let who = user
        .map(|u| u.0.username.clone())
        .ok_or(ApiError::AuthenticationFailed)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut t = tickets().lock().unwrap();
    t.retain(|_, (_, at)| at.elapsed() < TICKET_TTL);
    t.insert(id.clone(), (who, Instant::now()));
    Ok(Json(TicketResponse {
        ws_path: format!("/api/system/logs/stream/ws/{id}"),
        ticket: id,
    }))
}

fn redeem(id: &str, who: &str) -> bool {
    let mut t = tickets().lock().unwrap();
    match t.remove(id) {
        Some((owner, at)) => owner == who && at.elapsed() < TICKET_TTL,
        None => false,
    }
}

static OPEN: AtomicUsize = AtomicUsize::new(0);

struct Slot;
impl Slot {
    fn take(max: usize) -> Option<Slot> {
        let n = OPEN.fetch_add(1, Ordering::SeqCst);
        if n >= max {
            OPEN.fetch_sub(1, Ordering::SeqCst);
            None
        } else {
            Some(Slot)
        }
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn upgrade(
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
    user: AuthExt,
    Path(ticket): Path<String>,
    Query(f): Query<Filters>,
) -> Result<Response, ApiError> {
    let who = user
        .map(|u| u.0.username.clone())
        .ok_or(ApiError::AuthenticationFailed)?;
    if !redeem(&ticket, &who) {
        return Err(ApiError::NotFound(
            "log stream ticket is unknown, expired, already used, or belongs to another user"
                .into(),
        ));
    }
    let s = load_settings(&state).await;
    // refuse bad filters with a normal HTTP error, before upgrading
    resolved(&f, &s)?;
    if detect(&s).is_none() && remote_node(&state, f.node.as_deref()).await?.is_none() {
        return Err(no_source(&s));
    }
    let Some(slot) = Slot::take(s.follow_max_streams) else {
        return Err(ApiError::BadRequest(format!(
            "{} live log streams are already open on this node",
            s.follow_max_streams
        )));
    };
    Ok(ws.on_upgrade(move |sock| session(sock, state, f, s, slot)))
}

/// Identity of an entry for telling new ones from ones already sent.
pub fn fingerprint(e: &LogEntry) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (
        e.ts_us,
        &e.identifier,
        &e.message,
        e.pid,
        &e.unit,
        e.priority,
    )
        .hash(&mut h);
    h.finish()
}

/// Remembers what was sent for the recent past so a poll that overlaps the
/// last one sends only entries it has not sent (identical lines are counted).
#[derive(Default)]
pub struct Seen {
    counts: HashMap<u64, (u32, i64)>,
    pub newest_us: i64,
}

impl Seen {
    /// `batch` newest first. Returns the unseen entries, oldest first.
    pub fn fresh(&mut self, batch: Vec<LogEntry>) -> Vec<LogEntry> {
        let mut now_counts: HashMap<u64, u32> = HashMap::new();
        let mut out = Vec::new();
        // oldest first, so identical lines keep their order
        for e in batch.into_iter().rev() {
            let fp = fingerprint(&e);
            let n = now_counts.entry(fp).or_insert(0);
            *n += 1;
            let before = self.counts.get(&fp).map(|c| c.0).unwrap_or(0);
            let ts = e.ts_us.unwrap_or(0);
            if *n > before {
                self.newest_us = self.newest_us.max(ts);
                self.counts.insert(fp, (*n, ts));
                out.push(e);
            } else {
                self.counts.entry(fp).and_modify(|c| c.1 = ts);
            }
        }
        // forget what is older than the overlap window
        let cutoff = self.newest_us - 10_000_000;
        self.counts.retain(|_, (_, ts)| *ts >= cutoff);
        out
    }
}

fn err_text(e: &ApiError) -> String {
    match e {
        ApiError::Internal(m)
        | ApiError::BadRequest(m)
        | ApiError::NotFound(m)
        | ApiError::Forbidden(m) => m.clone(),
        ApiError::NotAvailable { message, hint, .. } => format!("{message} {hint}"),
        other => format!("{other:?}"),
    }
}

async fn send(sock: &mut WebSocket, v: serde_json::Value) -> bool {
    sock.send(Message::Text(v.to_string())).await.is_ok()
}

async fn session(
    mut sock: WebSocket,
    state: Arc<AppState>,
    f: Filters,
    s: horcrux_common::system_logs::LogSettings,
    _slot: Slot,
) {
    let started = Instant::now();
    let mut last_new = Instant::now();
    let mut sent = 0usize;
    let mut seen = Seen::default();
    // Without an explicit `since` the first look only learns what is
    // already there (the page loaded that history itself).
    let mut primed = f.since.as_deref().is_some_and(|x| !x.trim().is_empty());
    // only what happens from now on (the page loaded the history already)
    let start_us = match f.since.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
        Some(x) => parse_time(x, now_us(), local_offset()).unwrap_or_else(|_| now_us()),
        None => now_us(),
    };
    seen.newest_us = start_us;
    if !send(
        &mut sock,
        serde_json::json!({
            "type": "hello",
            "idle_secs": s.follow_idle_secs,
            "max_secs": s.follow_max_secs,
            "max_entries": s.follow_max_entries,
        }),
    )
    .await
    {
        return;
    }
    let mut tick = tokio::time::interval(Duration::from_millis(s.follow_poll_ms));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let reason = loop {
        tokio::select! {
            m = sock.recv() => match m {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => continue, // keep-alive
            },
            _ = tick.tick() => {}
        }
        if started.elapsed().as_secs() >= s.follow_max_secs {
            break "max-time";
        }
        if last_new.elapsed().as_secs() >= s.follow_idle_secs {
            break "idle";
        }
        // look a little before the newest entry so one that landed late
        // within its second is not missed; `Seen` drops the repeats
        let from = (seen.newest_us / 1_000_000 - 2).max(0);
        let q = Filters {
            since: Some(from.to_string()),
            until: None,
            limit: Some(s.max_lines),
            format: None,
            ..f.clone()
        };
        let batch = match read_node(&state, &q).await {
            Ok(b) => b,
            Err(e) => {
                let _ = send(
                    &mut sock,
                    serde_json::json!({"type":"closed","reason":"error","message": err_text(&e)}),
                )
                .await;
                return;
            }
        };
        let mut fresh = seen.fresh(batch);
        if !primed {
            primed = true;
            continue;
        }
        if fresh.is_empty() {
            continue;
        }
        last_new = Instant::now();
        let room = s.follow_max_entries.saturating_sub(sent);
        fresh.truncate(room);
        sent += fresh.len();
        if !send(
            &mut sock,
            serde_json::json!({"type":"entries","entries":fresh}),
        )
        .await
        {
            return;
        }
        if sent >= s.follow_max_entries {
            break "max-entries";
        }
    };
    let _ = send(
        &mut sock,
        serde_json::json!({"type":"closed","reason":reason}),
    )
    .await;
    let _ = sock.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(ts: i64, m: &str) -> LogEntry {
        LogEntry {
            ts_us: Some(ts * 1_000_000),
            message: m.into(),
            ..Default::default()
        }
    }

    #[test]
    fn overlapping_polls_send_each_entry_once_oldest_first() {
        let mut s = Seen::default();
        let a = s.fresh(vec![e(11, "b"), e(10, "a")]);
        assert_eq!(
            a.iter().map(|x| x.message.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        // the next poll overlaps: a, b again plus c
        let b = s.fresh(vec![e(12, "c"), e(11, "b"), e(10, "a")]);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].message, "c");
        assert!(s.fresh(vec![e(12, "c")]).is_empty());
    }

    #[test]
    fn identical_lines_in_one_second_are_counted_not_collapsed() {
        let mut s = Seen::default();
        assert_eq!(s.fresh(vec![e(10, "x")]).len(), 1);
        assert_eq!(s.fresh(vec![e(10, "x"), e(10, "x")]).len(), 1);
        assert_eq!(s.fresh(vec![e(10, "x"), e(10, "x")]).len(), 0);
    }

    #[test]
    fn stream_slots_are_bounded_and_released() {
        let a = Slot::take(2).unwrap();
        let b = Slot::take(2).unwrap();
        assert!(Slot::take(2).is_none());
        drop(a);
        assert!(Slot::take(2).is_some());
        drop(b);
    }

    #[test]
    fn tickets_work_once_and_only_for_their_owner() {
        let id = "t-test-1";
        tickets()
            .lock()
            .unwrap()
            .insert(id.into(), ("root".into(), Instant::now()));
        assert!(!redeem(id, "mallory"));
        // a wrong user's attempt burns the ticket
        assert!(!redeem(id, "root"));
        tickets()
            .lock()
            .unwrap()
            .insert(id.into(), ("root".into(), Instant::now()));
        assert!(redeem(id, "root"));
        assert!(!redeem(id, "root"));
    }
}
