# API access log

Every request to a node's API is recorded: who made it, from where, what it
asked for and how it ended. Administrators use it to see who accessed the
API, which requests failed, over an exact time range, with instant
statistics and click-through filters (for example a security audit of one
API token). The page is **Datacenter > Access Log** (`/access-log`).

This is different from the audit log (`/audit`): the audit log keeps explicit
security events (logins, permission changes); the access log keeps one line
per HTTP request.

## What is recorded

| Field | Meaning |
|---|---|
| `ts_ms`, `time` | request arrival (UTC, millisecond precision) |
| `node` | node that served it |
| `user`, `realm` | the account, when authentication got that far |
| `auth_method` | credential presented: `jwt` (login token), `session` (cookie), `api_key`, `cluster` (node-to-node); empty = none. Kept for rejected requests too |
| `token_id`, `token_name` | the API token (never its secret) |
| `client` | client address: the TCP peer, or the `X-Forwarded-For` client when the peer is a trusted proxy |
| `method`, `path` | the request (the query string is not stored, so tickets in URLs never reach the log) |
| `path_group` | the route template, e.g. `/api/vms/:id/start`; a path no route serves is `unmatched` |
| `status`, `duration_ms`, `bytes_out` | answer; time until the response headers were ready; size when known up front (streamed bodies have none) |
| `user_agent` | truncated to `max_user_agent_len` |

Public routes (login, health), refused requests (401, 403, 429) and unknown
`/api` paths are recorded as well. The web UI's own static files are not,
unless `log_static` is on.

## How it stays fast and bounded

* Requests only try to put their entry on a bounded queue; a writer task
  inserts entries in batches. A full queue drops the entry and counts it
  (`collector.dropped`, shown on the page), so logging never slows a request.
* Storage is a SQLite file of its own (`<data_dir>/access-log.db`, indexed by
  time, user, token, client, status and route). Each node keeps its own log.
* Retention is enforced every `prune_interval_secs`: entries older than
  `retention_days` go, then the oldest entries until the data fits in
  `max_mb`.

## API

All routes need the privilege `Sys.Audit` (changing settings: `Sys.Modify`).

* `GET /api/access-log` - `{total, limit, offset, entries, nodes}`, newest first.
* `GET /api/access-log/stats` - totals, 4xx+5xx count, counts per time
  bucket split by status class, and top clients, users, tokens, routes,
  status codes and methods for the same filter, plus the collector counters.
* `GET /api/access-log/:id?node=` - one entry.
* `GET|PUT /api/access-log/settings` - the settings below.

Filters (all optional, combined with AND): `start`, `end` (Unix seconds, or
`2026-11-25T14:03:07` / RFC 3339, UTC; the end bound includes its whole
second), `user`, `realm`, `token` (id or name), `client`, `auth_method`,
`method`, `path` (substring, any case), `group` (route template, exact),
`status` (`404` or a class `2xx` `3xx` `4xx` `5xx`), `node`, `limit`,
`offset`, and `buckets` (stats only).

`node` is `local` (default), `all` (this node and every online peer; peers
answer on `/api/ha/peer/access-log/*`, accepted only with a cluster node
token) or one node's name. An unreachable peer is reported per node in
`nodes[].error` and never fails the whole query when `all` is asked.

## Settings (typed keys, declarative `access_log:`)

| Key | Default | Range / meaning |
|---|---|---|
| `enabled` | `true` | record requests |
| `retention_days` | `30` | 1-3650 |
| `max_mb` | `256` | 1-1048576 |
| `trusted_proxies` | `[]` | IPs/CIDRs whose `X-Forwarded-For` is believed (the nearest untrusted hop is the client) |
| `exclude_path_prefixes` | `[]` | paths not recorded (e.g. a probe) |
| `log_static` | `false` | also record web UI file requests |
| `queue_capacity` | `8192` | 64-1000000 (applies at the next start) |
| `batch_size` | `256` | 1-10000 |
| `flush_interval_ms` | `500` | 50-60000 |
| `prune_interval_secs` | `600` | 10-86400 |
| `max_user_agent_len` | `256` | 16-1024 |
| `default_page_size` / `max_page_size` | `100` / `1000` | |
| `stats_buckets` / `stats_top_n` | `48` / `10` | |
| `peer_timeout_secs` | `10` | 1-120 |
| `db_file` | `access-log.db` | plain file name in the data directory (applies at the next start) |

```yaml
version: 1
access_log:
  retention_days: 90
  trusted_proxies: [10.0.0.0/8]
```

`GET /api/declarative/export` includes the section; `plan` and `apply`
manage it like the other node settings.

## Page

Filter bar (every control labelled; time range with exact start and end,
quick ranges), statistics panel (a bar chart and top lists), table, row
detail (the Details link or a double-click) and a collapsible settings
panel. The whole filter is in the URL. Every value in the statistics panel,
every bar, and the user, token, client, route and status in each row is a
real link that applies that filter, so a filtered view can be bookmarked or
shared. A node whose access log database cannot be opened shows an empty
state (HTTP 424, reason `access_log_unavailable`).

## Tests

* unit: client address and trusted proxies, route grouping, settings
  validation (`horcrux-common`), time parsing, filters, statistics and
  retention (`horcrux-api/src/api/access_log/`), queue drop and batching,
  cluster merge, declarative plan/export.
* integration (`horcrux-api/tests/it/access_log_tests.rs`): requests with a
  password session and an API token queried back with every filter, the
  statistics, privileges, trusted proxies, settings, restart persistence.
* browser (`horcrux-ui/e2e/access_log_e2e.py`): filter, statistics,
  click-through, exact time range, detail, settings, desktop and phone.
* load (`horcrux-api/tests/access_log_load.py`): 10000 requests with the log
  off and on; p99 latency must not grow by more than a few milliseconds.
