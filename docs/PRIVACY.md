# Privacy: what Horcrux sends off the machine

Short version: Horcrux does not collect or send usage data, crash reports or telemetry. It has no
account, no licence server and no "phone home". The only traffic it makes by itself is fetching
software updates, and everything else leaves the machine only when you configure it to.

This page was written from the source code, not from intentions: it lists every outbound
connection the code makes that we know of. If you find one that is missing here, that is a bug in
this page; please report it as described in [SECURITY.md](../SECURITY.md).

## Telemetry and crash reports

None. The source contains no analytics, usage statistics or crash-reporting code (no Sentry-style
client, no counters sent to a project server). Horcrux keeps its logs, metrics and audit log on the
node.

The one exporter that exists is OpenTelemetry metrics export (Monitoring > Observability).
It is **off by default** (`enabled: false`) and, once you switch it on, sends node metrics only to
the collector address you type in. Nothing is exported to the project.

## What the node fetches on its own: software updates

Updates are on by default (`sys-apps/horcrux-autoupdate`, see [UPDATE-POLICY.md](UPDATE-POLICY.md)).
Once a day it runs `emaint sync -a` and `emerge -uDN @world`, so the node contacts:

- the Horcrux package overlay, fetched with git from `github.com` (the repository is configured in
  `/etc/portage/repos.conf/horcrux.conf`, shipped by `sys-apps/horcrux-repo-config`);
- the Gentoo package tree and source or binary package mirrors, as configured in Portage. Horcrux
  images use Gentoo's official binary package host (`distfiles.gentoo.org`) and Gentoo's default
  mirrors for sources.

These servers see the node's IP address and the package names it asks for, as with any Gentoo
system. They are not given your VM, container or user data. Turn the schedule off with `ENABLED=no`
in `/etc/horcrux/autoupdate.conf`; every source above can be changed in Portage's configuration.

Horcrux does not check for new versions through any other channel and sends no version or
inventory information about the node.

## What your browser loads from other hosts

The web UI itself loads everything from the node. Two pages are the exception, and both load
public JavaScript from a content delivery network (so that CDN sees your browser's IP address and
which page you opened):

- the noVNC graphical console page of a VM loads the noVNC library from `cdn.jsdelivr.net`;
- the interactive API documentation page loads Swagger UI from `unpkg.com`.

If the node's browsers cannot reach the internet, those two pages do not work; the rest of the UI is unaffected.

## Connections you configure

These happen only after an administrator sets them up, and go only to the address that was entered:

| Feature | Goes to |
|---|---|
| Certificates (ACME) | the certificate authority you pick (Let's Encrypt's address is only the pre-filled default) |
| Notification channels (email, Slack, Teams, Discord, webhooks) | the endpoint you enter |
| ISO and cloud-image downloads, OVF/OVA import by URL | the URL you enter |
| Container images | the registry or image server of your choice; LXC templates default to `images.linuxcontainers.org` |
| Helm repositories | the repository you add |
| Kubernetes cluster provisioning | downloads the Kubernetes or k3s installer and network plugins from their upstream hosts (`get.k3s.io`, `pkgs.k8s.io`, `github.com`) onto the machines you are provisioning |
| Backup and storage targets (S3, NFS, Ceph, iSCSI) | the target you enter |
| Cluster, HA and migration traffic | the other nodes of your own cluster |
| Authentication realms (LDAP, OIDC) | the identity server you enter |

## This website (horcrux.errant.solutions)

- The pages contain no JavaScript (the site's content security policy forbids scripts), no
  analytics, no advertising and no third-party fonts or images: everything is served from the site
  itself.
- The site's own server sets no cookies.
- Visitors reach it through Cloudflare, which terminates the TLS connection.
- ISO download links redirect to the project's public GitHub releases, so GitHub serves the file
  and sees your IP address.
- Linked source and documentation pages are on GitHub and follow GitHub's privacy policy.

## Changes

A change to what Horcrux sends off the machine is a change to this page in the same commit; the
history is in the repository.
