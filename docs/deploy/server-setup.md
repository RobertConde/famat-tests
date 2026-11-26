# First-time server setup (Debian 13)

From a Debian 13 (trixie) machine with SSH access to the containers serving `famat.rconde.xyz`, and nightly backups running. Everything runs as root (`sudo -i`). Commands that need a value from you have it in `<angle brackets>`.

You need:

- The Cloudflare account that holds the `rconde.xyz` zone.
- The Hetzner Storage Box (BX21): its user name (`u123456`) and host (`u123456.your-storagebox.de`).
- The Better Stack account.
- A password manager. Three things only exist there and on the server: the **restic password**, the **Storage Box login**, and the **Admin passcode**.

## 1. Packages

```sh
apt update
apt install podman catatonit nftables curl openssl restic rsync git
podman --version     # 5.4 or later; Quadlet .pod units need Podman 5
```

`catatonit` and `nftables` normally come with `podman` as recommends. Without them, the app has no init (`RunInit=true`) and containers have no network. They are listed so that `--no-install-recommends` setups get them too.

The named volumes live in `/var/lib/containers/storage/volumes/`. The Garage data volume holds every Original and PDF. If `/var` is not on the RAID, put it there before step 4. For example, bind-mount a RAID directory onto `/var/lib/containers`, or add `Device=/srv/raid/famat-garage-data` with `Type=none` and `Options=bind` to `famat-garage-data.volume`.

## 2. The deployment files

```sh
git clone https://github.com/RobertConde/famat-tests.git /opt/famat-tests
cd /opt/famat-tests
deploy/install.sh
```

`install.sh` copies:

- the Quadlet units to `/etc/containers/systemd/`
- Garage's config to `/etc/famat/garage.toml`
- `famat`, `famat-backup` and `famat-garage-bootstrap` to `/usr/local/sbin/`
- the backup service and timer to `/etc/systemd/system/`
- `/etc/famat/backup.conf`, from the example, the first time only

It then runs `systemctl daemon-reload`. To check that Quadlet understood the units:

```sh
/usr/libexec/podman/quadlet -dryrun | grep -E '^---|ExecStart='
```

## 3. The tunnel and the secrets

First create the tunnel in Cloudflare and copy its token: [cloudflare.md](cloudflare.md), part 1. Then:

```sh
deploy/create-secrets.sh          # asks for the tunnel token (no echo)
podman secret ls
```

This creates five Podman secrets: `famat-db-password`, `famat-s3-access-key-id`, `famat-s3-secret-access-key`, `famat-garage-rpc-secret` and `famat-tunnel-token`. All but the token are random. They are stored only in Podman's secret store on this machine, never in git. A secret that already exists is kept, so the script is safe to re-run.

To replace the tunnel token later:

```sh
read -rs -p 'Token: ' token; echo
printf %s "$token" | podman secret create --replace famat-tunnel-token -
systemctl restart famat-cloudflared
```

## 4. Start the containers

The app image is `ghcr.io/robertconde/famat-tests`, pushed by CI (`.github/workflows/image.yml`) from every green `main`. The first push creates the package as **private**, and the server pulls without logging in. So make it public once: on GitHub, open the repository's Packages → `famat-tests` → Package settings → Change visibility → Public. To check from the server, run `podman pull ghcr.io/robertconde/famat-tests:latest`.

Pull the images first, so the first start doesn't hit the units' start timeouts on a slow pull:

```sh
podman pull ghcr.io/robertconde/famat-tests:latest docker.io/library/postgres:17 \
  docker.io/dxflrs/garage:v2.6.0 docker.io/cloudflare/cloudflared:latest \
  docker.io/rclone/rclone:1 registry.k8s.io/pause:3.10
systemctl start famat-net.service
```

`famat-net` holds the shared network namespace (it works like a pod's infra container; `deploy/quadlet/famat-net.container` says why it isn't a Quadlet pod). Starting it starts the rest in order:

1. Postgres and Garage, each counted as started only once its health check passes (`Notify=healthy`).
2. The app, which runs the migrations and passes `/healthz`.
3. cloudflared.

The units are `WantedBy=multi-user.target`, so they also start on every boot. There is nothing to enable, because Quadlet units are generated.

Then set up Garage once (a layout role for the single node, the `famat` bucket, and the app's key imported from the secrets). The script is idempotent:

```sh
famat-garage-bootstrap
```

Check:

```sh
systemctl list-units 'famat*'
podman ps --format '{{.Names}}  {{.Status}}'      # famat-net up; garage, postgres, app (healthy)
podman exec famat-app curl -fsS http://127.0.0.1:8080/healthz; echo    # ok
journalctl -u famat-cloudflared -n 20             # "Registered tunnel connection" ×4
```

Nothing listens on the host, and inside the shared namespace everything listens on 127.0.0.1 only. The app trusts `CF-Connecting-IP`, so this matters; never add a `PublishPort=`. To check:

```sh
ss -ltnp | grep -E '8080|5432|3900'                                    # nothing
nsenter -t "$(podman inspect famat-net --format '{{.State.Pid}}')" -n ss -ltn   # all 127.0.0.1
```

## 5. Automatic updates

```sh
systemctl enable --now podman-auto-update.timer
systemctl list-timers podman-auto-update.timer
```

Every day this pulls new images for the containers marked `AutoUpdate=registry`: the app (`:latest`, pushed by CI from `main`), Postgres 17 minor releases, and cloudflared. It restarts what changed and rolls back to the previous image if the restart fails. See [upgrading.md](upgrading.md) for how rollback works and what to check.

## 6. The Admin passcode

```sh
famat set-passcode
```

See [admin.md](admin.md). Put the passcode in the password manager.

## 7. Backups

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
- **Nextcloud:** find the real install first:
  - `occ` path (or a `podman exec … occ` if Nextcloud runs in a container)
  - database type and name (`dbtype`, `dbname` in `config/config.php`)
  - `datadirectory`

  Set `NEXTCLOUD_OCC`, `NEXTCLOUD_DB_DUMP` and `NEXTCLOUD_PATHS`, then `NEXTCLOUD_ENABLED=1`. The dump command's stdout is the dump. As root, Debian's MariaDB needs no password.
- **WeBWorK:** set `WEBWORK_DB_DUMP` from the database name in `webwork2/conf/site.conf`, and `WEBWORK_PATHS` to its `courses/` (and `conf/`). Then set `WEBWORK_ENABLED=1`.

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

1. Each part makes its dumps in `/var/backups/famat-staging` and backs them up as its own snapshot, tagged `famat`, `nextcloud` or `webwork`.
2. `forget --prune` keeps 7 daily, 4 weekly and 12 monthly snapshots per tag.
3. On Sundays, `restic check` runs.
4. Then the heartbeat is pinged.

The staging directory keeps a full copy of the Garage bucket, so leave room for it. Later runs only copy new objects.

Then do a test restore: [restore.md](restore.md). Repeat it every quarter.

## 8. Monitors

In Better Stack, add HTTP monitors for `https://famat.rconde.xyz/healthz` and, after launch, `https://tests.famat.org/healthz`. Each should expect status 200 and the body `ok`. `/healthz` answers 200 only when the app can reach Postgres.
