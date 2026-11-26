# Upgrading

`docker compose` commands here run in `/opt/stacks/famat` (`cd /opt/stacks/famat`).

| What | How it updates |
|---|---|
| The app | automatically: CI pushes `ghcr.io/robertconde/famat-tests:latest` from every green `main`, and `famat-update.timer` pulls it daily |
| Postgres 17 minor releases | automatically (`postgres:17`) |
| cloudflared | automatically (`cloudflared:latest`) |
| Postgres major versions, Garage, rclone | by hand, below |
| The deployment files (`deploy/`) | by hand: pull and re-run `install.sh` |
| Debian and Docker | `apt` |

## The automatic updates

### How an update and a rollback work

Every day at about 05:00, `famat-update` (run by `famat-update.timer`) does this for Postgres, the app and cloudflared, one at a time:

1. It pulls the service's image, as `compose.yaml` and any `compose.override.yaml` name it. If the image ID is the one the container already runs, that service is up to date.
2. Otherwise it tags the running image `famat-previous/<service>` and recreates only that container (`docker compose up --no-deps`). Nothing else restarts, so for the app the site is down for the restart only (seconds).
3. It waits up to 5 minutes for the container's health check: `/healthz` for the app, `pg_isready` for Postgres, `cloudflared tunnel ready` for cloudflared.
   - If it passes, the update is done.
   - If not, it tags the previous image back as the service's image, recreates the container from it, and logs `ROLLED BACK`. This was tried locally with an app image that never answers: it was back on the previous image 5 minutes later, healthy, and Postgres, Garage and cloudflared hadn't restarted.
4. A rolled-back image's ID is kept in `/var/lib/famat-update/<service>.bad`, and later runs don't try it again. The next new image is tried as usual. To retry a rolled-back image anyway, delete that file.

Every outcome is in `journalctl -u famat-update`. A run with a rollback or a failed pull exits non-zero, so `systemctl --failed` lists `famat-update.service` until the next good run.

**Migrations are not rolled back.** The app migrates the database when it starts, before `/healthz` answers. If a release migrated and then failed its health check, the previous image runs on the migrated schema. Keep migrations additive (new tables or columns, never renames or drops in the same release), or restore the database ([restore.md](restore.md)).

### By hand

```sh
famat-update                       # all three, now, instead of waiting for the timer
famat-update app                   # just the app
journalctl -u famat-update -n 50
docker compose ps
docker images famat-previous/*     # the image each service had before its last update
```

To go back to the previous app image by hand:

```sh
docker tag famat-previous/app ghcr.io/robertconde/famat-tests:latest
docker compose up -d --no-deps app
```

The next `famat-update` would pull `:latest` again, so also pin the version, below.

### Pinning a version

To hold back a bad release, or to stay on one version, pin it in `/opt/stacks/famat/compose.override.yaml`. Compose merges that file into `compose.yaml` on its own, and `install.sh` never touches it. CI tags every image `sha-<first 12 of the commit>`:

```sh
cat > /opt/stacks/famat/compose.override.yaml <<'EOF'
services:
  app:
    image: ghcr.io/robertconde/famat-tests:sha-0123456789ab
EOF
docker compose pull app
docker compose up -d --no-deps app
```

`famat-update` follows the override too, and a `sha-` tag never changes, so the app stays there. To follow `:latest` again, delete the file (or its `app:` part) and run `docker compose up -d --no-deps app`.

## The deployment files

```sh
cd /opt/famat-tests && git pull
deploy/install.sh                  # copies the compose file, units and scripts; keeps secrets, the override and backup.conf
cd /opt/stacks/famat && docker compose up -d     # recreates only the containers whose configuration changed
```

If `famat.service` itself changed, `systemctl restart famat` (it restarts everything). For a backup or update script change, nothing needs restarting. If `backup.conf.example` gained settings, copy the new ones into `/opt/stacks/famat/backup.conf` by hand.

## Postgres, a major version (e.g. 17 → 18)

The data directory format changes, so dump and restore:

1. `famat-backup part famat`. This takes a fresh dump at `/mnt/raid/famat/backup-staging/famat/famat.pgdump` and backs it up.
2. Stop the app and Postgres, and set the old data directory aside:

   ```sh
   cd /opt/stacks/famat
   docker compose stop cloudflared app postgres
   mv /mnt/raid/famat/postgres /mnt/raid/famat/postgres-17      # kept until the new one checks out
   install -d -m 0700 /mnt/raid/famat/postgres
   ```

3. In `deploy/compose.yaml` (in git, through a PR), change the `postgres` service:
   - `image: docker.io/library/postgres:18`
   - From 18, the image keeps its data under `/var/lib/postgresql/18/docker`. So change the bind's `target` to `/var/lib/postgresql`.
4. Install it and load the dump:

   ```sh
   cd /opt/famat-tests && git pull && deploy/install.sh
   cd /opt/stacks/famat
   docker compose pull postgres
   docker compose up -d --wait postgres
   docker exec -i famat-postgres pg_restore -U famat -d famat --no-owner --exit-on-error \
     < /mnt/raid/famat/backup-staging/famat/famat.pgdump
   docker compose up -d
   ```

5. Check the site. Then `rm -rf /mnt/raid/famat/postgres-17`.

## Garage

Read the release notes from your version up to the target version first: <https://git.deuxfleurs.fr/Deuxfleurs/garage/releases>. Major versions can need a migration step (`garage migrate`).

1. `famat-backup part famat`, so a fresh copy of the bucket is in the backups.
2. Change the `garage` service's `image: docker.io/dxflrs/garage:vX.Y.Z` in `deploy/compose.yaml`.
3. Run:

   ```sh
   cd /opt/famat-tests && git pull && deploy/install.sh
   cd /opt/stacks/famat
   docker compose pull garage
   docker compose up -d --wait garage
   docker exec famat-garage /garage status
   ```

Also update `GARAGE_IMAGE` in `scripts/dev-up.sh`, so development and CI test against the same version.

rclone (`rclone/rclone:1`, used only by the backups) gets new 1.x releases with `docker compose pull rclone`.

## Debian and Docker

```sh
apt update && apt full-upgrade
reboot                               # everything starts again on boot
```

An upgrade of `docker-ce` restarts `docker.service`. `famat.service` requires it, so systemd stops and starts the test bank with it; this was tried with `systemctl restart docker`. After a reboot, `famat.service` waits for `/mnt/raid`. If the RAID didn't come back, it stays down: see [server-setup.md](server-setup.md), step 5.

After a Docker or Compose upgrade, check that Compose still accepts the file: `docker compose config --quiet`.

## Changing a secret

The secrets are env files in `/opt/stacks/famat/secrets/`. Compose recreates a container when its env file changes, at the next `docker compose up -d`.

- **Tunnel token:** see [server-setup.md](server-setup.md), step 4.
- **Database password.** Postgres only reads it when the data directory is first created, so change it in both places:

  ```sh
  new=$(openssl rand -hex 24)
  docker exec famat-postgres psql -U famat -d postgres -c "alter user famat password '$new'"
  printf 'POSTGRES_PASSWORD=%s\nPGPASSWORD=%s\n' "$new" "$new" > /opt/stacks/famat/secrets/db.env
  cd /opt/stacks/famat && docker compose up -d --no-deps postgres app
  ```

- **Garage key:**
  1. Note the old key ID (`S3_ACCESS_KEY_ID` in `s3.env`), delete `/opt/stacks/famat/secrets/s3.env`, and re-run `deploy/create-secrets.sh`.
  2. Run `famat-garage-bootstrap`, which imports the new key.
  3. `cd /opt/stacks/famat && docker compose up -d --no-deps app`.
  4. Delete the old key: `docker exec famat-garage /garage key delete --yes <old key id>`.
- **Admin passcode:** `famat set-passcode` ([admin.md](admin.md)).
