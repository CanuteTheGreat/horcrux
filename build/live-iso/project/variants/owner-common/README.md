# owner-common: the owner's personal apps, shared by his own machines (NOT a public image)

Horcrux is a headless hypervisor/server OS with an optional desktop (`docs/ISO-INSTALL.md`, "Layers"). Public images (server, gnome-light, ...) carry NO
personal application. The owner's own picks are added by the machine layers (`gringotts`, `hogshead`, `hogsmeade`, the Surface/madammalkins profile) from here:

- `packages.txt`   apps every one of his general-purpose machines has (inventories: ctg-infrastructure `hosts/<host>/inventory-*.md`). A machine adds its
                   own extras in its own `packages.txt` / Dockerfile (hogshead: keepalived, sshpass, rustup; hogsmeade: Steam ONLY there, never here).
- `portage/`       keywords + USE those apps need (Brave and rustup are ~amd64; Nextcloud without the Dolphin plugin)
- `overlay/`       files the apps' backup needs: `/etc/ctg-backup/required` (what a restore must hold so the owner never signs in again)

A machine Dockerfile does `COPY project/variants/owner-common/portage/ /etc/portage/`, `COPY project/variants/owner-common/overlay/ /` and emerges
`packages.txt`. `project/public-forbidden.txt` lists what must never appear in a public image; the core/desktop Dockerfiles and the QEMU gates enforce it.
