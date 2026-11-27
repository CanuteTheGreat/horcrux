# Notification routing (matchers)

Channels (e-mail, Slack, Teams, Discord, PagerDuty, webhook) say *where* a
notification can go. Matchers say *which events go where*. Every notification
source hands an **event** to the router; the router checks it against every
enabled matcher and sends it to the union of the targets of all matchers that
match.

Web UI: Monitoring > Notifications > Routing. API: `/api/notifications/matchers`.
Declarative config: the `notification_matchers` section.

## Event sources

| Kind (`type`)        | Subtypes                                                         | Emitted when |
|----------------------|------------------------------------------------------------------|--------------|
| `alert`              | `firing`, `resolved`, `acknowledged` (alert rule id in `rule_id`) | an alert rule fires or resolves |
| `backup`             | `backup_ok`, `backup_failed`                                      | a backup job finishes (the job's `notify` setting still decides whether it notifies) |
| `package_update`     | `updates_available`, `sync_failed`                                | a package database sync finishes |
| `ha`                 | `ha_failover`, `node_failed`, `self_fence`, `recovery_failed`     | the HA supervisor recovers, fences or fails to recover a guest |
| `replication`        | `replication_ok`, `replication_failed`                            | a replication task finishes |
| `certificate_expiry` | `expiring`, `expired`, `renewed`, `renewal_failed`                | the node certificate is within its renewal window, or an ACME renewal ran |
| `security`           | `login_failed`, `login_success`, `password_changed`, `two_factor_enabled`, `two_factor_disabled` | the matching audit event |
| `test`               |                                                                  | a channel test or a test event |

Severities, lowest to highest: `info`, `notice`, `warning`, `error`,
`critical`. An event also carries an optional node, guest id, guest tags,
alert rule id and free-form `fields`.

## Matchers

A matcher is `{name, description, enabled, mode, invert, children, targets,
all_channels}`.

* `mode`: `all` (every child must match, AND) or `any` (at least one, OR).
  `invert` negates the result (NOT). A matcher or group without children
  matches every event.
* `children` is a tree of groups (`type: group`, with its own `mode`, `invert`
  and `children`) and leaves. Every node has `invert`.
* `targets` are channel ids (channel **names** in the declarative spec).
  `all_channels: true` sends to every enabled channel, including ones created
  later. Disabled channels never receive anything.

Leaves:

| `type`       | Fields                                                      | Matches when |
|--------------|-------------------------------------------------------------|--------------|
| `severity`   | `levels` (set), `min`                                       | severity is in `levels` (if given) and at least `min` (if given) |
| `event_type` | `kinds`, `subtypes`, `rule_ids`                             | every non-empty list contains the event's value |
| `node`       | `names`                                                     | the event's node is listed |
| `guest`      | `ids`, `tags`                                               | the guest id is listed or the guest has a listed tag |
| `calendar`   | `weekdays` (`mon`..`sun`, empty = every day), `start`, `end` (`HH:MM`), `timezone` | the event time, in `timezone`, falls into the window |
| `field`      | `field`, `regex`                                            | the regular expression matches the field (`title`, `body`, `node`, `guest_id`, `rule_id`, `subtype`, `kind`, `severity`, `fields.<key>`) |

Calendar rules: `end` is exclusive and may be `24:00`; `start` equal to `end`
means the whole day; an `end` earlier than `start` crosses midnight and the
window belongs to the weekday it starts on (Friday 22:00-06:00 covers Saturday
00:00-06:00 but not Friday 05:00). The time zone is an IANA name (default
`UTC`, the web UI pre-fills the browser's zone); daylight-saving changes
follow the zone's wall clock, so 09:00-17:00 stays 09:00-17:00 local all year.

Limits (typed constants, validated on save): groups nest at most 16 deep, at
most 500 conditions per matcher.

## Upgrade behaviour

A default matcher named `default` is created with the database: it matches
`alert`, `backup` and `test` events and sends them to every enabled channel,
which is exactly what notifications did before routing existed. The newer
sources (package updates, HA, replication, certificates, security) reach no
channel until a matcher routes them. Deleting every matcher means nothing is
sent; the UI says so.

## API

| Method and path | Purpose | Privilege |
|---|---|---|
| `GET /api/notifications/matchers`, `GET .../:id` | list, read | Sys.Audit |
| `POST /api/notifications/matchers`, `PUT .../:id`, `DELETE .../:id` | create, update, delete (400 with the reason on an invalid tree) | Sys.Modify |
| `PUT /api/notifications/matchers/order` `{ids}` | order of the list (display only; all matching matchers fire) | Sys.Modify |
| `POST /api/notifications/matchers/test` `{event, at?, matcher_ids?, matcher?}` | dry run: which matchers and channels the event hits, with the reason for every node; `matcher` tries an unsaved one | Sys.Audit |
| `POST /api/notifications/events` `{event}` | send a synthetic event through the real routing | Sys.Modify |

## Declarative config

```yaml
notification_matchers:
  - name: business-hours-updates-or-problems
    mode: any
    children:
      - type: group
        mode: all
        children:
          - {type: event_type, kinds: [package_update]}
          - {type: calendar, weekdays: [mon, tue, wed, thu, fri], start: "09:00", end: "17:00", timezone: Europe/Berlin}
      - {type: severity, min: warning}
    targets: [ops-webhook]
```

`export` writes the section (ids stripped, targets as channel names), `plan`
and `apply` create, update and (with `prune`) delete matchers by name, and a
target that is not an existing channel is a plan error.
