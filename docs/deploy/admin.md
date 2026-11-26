# Running `famat import` and `famat set-passcode` on the server

Both are subcommands of the app's binary. On the server, `/usr/local/sbin/famat` (from `deploy/bin/famat`) runs them in a throwaway container of the compose file's `app` service (`docker compose run --rm app …`). It has the app's image, environment and secrets, on the same `famat-backend` network, so it reaches Postgres and Garage just as the app does. Run it as root, with the containers up (`systemctl status famat`).

## `famat set-passcode`

```sh
famat set-passcode
```

It asks twice for the new passcode, without echo, and needs at least 8 characters. Only its argon2id hash is stored, in Postgres, so it is backed up with the database and restored with it. Setting it ends every Admin session. It is never a command-line argument; to script it, pipe it in: `famat set-passcode < file`.

Put the passcode in the password manager and give it to the owner and FAMAT's officers. To change it later, run the same command.

## `famat import`

The import builds the archive from the old site's data. Those files live on the Admin's Mac, outside git, and the SQL dump holds personal data, so copy them straight to the server and remove them after launch.

### 1. Copy the inputs to the server

The import reads six inputs, all kept on the Admin's Mac outside git:

| Input | Source on the Mac |
|---|---|
| the students' sheet | `test list - 000final.csv` |
| the old site's database (legacy IDs) | `doralmao_famat.sql` |
| AI labels, attach decisions, review list | `data/labels/` (`ai-labels.csv`, `attach-1.out.csv`, `attach-2.out.csv`, `review-core.csv`, `unpacked.csv`) |
| the files unpacked from the old zip bundles | `data/unpacked/` |
| the docs export | the OneDrive `FAMAT Test Bank/docs` folder |

`code.tar.gz` is not an input. It holds the old site's database password; never copy it anywhere.

Gather them into one folder and archive it. The command-line `zip` adds no macOS metadata; Finder's "Compress" does, so don't use that.

```sh
cd ~/Code/famat-tests
mkdir -p ~/Desktop/famat-import
cp "test list - 000final.csv" doralmao_famat.sql ~/Desktop/famat-import/
cp -R data/labels ~/Desktop/famat-import/labels
cp -R data/unpacked ~/Desktop/famat-import/unpacked
cp -R "<the docs export>" ~/Desktop/famat-import/docs
cd ~/Desktop && zip -qr famat-import.zip famat-import
```

Get the archive onto the server any way that works: Nextcloud, rsync over SSH or Tailscale, or a USB drive. Then:

```sh
sudo apt install -y unzip
sudo mkdir -p /srv/famat/import
sudo unzip -q famat-import.zip -d /srv/famat/import
ls /srv/famat/import/famat-import
# docs  doralmao_famat.sql  labels  test list - 000final.csv  unpacked
```

### 2. Dry run

Paths inside the container are under `/input`, which is the folder given to `--input`, mounted read-only. So the full report goes to standard output (`--report /dev/stdout`) and is saved with `tee`:

```sh
cd /srv/famat/import
sudo famat --input /srv/famat/import/famat-import import \
  --sheet "/input/test list - 000final.csv" \
  --ai-labels /input/labels/ai-labels.csv \
  --docs /input/docs \
  --legacy-sql /input/doralmao_famat.sql \
  --unpacked /input/labels/unpacked.csv \
  --unpacked-dir /input/unpacked \
  --attach /input/labels/attach-1.out.csv \
  --attach /input/labels/attach-2.out.csv \
  --review /input/labels/review-core.csv \
  --report /dev/stdout \
  --dry-run | sudo tee report-dry-run.txt | tail -20
```

This only reads. Expect about 4,000 Tests and 9,131 Documents, with `Problems: none`.

### 3. The real import

Run the same command without `--dry-run`, saving to a new report:

- Every file streams into Garage and is read back and checked before Postgres changes.
- Then one transaction replaces the archive.
- Every Word and PowerPoint Original is queued for PDF conversion. The app's two workers convert them in the background, and the site is usable meanwhile.

```sh
sudo famat --input /srv/famat/import/famat-import import \
  … the same arguments, without --dry-run … | sudo tee report-$(date +%F-%H%M).txt | tail -20
```

Repeat steps 1 to 3 until the report is clean and a 20-Test spot check on `https://famat.rconde.xyz` passes. The import refuses to run once an Admin has changed anything, so finish importing before Admins start editing.

To watch the PDF backfill:

```sh
docker exec famat-postgres psql -U famat -c "select pdf_status, count(*) from documents group by 1"
journalctl -t famat-app -f | grep -i pdf
```

Failures land on the Admin's PDF problems page, `/admin/needs-review`.

### 4. After launch

Once the import is frozen and the first backup has a restore test, remove the inputs:

```sh
rm -rf /srv/famat/import
```
