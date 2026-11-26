# System Log viewer

Page: **Node > System Log** (`/system/logs`). It reads the node's own log: journald when the host runs it (systemd
images), otherwise the syslog file the logger writes (`/var/log/syslog`, `messages` or `everything/current`; the OpenRC
images use sysklogd, whose `messages` file leaves out errors, so `syslog` - everything - is read first). In a cluster the node picker shows another node's log, fetched over the peer API.

## What the page does

- Entries are colour-coded by priority (0 emergency .. 7 debug). On a syslog host the priority is a guess from the message
  text unless the logger writes it; a guessed priority is marked with a `?`.
- **Time span**: presets or a custom range. Times are `2026-11-25 10:00`, RFC 3339, epoch seconds or relative (`1h`, `30m`, `2d`).
- **Minimum priority**, **unit / service**, **syslog identifier** (`*` matches any text: `ssh*`), **kernel messages only**, **search**.
- **Boot**: all boots, the current one, the previous one or an older one. A separator row stands wherever the node rebooted.
- **Live**: new entries appear on top as they are logged, with the same filters. The stream closes by itself when nothing new
  arrived for a while (`follow_idle_secs`), after `follow_max_secs`, or after `follow_max_entries`; the page says why and
  Live can be switched on again.
- Export (JSON or CSV) uses the filters; Clear empties the log (needs Sys.Modify).
- Every filter can be preset in the address, e.g. `/system/logs?unit=ssh&priority=3&boot=-1`.

## API

| Route | Purpose |
|---|---|
| `GET /api/system/logs` | entries, newest first |
| `GET /api/system/logs/export?format=txt\|csv\|json` | same filters |
| `GET /api/system/logs/boots` | boots (`index` 0 = current, -1 = previous, `boot_id`, first/last entry) |
| `GET /api/system/logs/units` | unit / service names to pick from |
| `GET /api/system/logs/info` | which source, and what it cannot give |
| `GET`/`PUT /api/system/logs/settings` | the typed limits below |
| `POST /api/system/logs/stream/ticket`, `GET /api/system/logs/stream/ws/:ticket?<filters>` | live mode (WebSocket, single-use ticket) |
| `DELETE /api/system/logs` | clear |

Filters (query): `since`, `until`, `priority` (0-7, minimum), `unit`, `identifier` (`*` wildcard), `kernel` (`true`), `boot`
(`0`, `-1` or a boot id), `node`, plus `level`, `service`, `search`, `limit`. Every entry has `priority`, `priority_name`,
`unit`, `identifier`, `pid`, `boot_id`, `hostname`, `kernel`, `node` and the older `level` / `service`.
Reads need Sys.Audit; settings and clear need Sys.Modify. A node without any log answers 424 `no-log-source`.

## What a syslog file cannot give

The OpenRC images log to a plain file. It has no priority field (guessed from the text, or read from a `<PRI>` prefix when
the logger writes one), no unit (the unit filter matches the program name, which is the OpenRC service name for most
services), and no boot id. Reboots are found from the login records (`wtmp` BOOT_TIME), kernel `Linux version` lines and the
kernel's boot time; older boots get ids like `boot-<unix time>`. Kernel messages of the current boot come from the kernel ring buffer when the logger writes none to its file (sysklogd
does not); earlier boots' kernel messages are there only if the logger recorded them. Only the last `syslog_tail_bytes`
of the file are searched.
The page shows these limits as a note on such nodes.

## Settings (typed keys)

`GET/PUT /api/system/logs/settings`, the "Log viewer limits" section of the page, and the `logs` section of the declarative
config (`docs/design/declarative-config.md`):

| Key | Default | Meaning |
|---|---|---|
| `max_lines` | 5000 | most lines a request or export returns |
| `default_lines` | 200 | lines when none is asked for |
| `max_scan_lines` | 200000 | lines scanned while looking for matches |
| `query_timeout_secs` | 30 | journalctl / peer answer time |
| `syslog_tail_bytes` | 8 MiB | how much of a syslog file is searched |
| `syslog_files` | syslog, messages, everything/current | files tried in order |
| `wtmp_path` | /var/log/wtmp | login records for reboots |
| `kmsg_path` | /dev/kmsg | kernel ring buffer, read for kernel messages of the current boot when the logger does not write them (empty = never) |
| `follow_poll_ms` | 1000 | live mode look interval |
| `follow_idle_secs` | 600 | live mode closes after this long without news |
| `follow_max_secs` | 3600 | longest stream |
| `follow_max_entries` | 20000 | entries per stream |
| `follow_max_streams` | 8 | streams open at once on the node |

## Tests

- Unit: argument building for `journalctl`, wildcard matching, syslog / journal parsing, boot detection
  (`horcrux-api/src/api/system_admin/logs/`), fixtures in `horcrux-api/tests/fixtures/system-logs/`.
- Real OpenRC host: `horcrux-api/tests/system_admin_e2e.py` (syslog file, every filter).
- Browser: `horcrux-ui/e2e/system_log_e2e.sh` (real rsyslogd and `logger`; priority colours, wildcard, boot separator, Live).
- Real ISOs: `tools/vmtest/scenarios/system_log_viewer.py` (systemd image: journald; OpenRC image: sysklogd).
