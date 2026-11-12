# Web UI / API login: system users (PAM)

Horcrux has **no user database and no `admin` account of its own**. You sign in to the web UI
(`https://<host>:8006`) and the API with the **Linux user** of the host, e.g. `canutethegreat`.
The password is checked by PAM (service `/etc/pam.d/horcrux`, package-owned, mirrors `system-auth`),
so local users and SSSD / FreeIPA / LDAP / AD users (wherever NSS + PAM resolve them) all work.
Horcrux never reads `/etc/shadow`.

## Who may do what

Rights come from the user's **system groups** (resolved through NSS):

| Group (default)           | Horcrux role                       | Config key                  |
|---------------------------|------------------------------------|-----------------------------|
| `horcrux-admin`, `wheel`  | `Administrator` (full rights)      | `[auth] admin_groups`       |
| (none)                    | `VMAdmin` (manage guests)          | `[auth] operator_groups`    |
| (none)                    | `Auditor` (read-only)              | `[auth] readonly_groups`    |

Users in none of the mapped groups are **denied**. The highest matching role wins. The role is
re-synced on every login (remove a user from the group and his next login is refused; his web
sessions are dropped). `root` (uid 0) cannot sign in to the web UI unless `[auth] allow_root = true`
(`HORCRUX_ALLOW_ROOT_LOGIN=1`). `HORCRUX_ADMIN_GROUPS=a,b` overrides `admin_groups`.

```toml
# /etc/horcrux/config.toml
[auth]
admin_groups    = ["horcrux-admin", "wheel"]
operator_groups = ["hx-operators"]
readonly_groups = ["hx-viewers"]
allow_root      = false
legacy_local_login = true     # old Horcrux-only accounts keep working this release (see Migration)
```

Add a user: `useradd -m -G horcrux-admin alice && passwd alice` (or `gpasswd -a alice horcrux-admin`).
The installer creates the user you type in the "Administrator account" step in `wheel` and
`horcrux-admin`: **log in with the user you created during install.**

## Clusters

Every node authenticates **locally** through its own PAM stack. The same user must therefore exist
on each node (same name; create it everywhere) or come from a shared directory (SSSD/FreeIPA/LDAP).
Group membership is evaluated on the node that handles the login. A session issued by one node
keeps working across the cluster UI exactly as before.

## Second factor

PAM is run with a password-only conversation. Per-user TOTP/WebAuthn in Horcrux (Access > Two-factor)
still applies after the PAM check. A PAM stack that itself prompts for an OTP (pam_google_authenticator
etc.) is not supported over the web: enable the Horcrux second factor instead.

## API tokens and automation

Tokens and service accounts are unchanged, but are owned by a system user (`<user>@pam`) rather than
a separate account store. Create them under that user; they stop working when it is deleted.

## Brute-force protection and audit

* per-IP rate limit on the credential routes (existing),
* per-user API backoff: 5 failures in 10 minutes block that name for 60 s without asking PAM,
* `pam_faillock` (in `system-auth` on Gentoo `pambase[passwdqc/faillock]`) counts every attempt PAM sees,
* every failure takes the same >= 4 s and returns the same generic 401, whether the password was wrong,
  the user does not exist, is not in a mapped group, or is root (the reason is only in the audit log),
* successes and failures go to the audit log.

## Migration from the old built-in `admin`

* Upgrade changes nothing for existing sessions. The old Horcrux-only accounts are kept
  **read-only for one release**: they can still sign in (pick realm `local` or type the old name) but no new local
  account can be created.
* The first time a PAM administrator signs in, the legacy `admin` account is **disabled** automatically.
* `/var/lib/horcrux/initial-admin-password` (and `/etc/horcrux/admin-password`) are deleted on start.
* `legacy_local_login = false` turns the old accounts off earlier; the next release removes them.
* Do not lock yourself out: before upgrading make sure your Linux user is in `wheel` or `horcrux-admin`
  (`id <user>`), then log in once with it.
