# Mylar → Folio snapshot push

Folio can show what a [Mylar3](https://github.com/mylar3/mylar3) install
is tracking: this week's pull list with each issue's status, the
watchlist, the wanted backlog and the download queue. It is **view-only**.
Folio never calls Mylar and cannot change anything in it.

The link is this one script. It runs **on the Mylar host**, opens
`mylar.db` read-only, and POSTs a snapshot to Folio over HTTPS.

```
mylar.db ──(read-only)──► folio-mylar-push.py ──(HTTPS POST + token)──► Folio
```

So Mylar needs no open port, Folio holds no Mylar API key, and a firewall
that keeps everything away from Mylar can stay exactly as it is.

## What is sent

| Block | From Mylar's table | Fields |
| --- | --- | --- |
| `pull_list` | `weekly`, ten days back to four weeks ahead | ship date, publisher, series, issue, status, ComicVine series and issue ids, series year |
| `watchlist` | `comics` | ComicVine series id, status, still-publishing flag |
| `wanted` | `issues` with status Wanted / Snatched / Failed | ComicVine ids, series, issue, store date, status, date added |
| `history` | `snatched`, last 14 days | ComicVine ids, series, issue, status, time |
| `queue` | `ddl_info`, not completed, newest 200 | title, year, size, status, time; counts per status |
| `jobs` | `jobhistory` | name, status, previous and next run |

**Never sent:** file paths, download links, indexer or provider names,
NZB names, hashes, or any credential. The script does not read
`config.ini` and never calls Mylar's HTTP API. Run it with `--print` to
see the exact JSON, or `--dry-run` for a one-line summary; neither sends
anything.

Annuals are left out unless `FOLIO_MYLAR_INCLUDE_ANNUALS=1`. Set it only
when Mylar's annual integration is on; otherwise old annual rows would
show up as wanted.

## Install

1. In Folio: **Admin → Upcoming releases → Mylar**, turn on "Accept Mylar
   snapshots" and generate a token. It is shown once.
2. On the Mylar host, as root:

   ```sh
   install -d /usr/local/lib/folio-mylar-push
   install -m 0755 folio-mylar-push.py /usr/local/lib/folio-mylar-push/
   install -m 0644 folio-mylar-push.service folio-mylar-push.timer /etc/systemd/system/
   umask 077
   cat > /etc/folio-mylar-push.env <<'ENV'
   FOLIO_URL=https://folio.example.com
   FOLIO_TOKEN=fmi_paste-the-token-here
   # MYLAR_DB=/opt/mylar3-data/mylar.db
   # FOLIO_MYLAR_INCLUDE_ANNUALS=1
   ENV
   systemctl daemon-reload
   systemctl enable --now folio-mylar-push.timer
   systemctl start folio-mylar-push.service   # first push now
   systemctl status folio-mylar-push.service --no-pager
   ```

3. Back in Folio, the Mylar card shows the snapshot's time within a few
   seconds.

If `mylar.db` is not at `/opt/mylar3-data/mylar.db`, set `MYLAR_DB` and
change `ReadOnlyPaths=` in the service file to its directory.

The unit runs as a throwaway user (`DynamicUser=yes`), so `mylar.db` and
its directory must be readable by "other" (`644` / `755`, Mylar's
default). If yours are not, add `SupplementaryGroups=<mylar's group>` to
the service instead of loosening the modes.

## How often

Every 15 minutes (`OnCalendar=*:0/15`). The script hashes the snapshot
and sends only when it differs from the last accepted one, plus one
heartbeat an hour so Folio can tell a quiet Mylar from a stopped timer.
Folio marks the snapshot stale after 180 minutes without one (a setting).

## When something is wrong

| `systemctl status` shows | Meaning |
| --- | --- |
| `HTTP 401` | The token is wrong or was revoked. Generate a new one. |
| `HTTP 403` | "Accept Mylar snapshots" is off in Folio. |
| `HTTP 422` | Folio is older than this script's snapshot format, or newer and no longer accepts it. Use the script from the Folio version you run. |
| `mylar.db is missing …` | This Mylar version changed a table the script reads. Nothing was sent. |
| `could not read mylar.db` | Mylar was mid-write. The next run retries. |

## Uninstall

```sh
systemctl disable --now folio-mylar-push.timer
rm /etc/systemd/system/folio-mylar-push.{service,timer} /etc/folio-mylar-push.env
rm -r /usr/local/lib/folio-mylar-push
systemctl daemon-reload
```

Then revoke the token in Folio. Revoking can also delete the stored
snapshot.

## Tests

```sh
python3 -m unittest -v     # in this directory; builds its own SQLite fixture
```
