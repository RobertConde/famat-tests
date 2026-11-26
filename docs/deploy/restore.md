# Restoring from the backups

All commands run as root on the server. `famat-backup restic …` runs restic against the repository in `/etc/famat/backup.conf`, with its password. `docker compose` commands run in `/etc/famat`.

## What a snapshot holds

Each nightly run makes one snapshot per part. Its tag is the part's name:

| Tag | Paths in the snapshot |
|---|---|
| `famat` | `/mnt/raid/famat/backup-staging/famat/famat.pgdump`: `pg_dump --format=custom` of the `famat` database (every Test, Document, Facet value, the change log, the passcode hash, sessions)<br>`/mnt/raid/famat/backup-staging/famat/garage/documents/<id>/original` and `…/pdf`: every object in the Garage bucket, under its own key |
| `nextcloud` | `/mnt/raid/famat/backup-staging/nextcloud/nextcloud-db.sql`, plus `NEXTCLOUD_PATHS` (data and config, as host paths) as they were in maintenance mode |
| `webwork` | `/mnt/raid/famat/backup-staging/webwork/webwork-db.sql`, plus `WEBWORK_PATHS` (courses and conf) |

```sh
famat-backup restic snapshots                  # all of them
famat-backup restic snapshots --tag famat      # the test bank's, newest last
famat-backup restic ls latest --tag famat /mnt/raid/famat/backup-staging/famat/garage/documents/1160
```

Below, `latest --tag famat` can be replaced by a snapshot ID from that list to go further back.

## The database into a scratch database

Use this for the quarterly restore test, or to look up an old value without touching the live data.

```sh
famat-backup restic dump latest --tag famat /mnt/raid/famat/backup-staging/famat/famat.pgdump > /root/famat.pgdump
docker exec famat-postgres createdb -U famat famat_restore
docker exec -i famat-postgres pg_restore -U famat -d famat_restore --no-owner --exit-on-error < /root/famat.pgdump
docker exec famat-postgres psql -U famat -d famat_restore -c \
  "select (select count(*) from tests) tests, (select count(*) from documents) documents, (select max(changed_at) from change_log) last_change"
```

Compare the counts with the live database (`-d famat`). When done:

```sh
docker exec famat-postgres dropdb -U famat famat_restore
rm /root/famat.pgdump
```

## The live database, after a bad edit or a bad migration

This replaces everything since the snapshot.

1. Restore into `famat_restore` as above and check it.
2. Stop the app and the tunnel, and rename the databases:

   ```sh
   cd /etc/famat
   docker compose stop cloudflared app
   docker exec famat-postgres psql -U famat -d postgres -c 'alter database famat rename to famat_broken'
   docker exec famat-postgres psql -U famat -d postgres -c 'alter database famat_restore rename to famat'
   docker compose start app cloudflared
   ```

3. Once the site checks out, `docker exec famat-postgres dropdb -U famat famat_broken`.

The Garage objects need no restore for this: objects are never renamed or rewritten in place. Any objects stored after the snapshot are just unreferenced.

## One file

A Document's Original or PDF, by Document ID (`D1160` is `documents/1160/`):

```sh
famat-backup restic dump latest --tag famat /mnt/raid/famat/backup-staging/famat/garage/documents/1160/original > /root/D1160-original
sha256sum /root/D1160-original
docker exec famat-postgres psql -U famat -At -c "select role, encode(sha256, 'hex') from document_files where document_id = 1160"
```

The sha256 should match the database's. To put it back into Garage, for an object deleted by mistake, use the compose file's `rclone` service (it is on the internal network, with the app's key):

```sh
cd /etc/famat
docker compose run --rm --no-deps -T -v /root:/restore:ro \
  rclone copyto /restore/D1160-original :s3:famat/documents/1160/original
```

Any other file (from Nextcloud, WeBWorK or this server's configuration) works the same way. Find it with `famat-backup restic find <name>`, then `dump` it, or `restore` a directory:

```sh
famat-backup restic restore latest --tag nextcloud --target /root/restore --include <a path from NEXTCLOUD_PATHS>/<user>/files/<path>
```

## Full disaster recovery (empty data directories, or a new server)

For when the data under `/mnt/raid/famat/` is gone: a replaced RAID, or a new server.

You need from the password manager:

- the **restic password**
- the **Storage Box login**
- the Cloudflare login. The tunnel token can be re-copied from the `famat` tunnel's page.

The other secrets can be new: the database is restored from a dump and the Garage objects are copied back, so neither depends on the old passwords or keys.

1. **Base setup.** Follow [server-setup.md](server-setup.md) steps 0–4:
   - the checks, packages, and the (empty) data directories on the RAID
   - the deployment files and `install.sh`
   - `create-secrets.sh` with the `famat` tunnel's token

   On the same server, with only the data lost: `systemctl stop famat` first. Then move whatever is left of `/mnt/raid/famat/postgres` and `garage/` aside, and make the directories again as in step 2. Keep `/etc/famat/secrets/`; it still works.
2. **Repository access.** Do the "Storage Box access" part of step 8. Put the old restic password into `/etc/famat/restic-password` instead of making a new one, and set `RESTIC_REPOSITORY` in `/etc/famat/backup.conf`. **Don't run `restic init`.** Check with `famat-backup restic snapshots`.
3. **Restore the test bank's staging copy:**

   ```sh
   famat-backup restic restore latest --tag famat --target /
   ls /mnt/raid/famat/backup-staging/famat/
   ```

4. **Start the containers and set up Garage:**

   ```sh
   cd /etc/famat
   docker compose pull app postgres garage cloudflared rclone
   systemctl enable --now famat     # the app migrates an empty database
   famat-garage-bootstrap
   ```

5. **Replace the empty database with the dump.** Stop the app and the tunnel first, so nothing serves or writes meanwhile:

   ```sh
   docker compose stop cloudflared app
   docker exec famat-postgres dropdb -U famat famat
   docker exec famat-postgres createdb -U famat famat
   docker exec -i famat-postgres pg_restore -U famat -d famat --no-owner --exit-on-error \
     < /mnt/raid/famat/backup-staging/famat/famat.pgdump
   ```

6. **Copy the objects back into Garage:**

   ```sh
   docker compose run --rm --no-deps -T -v /mnt/raid/famat/backup-staging/famat/garage:/staging:ro \
     rclone copy /staging :s3:famat
   ```

7. **Start the app and the tunnel:** `docker compose start app cloudflared`. Then check:
   - `docker compose ps`: all healthy.
   - The tunnel is Healthy in Cloudflare.
   - `https://famat.rconde.xyz/healthz` answers, and a Document downloads.
   - Log in with the old passcode. Its hash came back with the database.
8. **Nextcloud and WeBWorK.** Reinstall the same versions first (for Nextcloud in Docker: its compose project or containers, then stop them), then put back their data:

   ```sh
   famat-backup restic restore latest --tag nextcloud --target /
   famat-backup restic restore latest --tag webwork --target /
   ```

   Then load their dumps:
   - Into a MariaDB container: `docker exec -i <db container> sh -c 'export MYSQL_PWD="$MARIADB_ROOT_PASSWORD"; exec mariadb -uroot nextcloud' < /mnt/raid/famat/backup-staging/nextcloud/nextcloud-db.sql`
   - Into the host's MariaDB: `mysql nextcloud < /mnt/raid/famat/backup-staging/nextcloud/nextcloud-db.sql`, and the same for `webwork`.

   Then start Nextcloud, and run its `occ` (`NEXTCLOUD_OCC` in `backup.conf`) with `maintenance:mode --off`, then `maintenance:data-fingerprint`.

   `restore` brings back the files' owners and modes, as numeric IDs. For Nextcloud in Docker these are the container's `www-data` (33), so they match the same image again. For WeBWorK, the same users must exist on the host.
9. **Resume backups and updates:** `systemctl enable --now famat-backup.timer famat-update.timer`, then `systemctl start famat-backup`. The next snapshot carries on in the same repository.

The cost of a full restore is the download: about one copy of everything from the Storage Box.
