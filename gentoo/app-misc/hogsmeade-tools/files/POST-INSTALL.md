# HOGSMEADE after the install

1. `sudo hogsmeade-verify` : read-only checklist (GPU, audio, brightness, thermal, storage, services). Send the output to Claude.
2. Remote access: LAN ssh (key-only) `sudo hogsmeade-remote` prints the address; name hogsmeade.home.canutethegreat.com.
3. ComfyUI: http://127.0.0.1:8188 on the laptop, http://hogsmeade.home.canutethegreat.com:8188 from the home LAN (10.66.69.0/24) or the home WireGuard
   network (10.69.69.0/24); every other source is refused by `hogsmeade-firewall` (`nft list table inet hogsmeade`). Models: `sudo -u comfyui comfyui-fetch-models`  (manifest `/usr/share/comfyui/models-manifest.tsv`, every file sha256-checked).
4. Backups: the pool is a STRIPE (no redundancy). `hogsmeade-snapshots` runs hourly/daily/weekly (same drives: not a backup);
   `hogsmeade-backup` pushes an encrypted restic copy of /home and /data nightly (03:40, cron) to the SAME B2 repo the Windows job used (s3:.../ctg-backups/HOGSMEADE, tag `hogsmeade`) once /etc/hogsmeade/backup.env exists. The image carries NO restic key or password: the agent provisions them after first boot FROM crystalcave over SSH (stdin into root-only 0600 files; `hogsmeade-provision-restic.sh`). The pre-wipe extras are the snapshot tagged `pre-horcrux-wipe`.
   Restore: `set -a; . /etc/hogsmeade/backup.env; set +a; restic snapshots --tag hogsmeade; restic restore --tag hogsmeade latest:/home/.zfs/snapshot/hsm-backup --target /restore/home` (the same with `/data` for the data set; kept 30 days).
5. Replace a failed drive = reinstall + restore from restic. `hogsmeade-health` mails/notifies early (wear, spare, media errors, temperature).
6. Lighting/thermal: `awccctl status`, `awccctl profile quiet`, `awccctl light 0000FF shelf alienhead`, `awccctl light restore`.
7. Boot menu: any of the three drives' ESPs boots (`horcrux-boot-sync` keeps /boot copies equal after every kernel update).
