# Access control and HTTPS

Horcrux follows the Proxmox VE model: users authenticate in a *realm*, and
what they may do comes from *ACL grants* (a role on a path), not from the
account itself. Everything lives in the database and is enforced on every
API route.

## First start

- There is no default password. On the first start with an empty database
  Horcrux creates the user `admin` with an `Administrator` grant on `/`.
  The password comes from `ADMIN_PASSWORD`, or is generated and written to
  `/var/lib/horcrux/initial-admin-password` (mode 0600; the login page shows
  the path while the file exists). It is deleted when the admin password is
  changed.
- Public self-registration (`POST /api/auth/register`) is **off**. Turn it on
  only deliberately (`HORCRUX_ALLOW_REGISTRATION=true` or
  `[auth] allow_registration = true`); such accounts start with no
  permissions at all.
- Token signing secret: `JWT_SECRET`, or generated once into
  `/var/lib/horcrux/jwt.secret`. All nodes of a cluster need the same
  secret (set `JWT_SECRET` or copy that file).

## Users, groups, roles, ACL

- Users and groups: *Administration > Users*, *Permissions & Groups*
  (`/api/users`, `/api/groups`).
- Built-in roles (read-only): `Administrator`, `NoAccess`, `Auditor`,
  `VMAdmin`, `VMUser`, `DatastoreAdmin`, `DatastoreUser`, `UserAdmin`,
  `SysAdmin`, `SDNAdmin`. Custom roles are any set of privileges
  (`VM.Audit`, `VM.PowerMgmt`, `Datastore.Allocate`, `Sys.Modify`, ... -
  `GET /api/privileges`).
- A grant is `role` on `path` for a user or group (`POST /api/acl`). Paths:
  `/`, `/vms`, `/vms/<id>`, `/containers[/<id>]`, `/storage[/<pool>]`,
  `/sdn`, `/nodes`, `/cluster`, `/system`, `/access`, `/k8s`, `/nas`.
  With *propagate* a grant covers everything below. The most specific path
  with a grant wins and a user's own grant beats a group grant there, so
  `NoAccess` on `/vms/secret` revokes a broader grant for that one VM.
- Lists (`GET /api/vms`, `/api/containers`, `/api/storage/pools`) are
  filtered to what the caller may see.
- Changing ACLs needs `Permissions.Modify` on the ACL's path. A user with
  `User.Modify` cannot change an administrator's account, password or API
  keys, and the last administrator can't be disabled or deleted.
- Logging out, disabling or deleting an account revokes its tokens
  immediately (tokens are bound to a database session). API keys act with
  their owner's rights.

**For contributors:** every route is checked by `middleware/rbac.rs` using
`auth/privileges.rs::requirement()`. A new top-level `/api/<area>` must get
a rule there (`every_registered_route_is_mapped` fails otherwise); until
then it is administrators-only.

## Realms

*Administration > Authentication Realms* (`/api/access/realms`). Users of
a realm log in as `name@realm` (or pick the realm on the login page); their
account is created on first login and added to the realm's default group.

- `ldap`: `{"server", "port", "mode": "ldap|starttls|ldaps", "verify_tls",
  "ca_cert", "base_dn", "user_attr": "uid", "filter", "bind_dn",
  "bind_password"}` - search-then-bind.
- `ad`: `{"server", "mode", "domain", "base_dn"}` - binds as `user@domain`.
- `openid`: `{"issuer_url", "client_id", "client_secret", "username_claim",
  "redirect_uri"}`; register `https://<host>:8006/api/auth/oidc/callback`
  at the provider. Authorization code flow with PKCE, state and nonce.
- `pam`: host accounts through the `horcrux` PAM service (USE=pam; the
  ebuild installs `/etc/pam.d/horcrux`).

Bind passwords and client secrets are never returned by the API.

## Second factors

*Account Security* (`/auth/mfa`): authenticator apps (TOTP, RFC 6238),
security keys and passkeys (WebAuthn, ES256/RS256), and ten one-time
recovery codes created with the first factor. A second factor can be
required per user, per realm, or for everyone (policy on the same page);
an account that must have one but doesn't gets a restricted login that can
only enroll. OpenID Connect realms leave second factors to the provider.

## HTTPS

The API and web UI are served over HTTPS on port 8006. On first start the
node creates its own CA (`/etc/horcrux/ssl/ca.pem`, falling back to
`/var/lib/horcrux/ssl` when `/etc` isn't writable) and issues the server
certificate from it. Import the CA once (`https://<host>:8006/api/tls/ca.pem`,
also linked on *Administration > Certificates*) and browsers trust the node.
Plain `http://host:8006/...` is redirected; `/api/health` answers on plain
HTTP too for probes.

- Upload your own certificate, or re-issue from the local CA with extra
  names, on the Certificates page (switched without a restart).
- ACME (Let's Encrypt or any RFC 8555 CA) with HTTP-01: set
  `HORCRUX_HTTP_REDIRECT_PORT=80` (or forward port 80), enter domains and
  accept the CA's terms, *Order certificate now*. Renewed automatically 30
  days before expiry.
- `HORCRUX_TLS_ENABLED=false` only behind a TLS-terminating proxy.
- CLI: `horcrux --server https://host:8006 --ca-cert ca.pem ...` (the local
  node's CA is used automatically when readable).

## Tests

- `horcrux-api/tests/auth_e2e_tests.rs` (CI Test job, live server): RBAC on
  every registered route, DB-backed users, token revocation, TOTP,
  WebAuthn, recovery codes, enforcement, escalation guards, filtering,
  HTTPS redirect and certificate hot swap.
- `horcrux-api/tests/auth_realms_e2e.sh` (CI): OpenLDAP, Dex (OpenID
  Connect), PAM and Pebble (ACME HTTP-01) in containers.
- `horcrux-ui/e2e/ui_e2e.py`: login, TOTP and security-key (virtual
  authenticator) enrollment and login in a real browser that trusts the
  local CA.
