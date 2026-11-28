# Service accounts and API keys

Host jobs, CI and scripts should not log in with a password (and cannot answer a
second factor). Give each one a **service account**: an ordinary Horcrux user
that never logs in interactively, holds only the privileges its job needs, and
authenticates with an **API key**.

A key (`hx_...`) is sent with every request, either way:

```
X-API-Key: hx_...
Authorization: Bearer hx_...
```

It acts with its owner's rights (nothing more), is checked against the live
user on every request (disable or delete the user and the key stops at once),
may have an expiry, and is stored only as an Argon2 hash: the clear key is
shown once, when it is created.

## Set one up

All of this is also possible in the UI (Access control: Users, Roles,
Permissions, and the user's API keys page).

1. A role with just what the job does (privileges are listed by
   `GET /api/privileges`):

   ```sh
   curl -sk -H "Authorization: Bearer $ADMIN_TOKEN" -H 'Content-Type: application/json' \
     -d '{"name":"HostJobs","privileges":["VM.Audit","VM.PowerMgmt","VM.Console","VM.Snapshot","Sys.Audit","Sys.Modify"]}' \
     https://node:8006/api/roles
   ```

2. The user. Service accounts are host (PAM) accounts: a no-login system
   user, registered in the `pam` realm. The `local` realm (a password stored
   in Horcrux) is refused on a stock install with 403 "Horcrux has no
   accounts of its own", so do not use it here. The key is the credential,
   so the account needs no password:

   ```sh
   useradd --system --no-create-home --shell /sbin/nologin svc-hostjobs
   curl -sk ... -d '{"username":"svc-hostjobs","realm":"pam"}' \
     https://node:8006/api/users
   # the account is now named svc-hostjobs@pam
   ```

3. The grant, on the narrowest path that works (`/vms` for every VM, `/vms/100`
   for one, `/` for node-wide settings such as backup jobs):

   ```sh
   curl -sk ... -d '{"path":"/vms","subject_type":"user","subject":"svc-hostjobs@pam","role":"HostJobs"}' \
     https://node:8006/api/acl
   ```

4. The key (optional `expires_days`):

   ```sh
   curl -sk ... -d '{"name":"host jobs","expires_days":365}' \
     https://node:8006/api/users/svc-hostjobs@pam/api-keys
   # {"id":"...","key":"hx_...","user":"svc-hostjobs@pam","expires_at":...}
   ```

   Put the key in a root-only file (mode 0600) on the host that needs it.

Rotate by creating a second key, switching the job over, then
`DELETE /api/users/svc-hostjobs@pam/api-keys/<id>` (or `.../toggle` to disable it
first). Revoke everything for an account by disabling the user.

## Which privileges

| Job does | Privilege (path) |
|---|---|
| list/inspect VMs and containers | `VM.Audit` (`/vms`) |
| start, stop, reboot, hard reset | `VM.PowerMgmt` (`/vms/<id>`) |
| run a command in a VM (`guest-exec`) | `VM.Console` (`/vms/<id>`): console-equivalent |
| snapshots | `VM.Snapshot` |
| backup jobs (`/api/backup-jobs`, declarative `backup_jobs`) | `Sys.Audit` to read, `Sys.Modify` (`/`) to change |
| `horcrux apply` (declarative config) | the privileges of every kind it changes |

Administrators can mint a key for any user; other users can mint keys only for
themselves, and nobody can mint one for an administrator without administrator
rights.

## From the CLI

```sh
export HORCRUX_SERVER=https://node:8006
export HORCRUX_API_KEY_FILE=/etc/horcrux/svc-hostjobs.key   # or HORCRUX_API_KEY / --api-key
horcrux vm list
horcrux vm reset 100
horcrux vm exec 100 -t 30 -- /bin/sh -c 'uptime'
```

`--api-key-file` keeps the key out of the process list and the environment.
A key given this way wins over a stored login token.

## Audit

Calls made with a key are attributed to the service account in the access log
and in the audit log (`GuestExec`, `VmRestarted`, backup and configuration
events). Use one account per job so the trail says which job did what.
