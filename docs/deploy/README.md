# Deployment

The site runs on the self-hosted Debian 13 server as a Docker Compose project, `famat`, managed by systemd (`famat.service`). It has four containers. It is reached only through its own Cloudflare Tunnel, and its data is on the RAID at `/mnt/raid/famat/`. Backups go nightly to a Hetzner Storage Box with restic.

The server runs other things too: Nextcloud (probably in Docker), WeBWorK and MariaDB natively, and a host `cloudflared` service with another tunnel. The test bank leaves all of them alone. It has its own tunnel, its own `cloudflared` container and its own networks, and it publishes no port.

```text
                          ┌─ famat-backend (internal network: no route out, nothing published) ─┐
Cloudflare ◄── tunnel ────┤ famat-cloudflared ──► app:8080 ──► postgres:5432                       │
(famat.rconde.xyz,   ▲    │                                └──► garage:3900 (S3)                   │
 tests.famat.org)    │    └───────────────────────────────────────────────────────────────────────┘
                     └─ famat-egress (cloudflared's only way out)
data:  /mnt/raid/famat/{postgres,garage/meta,garage/data,backup-staging}
host:  famat.service          docker compose up / down, after docker.service and the /mnt/raid mount
       famat-update.timer     daily: new images for app, Postgres 17, cloudflared; rolled back if not healthy
       famat-backup.timer     nightly: pg_dump + rclone copy of the bucket (+ Nextcloud, WeBWorK) → restic → Storage Box
```

## Runbooks

| Runbook | When |
|---|---|
| [server-setup.md](server-setup.md) | first-time setup on this server: checks, data directories on the RAID, files, secrets, start, updates, backups, monitors |
| [cloudflare.md](cloudflare.md) | the test bank's own tunnel for `famat.rconde.xyz`; the Cloudflare for SaaS custom hostname `tests.famat.org` and the records FAMAT's DNS admin adds |
| [admin.md](admin.md) | `famat import` (rsyncing the inputs first) and `famat set-passcode` |
| [restore.md](restore.md) | restoring the database, one file, or everything onto empty data directories or a new server |
| [upgrading.md](upgrading.md) | the app, Postgres, Garage, Debian and Docker, deployment file changes, pinning a version, secrets |

## Files

| In the repository | On the server |
|---|---|
| `deploy/compose.yaml` | `/etc/famat/compose.yaml` |
| (not in git; optional) | `/etc/famat/compose.override.yaml`: local changes such as a pinned version, merged in by Compose |
| `deploy/garage/garage.toml` | `/etc/famat/garage.toml` |
| `deploy/systemd/famat.service`, `famat-update.{service,timer}`, `deploy/backup/famat-backup.{service,timer}` | `/etc/systemd/system/` |
| `deploy/bin/famat`, `deploy/bin/famat-update`, `deploy/backup/famat-backup`, `deploy/garage/bootstrap.sh` | `/usr/local/sbin/famat`, `famat-update`, `famat-backup`, `famat-garage-bootstrap` |
| `deploy/backup/backup.conf.example` | `/etc/famat/backup.conf` (copied once, then edited; mode 0600) |
| `deploy/create-secrets.sh` | run from the checkout; writes `/etc/famat/secrets/` |
| `deploy/install.sh` | run from the checkout; copies all of the above |
| (not in git) | `/etc/famat/secrets/*.env`, `/etc/famat/restic-password`, `/root/.ssh/storagebox` |

Docker Compose commands run from `/etc/famat` (`cd /etc/famat`), or with `--project-directory /etc/famat`, so that they find `compose.yaml` and the override.

## Secrets

Root-only env files in `/etc/famat/secrets/` (directory 0700, files 0600). The compose file reads them with `env_file`, so each container gets only its own.

| File | Holds | Used by | Made by |
|---|---|---|---|
| `db.env` | `POSTGRES_PASSWORD`, `PGPASSWORD` (the same value) | Postgres (first start only), the app and the CLI | `create-secrets.sh`, random |
| `s3.env` | `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`, and the same pair as `RCLONE_S3_*` | the app and the CLI, the backups' rclone; imported into Garage by `famat-garage-bootstrap` | `create-secrets.sh`, random |
| `garage.env` | `GARAGE_RPC_SECRET` | Garage | `create-secrets.sh`, random |
| `tunnel.env` | `TUNNEL_TOKEN` | cloudflared | pasted from Cloudflare |

The values also show in `docker inspect` and `docker compose config`, which only root (and the `docker` group, which is root-equivalent) can run. Don't paste their output anywhere.

The Admin passcode is not one of these. `famat set-passcode` stores its argon2id hash in Postgres, so it is backed up and restored with the database. The restic password is in `/etc/famat/restic-password` and in the owner's password manager. It is the one secret a disaster recovery cannot do without.

## Why these choices

- **No published ports.** The app trusts `CF-Connecting-IP` for its login rate limit and checks `Origin` against `Host`. So the tunnel must be the only way in, and it must keep the visitor's Host header. App, Postgres and Garage are only on `famat-backend`, an internal network, which Docker doesn't route to or from. (Tried: another machine with a route to that network through the server could not connect to the app.) Processes on the server itself can still reach the containers' addresses on the bridge, as with any local service.
- **Data on the RAID, guarded.** The root disk has about 200 GB, and the RAID has dropped all its disks once. If `/mnt/raid` isn't mounted, `famat.service` refuses to start (`RequiresMountsFor=`, `AssertPathIsMountPoint=`). The compose file's binds never create a missing directory, so even Docker restarting a container by itself, or a `docker compose up` by hand, fails instead of starting an empty database on the root disk.
- **`famat-update`, not Watchtower.** Docker has no `podman auto-update`, and Watchtower has no rollback. `famat-update` recreates one service at a time from the new image, waits for its health check, and puts the previous image back if it fails.
- **Backups of dumps, not live files.** The Postgres data directory and Garage's metadata database are not consistent while running. `pg_dump` and an S3-level copy are. Restic deduplicates, so the nightly Garage copy costs only the new objects.
