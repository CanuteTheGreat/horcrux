# Storage plugin API changelog

Versions of the storage plugin protocol (the JSON a plugin exchanges with the
Horcrux manager over stdio; see [storage-plugins.md](storage-plugins.md)). A
plugin reports the version it implements in `describe.api_version`; the
manager lists the versions it supports in `GET /api/storage/plugins`
(`supported_api_versions`) and refuses plugins outside that list with reason
`storage_plugin_incompatible`.

Rules for changes:

- Adding an optional request field, an optional result field, a schema field
  property with a default, a capability flag or an error hint does NOT bump
  the version: plugins ignore unknown request fields, and the manager treats a
  missing result field as its default. Such additions are listed under the
  current version with the Horcrux release that added them.
- Removing or renaming anything, changing a field's meaning or type, or adding
  an operation a plugin MUST implement bumps the version. The manager keeps
  speaking the previous version for at least two releases after a bump.
- A new optional operation does not bump the version: an older plugin answers
  `unsupported` (the SDK does that for every operation it does not override).

## Version 1 (Horcrux 0.1.x)

First version.

- Operations: `describe`, `activate`, `deactivate`, `status`, `list_volumes`,
  `create_volume`, `delete_volume`, `resize_volume`, `list_snapshots`,
  `create_snapshot`, `delete_snapshot`, `rollback_snapshot`, `volume_path`,
  `map_volume`, `unmap_volume`.
- Request: `api_version`, `op`, `pool {id, name, config, secrets, state_dir}`,
  `args {name, size_bytes, format, snapshot}`.
- Response: `ok`, `result`, `error {code, message, hint}` with codes `invalid`,
  `not_found`, `exists`, `unsupported`, `unavailable`, `failed`.
- Schema field types `string`, `int`, `bool`, `enum`, `secret`, `path`; field
  properties `key`, `type`, `label`, `hint`, `default`, `min`, `max`, `pattern`,
  `required`, `depends_on {key, value}`, `options`, `placeholder`.
- Capabilities: `content`, `snapshots`, `clone`, `resize`, `shared`,
  `vm_disk_dir`, `formats`. `clone` is reserved: no clone operation exists in
  version 1.
