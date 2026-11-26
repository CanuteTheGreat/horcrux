# Horcrux threat model

Status: first written version, 2026-11-22, by the maintainers. It is the written threat model that roadmap criterion 7 asks for. It is **not** an independent review: that is still open. Every control named here is documented elsewhere in `docs/` and (where noted) covered by a CI test; a control with no test is marked.

## What is protected

| Asset | Where it lives |
|---|---|
| Guest disks, memory state, backups | storage pools, `/var/lib/horcrux`, backup targets |
| Control of the host (the API can start processes, mount storage, change networking) | the API on port 8006 |
| Credentials: user database, API keys, realm bind secrets, the token signing secret | SQLite database, `/var/lib/horcrux/jwt.secret` |
| Cluster membership and quorum | corosync/cluster state, shared token signing secret |
| Release integrity (ISOs, tarball, packages) | signed artifacts, see [SECURITY.md](../SECURITY.md) |

## Actors

1. **Unauthenticated network attacker** who can reach port 8006 (LAN, or the internet if the owner forwards it).
2. **Authenticated low-privilege user** (VMUser, DatastoreUser, an OpenID-group user) trying to exceed their grant.
3. **Guest workload** (a VM or container run by a tenant) trying to reach the host, other guests or the management network.
4. **Compromised or rogue cluster node.**
5. **Supply-chain attacker** (binhost, ISO download, update channel).
6. **Someone with physical access** to a node.

## Threats and controls

| # | Threat | Control | Test |
|---|---|---|---|
| T1 | Anonymous access to the API | No default or generated password and no built-in admin. Login is by PAM, LDAP/AD or OpenID. Self-registration is off by default and new accounts start with no permissions. | `auth_e2e_tests.rs` |
| T2 | A user acts beyond their role | RBAC on every route (`middleware/rbac.rs`); a route without a rule is administrators-only. Most specific ACL path wins, `NoAccess` revokes. List endpoints are filtered to what the caller may see. | `every_registered_route_is_mapped`, `rbac_matrix_tests.rs` |
| T3 | Privilege escalation through user management | `User.Modify` cannot change an administrator; the last administrator cannot be disabled or deleted; ACL changes need `Permissions.Modify` on that path. | `auth_e2e_tests.rs` (escalation guards) |
| T4 | Stolen or leaked token | Tokens are bound to a database session; logout, disable and delete revoke at once. API keys act with their owner's rights only. | `auth_e2e_tests.rs` (token revocation) |
| T5 | Password guessing or phishing | Second factors (TOTP, WebAuthn security keys and passkeys, recovery codes), enforceable per user, realm or globally. Auth routes are rate limited. | `auth_e2e_tests.rs`, UI E2E |
| T6 | Traffic interception or tampering | HTTPS by default on first start from a node-local CA with an Authority Key Identifier; plain HTTP is redirected (except `/api/health`). Uploaded certificates and ACME are supported. | `auth_e2e_tests.rs` (redirect, certificate swap) |
| T7 | OpenID login abuse | Authorization code flow with PKCE, state and nonce. | `auth_realms_e2e.sh` (Dex) |
| T8 | Hostile names or paths in API input reaching a shell, `ovs-vsctl`, `nft`, file paths | Names are validated before they are used in commands; hostile names are refused (for example the Open vSwitch and storage tests). Whether every route spawns commands without a shell has not been audited route by route. | `ovs_e2e.sh`, storage tests. No single test covers every route: **gap** |
| T9 | Guest-to-host escape | QEMU/KVM and LXC isolation as shipped by Gentoo; Horcrux adds no privileges to guests. Device passthrough is an explicit opt-in per guest. Hardening of QEMU (seccomp sandbox, per-guest user) is **not** done: gap. | none |
| T10 | Guest-to-guest or guest-to-management network access | Per-VM nftables firewall chains. Isolation is only as good as the rules the owner writes; there is no default-deny between guests. | firewall tests |
| T11 | Rogue node joins the cluster | Joining needs the shared token signing secret and the cluster join flow; all nodes trust each other fully once joined. A compromised node is a compromised cluster: **accepted risk**, mitigated only by fencing and removal. | none |
| T12 | Tampered release or update | Detached OpenPGP signatures on every published file, CycloneDX SBOM, CI refuses to publish what it cannot sign and verify; signed binhost packages. | publish step in CI |
| T13 | Vulnerable dependency | `cargo audit` in CI. | `ci.yml` Security Audit |
| T14 | Physical access to a node | Out of scope except that full-disk encryption is the owner's choice; UEFI Secure Boot is supported (shim + our signed GRUB, kernel and modules; one-time key enrollment, [SECURE-BOOT.md](SECURE-BOOT.md)), so a tampered boot chain does not boot when Secure Boot is on. | none |

## Out of scope

Denial of service by an authenticated administrator; malicious hardware or firmware; a guest owner attacking their own guest; availability under a volumetric network attack.

## Accepted risks and open items

- Administrators are fully trusted: the API can run host-level actions by design.
- Cluster nodes trust each other completely (T11).
- No QEMU sandboxing profile (T9) and no complete input-validation test across all routes (T8): both are candidates for the next hardening item.
- No independent review yet. Criterion 7 stays "not met" until one is done and every critical and high finding is fixed or accepted in writing.
