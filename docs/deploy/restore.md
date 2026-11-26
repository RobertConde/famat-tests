# Restoring from the backups

All commands run as root on the server. `famat-backup restic …` runs restic against the repository in `/etc/famat/backup.conf`, with its password.

## What a snapshot holds

Each nightly run makes one snapshot per part. Its tag is the part's name:

| Tag | Paths in the snapshot |
|---|---|
| `famat` | `/var/backups/famat-staging/famat/famat.pgdump`: `pg_dump --format=custom` of the `famat` database (every Test, Document, Facet value, the change log, the passcode hash, sessions)<br>`/var/backups/famat-staging/famat/garage/documents/<id>/original` and `…/pdf`: every object in the Garage bucket, under its own key |
| `nextcloud` | `/var/backups/famat-staging/nextcloud/nextcloud-db.sql`, plus `NEXTCLOUD_PATHS` (data and config) as they were in maintenance mode |
| `webwork` | `/var/backups/famat-staging/webwork/webwork-db.sql`, plus `WEBWORK_PATHS` (courses and conf) |

```sh
famat-backup restic snapshots                  # all of them
famat-backup restic snapshots --tag famat      # the test bank's, newest last
famat-backup restic ls latest --tag famat /var/backups/famat-staging/famat/garage/documents/1160
```

Below, `latest --tag famat` can be replaced by a snapshot ID from that list to go further back.

## The database into a scratch database

Use this for the quarterly restore test, or to look up an old value without touching the live data.

```sh
famat-backup restic dump latest --tag famat /var/backups/famat-staging/famat/famat.pgdump > /root/famat.pgdump
podman exec famat-postgres createdb -U famat famat_restore
podman exec -i famat-postgres pg_restore -U famat -d famat_restore --no-owner --exit-on-error < /root/famat.pgdump
podman exec famat-postgres psql -U famat -d famat_restore -c \
  "select (select count(*) from tests) tests, (select count(*) from documents) documents, (select max(changed_at) from change_log) last_change"
```

Compare the counts with the live database (`-d famat`). When done:

```sh
podman exec famat-postgres dropdb -U famat famat_restore
rm /root/famat.pgdump
```

## The live database, after a bad edit or a bad migration

This replaces everything since the snapshot.

1. Restore into `famat_restore` as above and check it.
2. Stop the app and rename the databases:

   ```sh
   systemctl stop famat-cloudflared famat-app
   podman exec famat-postgres psql -U famat -d postgres -c 'alter database famat rename to famat_broken'
   podman exec famat-postgres psql -U famat -d postgres -c 'alter database famat_restore rename to famat'
   systemctl start famat-app famat-cloudflared
   ```

3. Once the site checks out, `dropdb -U famat famat_broken`.

The Garage objects need no restore for this: objects are never renamed or rewritten in place. Any objects stored after the snapshot are just unreferenced.

## One file

A Document's Original or PDF, by Document ID (`D1160` is `documents/1160/`):

```sh
famat-backup restic dump latest --tag famat /var/backups/famat-staging/famat/garage/documents/1160/original > /root/D1160-original
sha256sum /root/D1160-original
podman exec famat-postgres psql -U famat -At -c "select role, encode(sha256, 'hex') from document_files where document_id = 1160"
```

The sha256 should match the database's. To put it back into Garage, for an object deleted by mistake:

```sh
podman run --rm --network container:famat-net -v /root:/restore:ro \
  --secret famat-s3-access-key-id,type=env,target=RCLONE_S3_ACCESS_KEY_ID \
  --secret famat-s3-secret-access-key,type=env,target=RCLONE_S3_SECRET_ACCESS_KEY \
  docker.io/rclone/rclone:1 copyto /restore/D1160-original :s3:famat/documents/1160/original \
  --s3-provider Other --s3-endpoint http://127.0.0.1:3900 --s3-region garage --s3-force-path-style
```

Any other file (from Nextcloud, WeBWorK or this server's configuration) works the same way. Find it with `famat-backup restic find <name>`, then `dump` it, or `restore` a directory:

```sh
famat-backup restic restore latest --tag nextcloud --target /root/restore --include /var/www/nextcloud/data/<user>/files/<path>
```

## Full disaster recovery (a new server)

You need from the password manager:

- the **restic password**
- the **Storage Box login**
- the Cloudflare login. The tunnel token can be re-copied from the tunnel's page.

The other secrets can be new: the database is restored from a dump and the Garage objects are copied back, so neither depends on the old passwords or keys.

1. **Base setup.** Follow [server-setup.md](server-setup.md) steps 1–3:
   - packages, then the deployment files and `install.sh`
   - `create-secrets.sh` with the same tunnel's token
2. **Repository access.** Do the "Storage Box access" part of step 7. Put the old restic password into `/etc/famat/restic-password` instead of making a new one, and set `RESTIC_REPOSITORY` in `/etc/famat/backup.conf`. **Don't run `restic init`.** Check with `famat-backup restic snapshots`.
3. **Restore the test bank's staging copy:**

   ```sh
   famat-backup restic restore latest --tag famat --target /
   ls /var/backups/famat-staging/famat/
   ```

4. **Start the containers and set up Garage:**

   ```sh
   systemctl start famat-net        # starts all of them; the app migrates an empty database
   famat-garage-bootstrap
   ```

5. **Replace the empty database with the dump.** Stop the app and the tunnel first, so nothing serves or writes meanwhile:

   ```sh
   systemctl stop famat-cloudflared famat-app
   podman exec famat-postgres dropdb -U famat famat
   podman exec famat-postgres createdb -U famat famat
   podman exec -i famat-postgres pg_restore -U famat -d famat --no-owner --exit-on-error \
     < /var/backups/famat-staging/famat/famat.pgdump
   ```

6. **Copy the objects back into Garage:**

   ```sh
   podman run --rm --network container:famat-net -v /var/backups/famat-staging/famat/garage:/staging:ro \
     --secret famat-s3-access-key-id,type=env,target=RCLONE_S3_ACCESS_KEY_ID \
     --secret famat-s3-secret-access-key,type=env,target=RCLONE_S3_SECRET_ACCESS_KEY \
     docker.io/rclone/rclone:1 copy /staging :s3:famat \
     --s3-provider Other --s3-endpoint http://127.0.0.1:3900 --s3-region garage --s3-force-path-style
   ```

7. **Start the app and the tunnel:** `systemctl start famat-app famat-cloudflared`. Then check:
   - `podman ps`: all healthy.
   - The tunnel is Healthy in Cloudflare.
   - `https://famat.rconde.xyz/healthz` answers, and a Document downloads.
   - Log in with the old passcode. Its hash came back with the database.
8. **Nextcloud and WeBWorK.** Reinstall the same versions first, then put back their data:

   ```sh
   famat-backup restic restore latest --tag nextcloud --target /
   famat-backup restic restore latest --tag webwork --target /
   ```

   Then load their dumps (`mysql nextcloud < /var/backups/famat-staging/nextcloud/nextcloud-db.sql`, and the same for `webwork`). Then:

   ```sh
   sudo -u www-data php occ maintenance:mode --off
   sudo -u www-data php occ maintenance:data-fingerprint
   ```

   `restore` brings back the files' owners and modes, as long as the same users (`www-data`, the WeBWorK user) exist.
9. **Resume backups:** `systemctl enable --now famat-backup.timer podman-auto-update.timer`, then `systemctl start famat-backup`. The next snapshot carries on in the same repository.

The cost of a full restore is the download: about one copy of everything from the Storage Box.
