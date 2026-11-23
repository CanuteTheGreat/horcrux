# Alert delivery

Alerts (Monitoring > Alert Center rules) are delivered to every enabled
notification channel (Monitoring > Notifications, or
`POST /api/notifications/channels`) and to event webhooks that subscribe to
`alert_triggered` / `alert_resolved` (`POST /api/webhooks`).

Channel types: `email` (SMTP), `slack`, `teams`, `discord`, `pagerduty`,
`ntfy`, and the generic `webhook`.

## Payloads

Generic webhook channel (JSON, method and headers/auth as configured):

    {"event": "alert", "title": "[WARNING] any vm - *", "message": "...",
     "severity": "warning", "channel": "<channel name>",
     "timestamp": "<RFC 3339>", "alert": {"id": "...", "rule_id": "...",
     "rule_name": "...", "target": "...", "metric_value": 1.0,
     "threshold": 0.0, "message": "...", "severity": "...", "status": "..."}}

ntfy channel: `POST <server_url>/<topic>`, plain-text body (title, then
message), headers `Title`, `Priority` (`urgent`, `high`, `default` for
critical, warning, info, or the channel's fixed priority), `Tags:
<event>,<severity>`, and `Authorization: Bearer <token>` when a token is set.
`server_url` defaults to `https://ntfy.sh`.

E-mail: SMTP (server, port, optional login, STARTTLS/TLS option), one message
to all recipients, subject = the alert title.

Event webhook (`/api/webhooks`): JSON `{"event_type", "timestamp", "source":
"horcrux", "data": {"alert_id", "rule_id", "rule_name", "severity",
"message", "target", "metric_value", "threshold"}}`, with an HMAC signature in
`X-Horcrux-Signature` when a secret is set.

## Retry, backoff, timeouts, audit

A delivery that fails (HTTP non-2xx, no answer within the time limit,
connection refused, SMTP rejection) is retried with growing waits. After the
last failed attempt an audit-log entry of type `NotificationDeliveryFailed`
(result `Failure`, details: channel, type, event, attempts, error) is written
(Audit log page, `GET /api/audit/search`); the channel's failure counter
increases.

Settings (`[notifications]` in config.toml, or the environment; defaults
apply when unset):

| key | env | default | meaning |
| --- | --- | --- | --- |
| `attempts` | `HORCRUX_NOTIFY_ATTEMPTS` | 3 | total attempts per channel and alert (1-20) |
| `backoff_ms` | `HORCRUX_NOTIFY_BACKOFF_MS` | 1000 | wait before the second attempt |
| `backoff_factor` | `HORCRUX_NOTIFY_BACKOFF_FACTOR` | 2 | each further wait = previous x this |
| `timeout_secs` | `HORCRUX_NOTIFY_TIMEOUT_SECS` | 20 | time limit of one attempt (HTTP or SMTP) |

Event webhooks keep their own per-webhook `retry_count` (retries after the
first attempt) and `timeout_seconds`; their first wait follows `backoff_ms`
and doubles per retry (capped at 60 s).

## Proof

`horcrux-api/tests/it/alert_delivery_tests.rs` (run by CI's integration suites
against the real server): a VM-count rule fires and the webhook, ntfy, e-mail
channels receive it at local sinks with the payload above; a sink that fails
once is retried and then gets it; 500, hanging, refused and SMTP-rejecting
sinks are retried with the configured backoff and each is audited.
