# Storage plugins

Horcrux ships built-in storage types (directory, ZFS, LVM, Ceph RBD, CephFS,
NFS, CIFS, GlusterFS, iSCSI, BtrFS, S3). A **storage plugin** adds another
type without changing Horcrux: once its package is installed, the type shows
up in **Storage > Create Pool** next to the built-ins, with a form generated
from the settings the plugin declares. Plugin authors write no UI code.

This is the Horcrux counterpart of Proxmox VE's custom storage plugins, with
one difference by design: a plugin never runs inside the API process. It is a
separate executable that the manager starts once per operation, so a plugin
that crashes, hangs or prints garbage only fails that one call.

- Protocol versions and changes: [storage-plugin-api-changelog.md](storage-plugin-api-changelog.md)
- Rust SDK: `crates/horcrux-storage-plugin-sdk`
- Reference plugin: `crates/horcrux-storage-plugin-sshfs` (package
  `app-emulation/horcrux-storage-plugin-sshfs`)

## How a plugin is found and run

| Setting (`[storage_plugins]`, API, declarative `storage_plugins:`) | Default | Meaning |
|---|---|---|
| `plugin_dir` | `/usr/libexec/horcrux/storage-plugins` | Every executable in it is a plugin; the file name is the plugin id (`[a-z0-9][a-z0-9-]{0,31}`). Env `HORCRUX_STORAGE_PLUGIN_DIR` overrides the config file value. |
| `state_dir` | empty = `<data_dir>/storage-plugins` | Each pool gets `<state_dir>/<pool id>/` (mode 0700) for mount points, keys and caches. |
| `require_root_owned` | `true` | Run only plugins owned by root and not writable by group/others, in a directory with the same property. Plugins run as the manager's user (root). |
| `max_output_kib` | `1024` | Largest answer read from a plugin; a bigger one fails the call. |
| `timeouts.describe_secs` | 10 | `describe` |
| `timeouts.activate_secs` | 120 | `activate` (pool add, every manager start) |
| `timeouts.deactivate_secs` | 60 | `deactivate` (pool removed) |
| `timeouts.status_secs` | 15 | `status` (capacity; runs on every pool list) |
| `timeouts.list_secs` | 30 | `list_volumes`, `list_snapshots` |
| `timeouts.create_secs` | 600 | `create_volume` |
| `timeouts.delete_secs` | 300 | `delete_volume` |
| `timeouts.resize_secs` | 300 | `resize_volume` |
| `timeouts.snapshot_secs` | 300 | snapshot create / delete / rollback |
| `timeouts.path_secs` | 15 | `volume_path` |
| `timeouts.map_secs` | 60 | `map_volume`, `unmap_volume` |

The settings are in `GET/PUT /api/storage/plugin-settings` (changing them
needs Sys.Modify on `/`: the plugin directory decides what runs as root),
in **Storage > Storage plugins > Plugin settings**, and in a declarative spec:

```yaml
storage_plugins:
  plugin_dir: /usr/libexec/horcrux/storage-plugins
  timeouts: {create_secs: 900}
```

Changes made through the API or a spec are stored in
`<data_dir>/storage-plugins.json` and win over config.toml.

A plugin process runs with an empty environment except `PATH`
(`/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`), `LANG=C.UTF-8`
and `HORCRUX_STORAGE_PLUGIN_API=<version>`, with the pool's state directory
as working directory. At its timeout it is killed (SIGKILL) and the call fails
with reason `storage_plugin_timeout`. Anything a plugin starts that must
outlive the call (a FUSE daemon) must not keep the plugin's stdout or stderr
open, or the call only ends at its timeout: redirect them to `/dev/null` or a
log file in the state directory.

## The protocol

One call is one process: the manager writes ONE JSON request to stdin and
closes it; the plugin writes ONE JSON response to stdout and exits (exit code
0; the manager reads `ok`). Stderr is logged (last 64 KiB kept) and quoted in
errors when the answer is not valid JSON.

Request:

```json
{
  "api_version": 1,
  "op": "create_volume",
  "pool": {
    "id": "8b0f...", "name": "nas",
    "config": {"host": "nas.example.org", "port": 22, "remote_dir": "/srv/hx"},
    "secrets": {"password": "..."},
    "state_dir": "/var/lib/horcrux/storage-plugins/8b0f..."
  },
  "args": {"name": "vm-100-disk-0", "size_bytes": 34359738368}
}
```

`pool` is absent for `describe`. `config` holds the validated, non-secret
values with defaults filled in; `secrets` holds the `secret` fields. Neither
contains keys the plugin did not declare. Volume and snapshot names are
already checked by the manager: 1-64 of `[A-Za-z0-9._-]`, not starting with
`.` or `-`.

Response: `{"ok": true, "result": ...}` or
`{"ok": false, "error": {"code": "...", "message": "...", "hint": "..."}}`.

| `error.code` | HTTP | Use it for |
|---|---|---|
| `invalid` | 400 | bad input the user can fix (wrong password, bad name) |
| `not_found` | 404 | no such volume or snapshot |
| `exists` | 409 | the volume or snapshot already exists |
| `unsupported` | 400 | an operation the plugin does not implement |
| `unavailable` | 424 | the node lacks something: a tool, a kernel module, a reachable server. The UI shows `message` and `hint` as an empty state. |
| `failed` | 500 | anything else |

### Operations (protocol 1)

| `op` | `args` | `result` |
|---|---|---|
| `describe` | none | `{id, name, version, api_version, description, schema: [Field], capabilities}` |
| `activate` | none | `{local_dir?}`: make the pool usable on this node (mount, log in). Called at pool add and at every manager start; must be idempotent. |
| `deactivate` | none | `{}`; undo `activate` (pool removed). Idempotent. |
| `status` | none | `{active, total_bytes, available_bytes, message?}` |
| `list_volumes` | none | `[Volume]` |
| `create_volume` | `name, size_bytes, format?` | `Volume` |
| `delete_volume` | `name` | `{}` |
| `resize_volume` | `name, size_bytes` | `Volume` (grow only: the manager refuses a shrink before calling) |
| `list_snapshots` | `name` | `[{name, created?}]` |
| `create_snapshot` / `delete_snapshot` / `rollback_snapshot` | `name, snapshot` | `{}` |
| `volume_path` | `name` | `{path, format}`: what QEMU opens (file, block device or QEMU URL) |
| `map_volume` / `unmap_volume` | `name` | `{path, format}` / `{}`: for storage that needs a mapping step (default: `volume_path`, no-op) |

`Volume` is `{name, path, format, size_bytes, allocated_bytes}`.

### Capabilities

```json
{"content": ["images"], "snapshots": true, "clone": false, "resize": true,
 "shared": true, "vm_disk_dir": true, "formats": ["qcow2", "raw"]}
```

`vm_disk_dir: true` means `activate` returns a local directory in which the
manager may also create VM disks itself (like a directory pool): such a pool
is offered in Create VM and Add disk. `shared` says every cluster node sees
the same storage.

## The config schema

The schema is the plugin's whole UI. Each field:

| Key | Meaning |
|---|---|
| `key` | `[a-z][a-z0-9_]{0,63}` |
| `type` | `string`, `int`, `bool`, `enum`, `secret`, `path` |
| `label` | form label |
| `hint` | one or two sentences; shown as the label's tooltip and under the input |
| `default` | used when the field is left empty (not allowed on `secret`) |
| `min` / `max` | `int`: value range; `string`/`secret`/`path`: length range |
| `pattern` | `string`/`path`: regular expression the whole value must match (HTML `pattern` semantics) |
| `required` | must be set (after defaults) |
| `depends_on` | `{key, value?}`: the field only applies when that EARLIER field has `value` (or, without `value`, is set and truthy). A field that does not apply is hidden in the form and ignored by validation. |
| `options` | `enum`: `[{value, label}]` |
| `placeholder` | form placeholder |

The manager checks the schema itself when it reads `describe` (unique keys,
defaults of the right type and in range, enum options, `depends_on` naming an
earlier field, patterns that compile); a plugin with a broken schema is
listed with the reason and can't be used. It then validates every pool
config against the schema before the plugin sees it: unknown keys, wrong
types, out-of-range numbers, pattern mismatches and missing required fields
are a 400 that names every bad field. The web UI runs the same validation (the
SDK's `validate_config`, compiled to WebAssembly) and shows each message under
its field, and its inputs carry the same `min`/`max`/`pattern`/`required`.

`int` values may arrive as decimal strings and `bool` values as
`"true"`/`"false"` (what HTML forms send); validation turns them into JSON
numbers and booleans. An empty string means "not set".

### Secrets

`secret` fields are split off at validation and stored apart from the pool
record (config store kind `storage_plugin_secret`). The API never returns
them, the declarative export leaves them out (add them to a spec by hand
where the pool is created), and the form uses password inputs.

## Writing a plugin in Rust

```toml
[dependencies]
horcrux-storage-plugin-sdk = { path = "../horcrux-storage-plugin-sdk" }
```

```rust
use horcrux_storage_plugin_sdk::*;

struct MyNas;

impl StoragePlugin for MyNas {
    fn describe(&self) -> PluginDescription {
        PluginDescription {
            id: "mynas".into(),
            name: "My NAS".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            description: "Volumes on My NAS appliances".into(),
            schema: vec![
                Field::new("host", FieldType::String, "Appliance").required()
                    .hint("Host name or IP address of the appliance."),
                Field::new("port", FieldType::Int, "Port").default_value(8443.into())
                    .range(1, 65535),
                Field::new("api_key", FieldType::Secret, "API key").required(),
            ],
            capabilities: Capabilities { content: vec!["images".into()], ..Default::default() },
        }
    }
    fn activate(&self, pool: &PoolContext) -> PluginResult<ActivateResult> { /* ... */ }
    fn status(&self, pool: &PoolContext) -> PluginResult<StatusResult> { /* ... */ }
    // list_volumes, create_volume, ... : unimplemented ones answer `unsupported`
}

fn main() {
    std::process::exit(serve(MyNas));
}
```

`serve` reads the request, checks names, dispatches and writes the response;
`handle_request` does the same on a `Request` value for unit tests.
`<plugin> describe` (no stdin) prints the description while developing.

The SDK has no dependency beyond `serde`/`serde_json` (and `regex` with its
`pattern` feature, which only the manager needs). Any language works: the
integration test `horcrux-api/tests/it/storage_plugin_tests.rs` drives a
plugin written in Python.

## Packaging

Install the executable, root-owned and mode 0755, as
`/usr/libexec/horcrux/storage-plugins/<id>` from a package (on Gentoo an
ebuild; see `gentoo/app-emulation/horcrux-storage-plugin-sshfs`). A plugin
needs no service, so there is nothing to enable on OpenRC or systemd; list
the tools it calls (`net-fs/sshfs`, `app-emulation/qemu`) in `RDEPEND`. The
manager re-reads `describe` whenever the executable's size or modification
time changes, so an upgrade needs no restart.

## The reference SSHFS plugin

`sshfs` keeps qcow2 (or raw) volumes in a directory on an SSH server,
mounted with SSHFS under the pool's state directory. Snapshots are qcow2
internal snapshots. Settings: server, port (22), user (root), remote
directory, login with SSH key (optional key file) or password (secret),
connect timeout, host key check (trust on first use, recorded in the pool's
own known_hosts, or known hosts only) and compression. Every node mounts the
same directory, so the pool is shared, and VM disks can be created on it.

```sh
curl -sk -H "Authorization: Bearer $T" https://node/api/storage/pools \
  -d '{"name":"nas","storage_type":"plugin:sshfs",
       "config":{"host":"nas.example.org","remote_dir":"/srv/horcrux"}}'
```

## API

- `GET /api/storage/plugins`: plugin directory, supported protocol versions,
  and every plugin found (`usable`, `error`, `description` with schema).
- `GET /api/storage/plugins/:id`: one description (424
  `storage_plugin_missing` when not installed).
- `POST /api/storage/pools` with `storage_type: "plugin:<id>"` and `config`.
- Volumes: the normal pool volume routes; snapshots:
  `GET|POST /api/storage/pools/:id/volumes/:name/snapshots`,
  `DELETE .../snapshots/:snap`, `POST .../snapshots/:snap/rollback`
  (the same routes volume-chain LVM pools use; every call answers
  `{volume, snapshots: [names, oldest first], virtual_size_bytes}`, and a
  volume a VM uses is refused with 409).

Failure reasons (424): `storage_plugin_missing`, `storage_plugin_untrusted`
(ownership check), `storage_plugin_incompatible` (id mismatch, protocol
version, broken schema), `storage_plugin_unavailable` (the plugin's own
`unavailable`), `storage_plugin_timeout`.
