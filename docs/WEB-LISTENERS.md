# Web UI / API listeners

Owner rule: stick to standard ports. A node serves

- **HTTPS on 443**: the web UI and the API;
- **HTTP on 80**: only a redirect to HTTPS (plus ACME HTTP-01 challenges and `/api/health*` for probes);
- nothing else by default.

Everything is a typed setting with these defaults. Nothing is baked into units, scripts or images.

## Settings (all optional)

| key | default | meaning |
|---|---|---|
| `host` | `0.0.0.0` | address to bind (an IP; `::` for IPv6) |
| `port` | `443` | main HTTPS port (plain HTTP sent to it is redirected to it) |
| `http_redirect_port` | `80` | plain-HTTP port that only redirects; `null` / env `0` = off |
| `extra_https_ports` | `[]` | further HTTPS ports serving the same API, e.g. `[8006]` for existing API clients, cluster peers and Proxmox-style tools |
| `ha_fence_port` | `8026` | HA fence agent attestation port |
| `incus_https_port` | `8443` | port Incus/LXD listens on for container moves (their own standard remote-API port) |

Layers, each replacing the one before key by key:

1. built-in defaults above;
2. `[server]` in `/etc/horcrux/config.toml`;
3. environment: `HORCRUX_HOST`, `HORCRUX_PORT`, `HORCRUX_HTTP_REDIRECT_PORT` (`0` = off), `HORCRUX_EXTRA_HTTPS_PORTS` (comma list, `none`), `HORCRUX_HA_FENCE_PORT`, `HORCRUX_INCUS_HTTPS_PORT`;
4. `<data_dir>/web.json`, written by the API, the UI (Administration > Web listeners) and a declarative `web:` section. Deleting it (`DELETE /api/system/web`) goes back to layers 1-3.

`GET /api/system/web` returns `running` (what the process listens on now), `configured` (what the next start will use), `defaults`, `stored` and `restart_required`; `PUT` stores new values after validating them (distinct, non-zero ports, a real IP) and tells you when the change drops the port you are connected on. Listeners are bound at start, so a change applies at the next `horcrux-api` restart. The declarative form:

```yaml
web:
  host: 0.0.0.0
  port: 443
  http_redirect_port: 80
  extra_https_ports: [8006]
```

## Firewall

A host policy with `drop` never cuts off SSH, the configured web ports (HTTPS, extras, redirect) and the HA fence port; the `Horcrux` firewall macro expands to the configured web ports. Both follow the settings above at every start.

## Privileged ports

443 and 80 are below 1024. The shipped systemd units grant `CAP_NET_BIND_SERVICE` (`AmbientCapabilities`, and in `CapabilityBoundingSet` where one is set); the OpenRC scripts run `horcrux-api` as root, which binds them without a capability (a script that sets `command_user` must also set `capabilities="^cap_net_bind_service,..."`). The container image carries the file capability on the binary. `ci/check-web-listen-units.sh` fails CI when a unit loses this or pins a port.

If the main port cannot be bound (for example no capability) it is logged and skipped as long as another listener works; with none the service stops with an error naming the ports and the fix. A redirect port that cannot be bound only logs a warning.

## Upgrading an existing node

The first start of this version on a node that already has a database and never chose a port (no `[server] port` in config.toml, no `HORCRUX_PORT`) **keeps its historical listener**: it writes `web.json` with `port = 8006`, `http_redirect_port = 80` and `extra_https_ports = [443]`. Existing clients and cluster peers (which reach each other on the configured API port) keep working while the node also answers on the standard ports. Once every cluster member is upgraded, set `port` to 443 (and drop 8006 from the extras if nothing uses it) and restart. A node that configured a port itself keeps exactly that port. Fresh installs get the standard defaults. The decision is made once (`<data_dir>/.web-ports-v2`).

## Not on the standard ports, on purpose

- Prometheus exporter `9090`, VNC `5900-5999`, WireGuard `51820`: upstream conventions, each configurable where Horcrux owns the setting.
- NAS WebDAV share `8080` / `8443`: per-share settings; 80/443 belong to the web UI.
- Incus/LXD remote API `8443` (container moves between nodes): Incus's own standard port; configurable (`incus_https_port`).
