# Upgrading

| What | How it updates |
|---|---|
| The app | automatically: CI pushes `ghcr.io/robertconde/famat-tests:latest` from every green `main`, and `podman-auto-update.timer` pulls it daily |
| Postgres 17 minor releases | automatically (`postgres:17`) |
| cloudflared | automatically (`cloudflared:latest`) |
| Postgres major versions, Garage | by hand, below |
| The deployment files (`deploy/`) | by hand: pull and re-run `install.sh` |
| Debian and Podman | `apt` |

## The app

### How an update and a rollback work

1. `podman auto-update` sees a new digest for `:latest`, pulls it, and restarts `famat-app.service`.
2. The unit counts as started only once `/healthz` answers 200 (`Notify=healthy`).
   - If the new image never gets there, the start fails: the health check runs out its 120 s start period and 3 retries, and the container is killed.
   - Auto-update then re-tags the previous image and restarts the unit with it. Its report says `rolled back`.
   - This was tried with Podman 5.4.2 on Debian 13, with an image that never answers. It was back on the previous image about 2.5 minutes later.
3. Only the app restarts. Postgres, Garage and cloudflared keep running, so the site is down for the restart only (seconds).

The units are not Quadlet pod members, for this reason: auto-update would restart a pod member through the pod's unit, which reports success before the app is healthy, so it would never roll back. See `deploy/quadlet/famat-net.container`.

**Migrations are not rolled back.** The app migrates the database when it starts, before `/healthz` answers. If a release migrated and then failed its health check, the previous image runs on the migrated schema. Keep migrations additive (new tables or columns, never renames or drops in the same release), or restore the database ([restore.md](restore.md)).

### By hand

```sh
podman auto-update --dry-run        # what would update ("pending")
podman auto-update                  # do it now, instead of waiting for the timer
podman ps --format '{{.Names}} {{.Image}} {{.Status}}'
journalctl -u famat-app -n 50
```

To pin a version (for example to hold back a bad release), use a drop-in. CI tags every image `sha-<first 12 of the commit>`:

```sh
mkdir -p /etc/containers/systemd/famat-app.container.d
printf '[Container]\nImage=ghcr.io/robertconde/famat-tests:sha-0123456789ab\n' \
  > /etc/containers/systemd/famat-app.container.d/pin.conf
systemctl daemon-reload && systemctl restart famat-app
```

Delete the drop-in, then run `daemon-reload` and restart, to follow `:latest` again.

## The deployment files

```sh
cd /opt/famat-tests && git pull
deploy/install.sh                   # copies units and scripts; keeps backup.conf and secrets
systemctl restart famat-net         # restarts everything, for unit changes
```

For a change to one unit, restart only that unit. For a backup script change, nothing needs restarting. If `backup.conf.example` gained settings, copy the new ones into `/etc/famat/backup.conf` by hand.

## Postgres, a major version (e.g. 17 → 18)

The data directory format changes, so dump and restore:

1. `famat-backup part famat`. This takes a fresh dump at `/var/backups/famat-staging/famat/famat.pgdump` and backs it up.
2. Stop and set aside the old volume:

   ```sh
   systemctl stop famat-cloudflared famat-app famat-postgres
   podman volume rename famat-pg famat-pg-17     # kept until the new one checks out
   ```

3. Edit `deploy/quadlet/famat-postgres.container`:
   - `Image=docker.io/library/postgres:18`
   - From 18, the image keeps its data under `/var/lib/postgresql/18/docker`. So mount the volume at `/var/lib/postgresql` instead: `Volume=famat-pg.volume:/var/lib/postgresql`.
4. Install and load the dump:

   ```sh
   deploy/install.sh
   systemctl start famat-postgres
   podman exec -i famat-postgres pg_restore -U famat -d famat --no-owner --exit-on-error \
     < /var/backups/famat-staging/famat/famat.pgdump
   systemctl start famat-app famat-cloudflared
   ```

5. Check the site. Then `podman volume rm famat-pg-17`.

## Garage

Read the release notes from your version up to the target version first: <https://git.deuxfleurs.fr/Deuxfleurs/garage/releases>. Major versions can need a migration step (`garage migrate`).

1. `famat-backup part famat`, so a fresh copy of the bucket is in the backups.
2. Change `Image=docker.io/dxflrs/garage:vX.Y.Z` in `deploy/quadlet/famat-garage.container`.
3. Run:

   ```sh
   deploy/install.sh
   podman pull docker.io/dxflrs/garage:vX.Y.Z
   systemctl restart famat-garage      # the app is restarted with it (Requires=)
   podman exec famat-garage /garage status
   ```

Also update `GARAGE_IMAGE` in `scripts/dev-up.sh`, so development and CI test against the same version.

## Debian and Podman

```sh
apt update && apt full-upgrade
reboot                               # everything starts again on boot
```

After a Podman upgrade, check that Quadlet still accepts the units, before you reboot:

```sh
/usr/libexec/podman/quadlet -dryrun > /dev/null
```

## Changing a secret

- **Tunnel token:** see [server-setup.md](server-setup.md), step 3.
- **Database password.** Postgres only reads the secret when the volume is first created, so change it in both places:

  ```sh
  new=$(openssl rand -hex 24)
  podman exec famat-postgres psql -U famat -d postgres -c "alter user famat password '$new'"
  printf %s "$new" | podman secret create --replace famat-db-password -
  systemctl restart famat-postgres famat-app
  ```

- **Garage key:**
  1. Delete the two `famat-s3-*` secrets and re-run `deploy/create-secrets.sh`.
  2. Run `famat-garage-bootstrap`, which imports the new key.
  3. Restart `famat-app`.
  4. Delete the old key: `podman exec famat-garage /garage key delete --yes <old key id>`.
- **Admin passcode:** `famat set-passcode` ([admin.md](admin.md)).
