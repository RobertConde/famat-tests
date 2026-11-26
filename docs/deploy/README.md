# Deployment

The site runs on a self-hosted Debian 13 machine as five Podman containers under systemd (Quadlet). It is reached only through a Cloudflare Tunnel. Backups go nightly to a Hetzner Storage Box with restic.

```text
                          ┌─ famat-net (shared network namespace, 127.0.0.1 only) ─┐
Cloudflare ◄── tunnel ────┤ famat-cloudflared ──► famat-app :8080                   │
(famat.rconde.xyz,        │                         ├──► famat-postgres :5432       │
 tests.famat.org)         │                         └──► famat-garage   :3900 (S3)  │
                          └───────────────────────────────────────────────────────┘
host: famat-backup.timer → pg_dump + rclone copy of the bucket (+ Nextcloud, WeBWorK) → restic → Storage Box
      podman-auto-update.timer → new ghcr.io/robertconde/famat-tests:latest, rolled back if /healthz fails
```

## Runbooks

| Runbook | When |
|---|---|
| [server-setup.md](server-setup.md) | first-time setup on Debian 13: packages, units, secrets, start, auto-update, backups, monitors |
| [cloudflare.md](cloudflare.md) | the tunnel for `famat.rconde.xyz`; the Cloudflare for SaaS custom hostname `tests.famat.org` and the records FAMAT's DNS admin adds |
| [admin.md](admin.md) | `famat import` (rsyncing the inputs first) and `famat set-passcode` |
| [restore.md](restore.md) | restoring the database, one file, or everything onto a new server |
| [upgrading.md](upgrading.md) | app, Postgres, Garage, Debian, deployment file and secret changes |

## Files

| In the repository | On the server |
|---|---|
| `deploy/quadlet/*.container`, `*.volume` | `/etc/containers/systemd/` |
| `deploy/garage/garage.toml` | `/etc/famat/garage.toml` |
| `deploy/backup/backup.conf.example` | `/etc/famat/backup.conf` (copied once, then edited; mode 0600) |
| `deploy/backup/famat-backup.{service,timer}` | `/etc/systemd/system/` |
| `deploy/bin/famat`, `deploy/backup/famat-backup`, `deploy/garage/bootstrap.sh` | `/usr/local/sbin/famat`, `famat-backup`, `famat-garage-bootstrap` |
| `deploy/create-secrets.sh` | run from the checkout; makes the Podman secrets |
| `deploy/install.sh` | run from the checkout; copies all of the above |
| (not in git) | `/etc/famat/restic-password`, `/root/.ssh/storagebox`, the Podman secrets |

## Secrets

| Podman secret | Used by | Made by |
|---|---|---|
| `famat-db-password` | Postgres (`POSTGRES_PASSWORD`, first start only), app and CLI (`PGPASSWORD`) | `create-secrets.sh`, random |
| `famat-s3-access-key-id`, `famat-s3-secret-access-key` | app and CLI (`S3_*`), backup rclone, imported into Garage by `famat-garage-bootstrap` | `create-secrets.sh`, random |
| `famat-garage-rpc-secret` | Garage (`GARAGE_RPC_SECRET_FILE`) | `create-secrets.sh`, random |
| `famat-tunnel-token` | cloudflared (`TUNNEL_TOKEN`) | pasted from Cloudflare |

The Admin passcode is not a Podman secret: `famat set-passcode` stores its argon2id hash in Postgres, so it is backed up and restored with the database. The restic password is in `/etc/famat/restic-password` and in the owner's password manager. It is the one secret a disaster recovery cannot do without.

## Why these choices

- **No published ports.** The app trusts `CF-Connecting-IP` for its login rate limit and checks `Origin` against `Host`. So the tunnel must be the only way in, and it must keep the visitor's Host header.
- **A pause container, not a Quadlet `.pod`.** In a pod, `podman auto-update` restarts the pod's unit, which succeeds before the app is healthy, so a bad image is never rolled back (tried on Debian 13's Podman 5.4.2). Joining `famat-net`'s namespace gives the same shared loopback with one unit per container. See `deploy/quadlet/famat-net.container`.
- **Backups of dumps, not live files.** The Postgres data directory and Garage's metadata database are not consistent while running. `pg_dump` and an S3-level copy are. Restic deduplicates, so the nightly Garage copy costs only the new objects.
