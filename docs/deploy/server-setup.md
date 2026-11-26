# First-time server setup (Debian 13, Docker)

This takes the server (Debian 13, Docker already running, the RAID at `/mnt/raid`) to the test bank serving `famat.rconde.xyz`, updating itself, and backed up nightly. Everything runs as root (`sudo -i`). Commands that need a value from you have it in `<angle brackets>`.

You need:

- The Cloudflare account that holds the `rconde.xyz` zone.
- The Hetzner Storage Box (BX21): its user name (`u123456`) and host (`u123456.your-storagebox.de`).
- The Better Stack account.
- A password manager. Three things only exist there and on the server: the **restic password**, the **Storage Box login**, and the **Admin passcode**.

**Leave alone, at every step:** the host `cloudflared` service, `/etc/cloudflared/` and the other tunnel's routes; Nextcloud's containers, networks and volumes; MariaDB and WeBWorK. The test bank never needs them changed. In particular:

- Never run `cloudflared service install` on the host. The test bank's cloudflared is a container.
- Never run `docker system prune` or `docker volume prune` here. They would also remove things belonging to other stacks.
- Don't add users to the `docker` group. It is root-equivalent, and it can read the secrets through `docker inspect`.

## 0. Check the server first

These only read:

```sh
cat /etc/debian_version                          # 13.x
docker version --format '{{.Server.Version}}'    # 29.x
docker compose version                           # the Compose plugin
cat /proc/mdstat                                 # md0 active, every disk up ([UUUU])
findmnt /mnt/raid                                # /dev/md0 (or its UUID) mounted
grep -E '[[:space:]]/mnt/raid[[:space:]]' /etc/fstab
systemctl status mnt-raid.mount --no-pager       # loaded from /etc/fstab, active
df -h / /mnt/raid
ss -ltnp | grep -E ':(80|443|5432|3900|3901|8080)\b'   # nothing (the test bank publishes no port anyway)
docker ps -a --format '{{.Names}}' | grep -i famat     # nothing: no name clash
docker network ls --format '{{.Name}}' | grep -i famat # nothing
systemctl status cloudflared --no-pager          # the other tunnel. Only look.
```

- If `docker compose version` fails, install the plugin from the same (Docker) apt repository as Docker itself: `apt install docker-compose-plugin`.
- `famat.service` depends on the systemd mount unit `mnt-raid.mount`, which systemd makes from the `/etc/fstab` line. If `/mnt/raid` is not in `/etc/fstab` (mounted some other way), add a line for it before step 5. Use `UUID=<from blkid /dev/md0> /mnt/raid <its fstype> defaults,nofail 0 2`. `nofail` lets the server boot, with SSH, even when the RAID doesn't assemble. Without the mount, the test bank stays down; it never starts on the root disk instead.

## 1. Packages

Docker is already installed. The rest:

```sh
apt update
apt install restic rsync git curl openssl
restic version     # 0.18 or later
```

Don't install Podman. This deployment uses Docker only.

## 2. Data directories on the RAID

Postgres's data, Garage's metadata and objects, and the backups' staging copy all live under `/mnt/raid/famat/`. Make the directories only while the RAID is mounted:

```sh
mountpoint /mnt/raid      # "/mnt/raid is a mountpoint"; stop here if not
install -d -m 0700 /mnt/raid/famat
install -d -m 0700 /mnt/raid/famat/postgres /mnt/raid/famat/garage \
  /mnt/raid/famat/garage/meta /mnt/raid/famat/garage/data /mnt/raid/famat/backup-staging
```

The containers never create these directories. If the RAID isn't mounted, the directories don't exist on the root disk, so the containers refuse to start. `famat.service` also checks the mount itself (step 5).

## 3. The deployment files

```sh
git clone https://github.com/RobertConde/famat-tests.git /opt/famat-tests
cd /opt/famat-tests
deploy/install.sh
```

`install.sh` copies:

- `deploy/compose.yaml` to `/etc/famat/compose.yaml`, and Garage's config to `/etc/famat/garage.toml`
- `famat`, `famat-update`, `famat-backup` and `famat-garage-bootstrap` to `/usr/local/sbin/`
- `famat.service`, `famat-update.service` and `.timer`, and `famat-backup.service` and `.timer` to `/etc/systemd/system/`
- `/etc/famat/backup.conf`, from the example, the first time only

It then runs `systemctl daemon-reload`. It starts nothing.

## 4. The tunnel and the secrets

First create the test bank's **own** tunnel in Cloudflare and copy its token: [cloudflare.md](cloudflare.md), part 1. It is a new tunnel, separate from the one the host's `cloudflared` runs. Then:

```sh
cd /opt/famat-tests
deploy/create-secrets.sh          # asks for the tunnel token (no echo)
ls -l /etc/famat/secrets/         # db.env  garage.env  s3.env  tunnel.env, all -rw------- root
deploy/install.sh                 # again: now it also checks the compose file ("docker compose config: OK")
```

This writes four root-only env files to `/etc/famat/secrets/`. All the values but the token are random. They exist only there, never in git. A file that already exists is kept, so the script is safe to re-run.

To replace the tunnel token later:

```sh
read -rs -p 'Token: ' token; echo
printf 'TUNNEL_TOKEN=%s\n' "$token" > /etc/famat/secrets/tunnel.env
cd /etc/famat && docker compose up -d cloudflared     # recreates it with the new token
```

## 5. Start the containers

The app image is `ghcr.io/robertconde/famat-tests`, pushed by CI (`.github/workflows/image.yml`) from every green `main`. The first push creates the package as **private**, and the server pulls without logging in. So make it public once: on GitHub, open the repository's Packages → `famat-tests` → Package settings → Change visibility → Public.

Pull the images first, so the first start doesn't run into its timeout on a slow pull:

```sh
cd /etc/famat
docker compose pull app postgres garage cloudflared rclone
systemctl enable --now famat.service
```

`famat.service` runs `docker compose up` in `/etc/famat`, in order:

1. Postgres and Garage, each counted as started once its health check passes.
2. The app (`depends_on: … service_healthy`). It runs the migrations, then passes `/healthz`. The unit counts as started at this point.
3. cloudflared, which dials out to Cloudflare.

It starts after `docker.service` and after `/mnt/raid` is mounted (`RequiresMountsFor=`). If the RAID isn't mounted, or `/mnt/raid` is only a directory, it refuses to start. `systemctl status famat` then says `Dependency failed` or `Assertion failed`. Once the RAID is back (`cat /proc/mdstat`, `mount /mnt/raid`), run `systemctl start famat`. `systemctl stop famat` removes the containers; the data stays on the RAID. It is `enable`d, so it also starts on every boot.

Then set up Garage once: a layout role for the single node, the `famat` bucket, and the app's key imported from `s3.env`. The script is idempotent:

```sh
famat-garage-bootstrap
```

Check:

```sh
systemctl status famat --no-pager
cd /etc/famat && docker compose ps     # postgres, garage, app (healthy); cloudflared (healthy) once the tunnel is up
docker exec famat-app curl -fsS http://127.0.0.1:8080/healthz; echo     # ok
docker logs --tail 20 famat-cloudflared    # "Registered tunnel connection" ×4
journalctl -t famat-app -n 20              # the app's log (also: docker logs famat-app)
```

Nothing listens on the host, so nothing but cloudflared reaches the app. The app trusts `CF-Connecting-IP`, so this matters. Never add `ports:` to the compose file or the override. To check:

```sh
ss -ltnp | grep -E ':(5432|3900|3901|8080)\b'                         # nothing
docker ps --filter label=com.docker.compose.project=famat --format '{{.Names}} {{.Ports}}'   # no "->"
```

## 6. Automatic updates

```sh
systemctl enable --now famat-update.timer
systemctl list-timers famat-update.timer
```

Every day at about 05:00 (after the backup), `famat-update` pulls new images for the app (`:latest`, pushed by CI from `main`), Postgres 17 minor releases, and cloudflared. For each one that changed, it recreates only that container. If it isn't healthy within 5 minutes, it puts the previous image back. See [upgrading.md](upgrading.md) for how rollback works and what to check.

## 7. The Admin passcode

```sh
famat set-passcode
```

See [admin.md](admin.md). Put the passcode in the password manager.

## 8. Backups

### Storage Box access

1. In Hetzner's console, turn on **SSH support** for the Storage Box. Restic uses SFTP on port 23.
2. Give root an SSH key for it, and install the key on the box:

   ```sh
   ssh-keygen -t ed25519 -N '' -f /root/.ssh/storagebox -C "famat-backup@$(hostname)"
   cat /root/.ssh/storagebox.pub | ssh -p 23 <u123456>@<u123456>.your-storagebox.de install-ssh-key
   ```

   The second command asks for the Storage Box password once.
3. Add the host alias that `backup.conf` uses to `/root/.ssh/config`:

   ```
   Host storagebox
       HostName <u123456>.your-storagebox.de
       User <u123456>
       Port 23
       IdentityFile /root/.ssh/storagebox
       ServerAliveInterval 60
   ```

4. Accept the host key: `ssh storagebox ls` (answer `yes`).

### The restic repository

```sh
umask 077
openssl rand -base64 32 > /etc/famat/restic-password   # copy this into the password manager NOW
famat-backup restic init
```

Without that password, the backups cannot be read. The Storage Box only ever sees encrypted data.

### Settings

Edit `/etc/famat/backup.conf`:

- **`BETTERSTACK_HEARTBEAT_URL`:** in Better Stack, open Uptime → Heartbeats → Create heartbeat. Name it "Nightly backup", set the period to 1 day and the grace period to 2 hours, and pick who to alert. Then copy the URL. A successful run requests it, and a failed run requests `<url>/fail`, which alerts at once.
- **`STAGING`** stays `/mnt/raid/famat/backup-staging` (step 2). It holds a full copy of the Garage bucket.
- **Nextcloud.** Find out how it runs first. These only read:

  ```sh
  docker ps --format '{{.Names}}\t{{.Image}}'        # Nextcloud's container, and its database's if that is one too
  docker exec -u www-data <nextcloud container> php occ status
  docker exec -u www-data <nextcloud container> php occ config:system:get datadirectory
  docker exec -u www-data <nextcloud container> php occ config:system:get dbtype
  docker exec -u www-data <nextcloud container> php occ config:system:get dbname
  docker exec -u www-data <nextcloud container> php occ config:system:get dbhost
  docker inspect -f '{{range .Mounts}}{{.Source}} -> {{.Destination}}{{println}}{{end}}' <nextcloud container>
  ```

  - `dbhost` says where its database is. It is the host's MariaDB if it is the host's address (or `host.docker.internal`), or another container if it is that container's name.
  - The data directory and `config/` are backed up by their paths **on the host**: the `Source` of the mount that holds them. A named volume's `Source` is under `/var/lib/docker/volumes/<name>/_data`.

  Then set `NEXTCLOUD_OCC`, `NEXTCLOUD_DB_DUMP` and `NEXTCLOUD_PATHS` from the examples in `backup.conf`, and set `NEXTCLOUD_ENABLED=1`. The dump command's stdout is the dump. If it's Nextcloud All-in-One (a `nextcloud/all-in-one` mastercontainer), it has its own backup, which this script would only get in the way of. Leave `NEXTCLOUD_ENABLED=0` and use that.
  If Nextcloud isn't in Docker after all, use the native examples: `sudo -u www-data php …/occ`, and `mysqldump` (as root, Debian's MariaDB needs no password).
- **WeBWorK** (native): set `WEBWORK_DB_DUMP` from the database name in `webwork2/conf/site.conf`, and `WEBWORK_PATHS` to its `courses/` (and `conf/`). Then set `WEBWORK_ENABLED=1`.

Try each part by hand before trusting the timer:

```sh
famat-backup part famat
famat-backup part nextcloud    # checks maintenance mode goes on and back off
famat-backup part webwork
```

### The timer

```sh
systemctl enable --now famat-backup.timer
systemctl start famat-backup.service       # the first full run now
journalctl -u famat-backup -f
famat-backup restic snapshots
```

The first upload of everything can take hours. Later runs only send changes.

Each night (03:30, plus up to 20 minutes of random delay):

1. Each part makes its dumps in `/mnt/raid/famat/backup-staging` and backs them up as its own snapshot, tagged `famat`, `nextcloud` or `webwork`.
2. `forget --prune` keeps 7 daily, 4 weekly and 12 monthly snapshots per tag.
3. On Sundays, `restic check` runs.
4. Then the heartbeat is pinged.

`famat-backup` and `famat-update` share a lock (`/run/famat.lock`), so an update never recreates Postgres in the middle of a dump.

Then do a test restore: [restore.md](restore.md). Repeat it every quarter.

## 9. Monitors

In Better Stack, add HTTP monitors for `https://famat.rconde.xyz/healthz` and, after launch, `https://tests.famat.org/healthz`. Each should expect status 200 and the body `ok`. `/healthz` answers 200 only when the app can reach Postgres.

Docker doesn't restart a container just because it turned unhealthy, so these monitors are what notice a hung app. If one alerts: `cd /etc/famat && docker compose ps`, `journalctl -t famat-app -n 100`, then `docker compose restart app`.
