# Running `famat import` and `famat set-passcode` on the server

Both are subcommands of the app's binary. On the server, `/usr/local/sbin/famat` (from `deploy/bin/famat`) runs them in a throwaway container from the app's image. That container is in `famat-net`'s network namespace with the app's secrets, so it reaches Postgres and Garage just as the app does. Run it as root, with the containers up.

## `famat set-passcode`

```sh
famat set-passcode
```

It asks twice for the new passcode, without echo, and needs at least 8 characters. Only its argon2id hash is stored, in Postgres, so it is backed up with the database and restored with it. Setting it ends every Admin session. It is never a command-line argument; to script it, pipe it in: `famat set-passcode < file`.

Put the passcode in the password manager and give it to the owner and FAMAT's officers. To change it later, run the same command.

## `famat import`

The import builds the archive from the old site's data. Those files live on the Admin's Mac, outside git, and the SQL dump holds personal data, so copy them straight to the server and remove them after launch.

### 1. Copy the inputs to the server

From the Mac, in the repository's root:

```sh
ssh <server> 'sudo install -d -m 0700 -o $USER /srv/famat/import'
rsync -av --progress \
  "test list - 000final.csv" \
  data/labels/ai-labels.csv \
  doralmao_famat.sql \
  <server>:/srv/famat/import/
rsync -av --progress "<the docs export>/" <server>:/srv/famat/import/docs/
```

- The trailing `/` on the docs export copies its contents into `docs/`.
- rsync only sends what changed, so it is cheap to re-run after fixing labels on the Mac.
- `code.tar.gz` is not an input. Never copy it anywhere.

### 2. Dry run

Paths inside the container are under `/input`, which is `/srv/famat/import` read-only:

```sh
famat --input /srv/famat/import import \
  --sheet "/input/test list - 000final.csv" \
  --ai-labels /input/ai-labels.csv \
  --docs /input/docs \
  --legacy-sql /input/doralmao_famat.sql \
  --dry-run | tee /srv/famat/import/report-dry-run.txt
```

This only reads, and prints the reconciliation report.

### 3. The real import

Run the same command without `--dry-run`:

- Every file streams into Garage and is read back and checked before Postgres changes.
- Then one transaction replaces the archive.
- Every Word and PowerPoint Original is queued for PDF conversion. The app's two workers convert them in the background, and the site is usable meanwhile.

```sh
famat --input /srv/famat/import import \
  --sheet "/input/test list - 000final.csv" \
  --ai-labels /input/ai-labels.csv \
  --docs /input/docs \
  --legacy-sql /input/doralmao_famat.sql | tee /srv/famat/import/report-$(date +%F-%H%M).txt
```

Repeat steps 1 to 3 until the report is clean and a 20-Test spot check on `https://famat.rconde.xyz` passes. The import refuses to run once an Admin has changed anything, so finish importing before Admins start editing.

To watch the PDF backfill:

```sh
podman exec famat-postgres psql -U famat -c "select pdf_status, count(*) from documents group by 1"
journalctl -u famat-app -f | grep -i pdf
```

Failures land on the Admin's PDF problems page, `/admin/needs-review`.

### 4. After launch

Once the import is frozen and the first backup has a restore test, remove the inputs:

```sh
rm -rf /srv/famat/import
```
