# Showing what Mylar is tracking

Folio's **Upcoming releases** can show what a
[Mylar3](https://github.com/mylar3/mylar3) install is doing: this week's
pull list with each issue's status (wanted, snatched, downloaded), the
watchlist, the wanted backlog and the download queue.

This part is **view-only**. It never contacts Mylar and needs neither
Mylar's address nor its API key. A small script on the Mylar host reads
`mylar.db` read-only and pushes a snapshot to Folio.

Separately, and only if you want it, Folio can **add series to Mylar**:
see [Add to Mylar](#add-to-mylar-optional) at the end of this page.

```
mylar.db ──(read-only)──► folio-mylar-push.py ──(HTTPS POST + token)──► Folio
        on the Mylar host, every 15 minutes
```

Because the traffic only goes from Mylar's host to Folio:

- Mylar needs no port open to Folio. If a firewall keeps everything away
  from Mylar, leave it exactly as it is.
- Folio holds no Mylar credential. The ingest token Folio issues can do
  one thing: deliver a snapshot to this Folio.

## Before you start

- **Upcoming releases is on**: Admin → Upcoming releases → "Show upcoming
  releases" (it needs Metron enabled with credentials).
- The Mylar host can reach Folio's URL. Test it there:
  `curl -fsS https://folio.example.com/healthz`.
- The Mylar host has Python 3.9 or newer (`python3 --version`) and
  systemd.

## Set it up

1. **In Folio**: Admin → Upcoming releases → Mylar. Turn on **Accept Mylar
   snapshots**, save, then **Generate token**. Copy the token; it is
   shown once.
2. **On the Mylar host**, follow
   [`integrations/mylar/README.md`](../../integrations/mylar/README.md):
   copy the script and the two unit files, write the environment file
   with Folio's URL and the token, and enable the timer.
3. **Back in Folio**, the Mylar card shows "Receiving" and when the last
   snapshot arrived.

## What Folio receives

Ship dates, series and issue names, statuses, ComicVine ids, the
watchlist's ComicVine ids, queue titles and sizes, and Mylar's job times.
Never file paths, download links, indexer or provider names, or any
credential; the script does not read `config.ini`. Run
`folio-mylar-push.py --print` on the Mylar host to see the exact JSON.

## Who sees it

Mylar's watchlist covers your whole collection, and Folio answers a
hidden series exactly as it answers a missing one. So Mylar information
is shown only to admins and to users who can open every library with no
age-rating cap. The download queue, failures and backlog are admin-only.

## Staleness

The script sends when something changed, and at least once an hour. Folio
judges a snapshot by when **it** received it:

| Age | What happens |
| --- | --- |
| up to `integrations.mylar.stale_after_minutes` (default 180) | shown as current |
| older | shown with its time; the admin card warns |
| older than 7 days | Mylar information is no longer shown to readers |

## Turning it off

- **Accept Mylar snapshots** off: pushes are refused (the script logs
  `HTTP 403`) and stored Mylar data is not shown. Nothing is deleted.
- **Revoke** the token: pushes are refused (`HTTP 401`). Tick "Also
  delete the stored snapshot" to remove what Folio holds.
- On the Mylar host: `systemctl disable --now folio-mylar-push.timer`.

## Troubleshooting

`systemctl status folio-mylar-push.service` on the Mylar host shows the
last run's one-line result. The README lists what each error means.

## Add to Mylar (optional)

With Mylar's address and API key, an admin can add the series of a
reading list's missing entries to Mylar's watchlist and mark those
issues Wanted, and can check and tidy Mylar's wanted list (below). Folio
cannot delete a series, change Mylar's settings or restart it.

1. In Mylar, enable the API and copy its key (Settings → Web Interface).
2. In Folio, open **Admin → Upcoming → Mylar → Add to Mylar**, enter
   Mylar's address (for example `https://mylar.example.com/`) and the
   key, and save. **Test connection** should report Mylar's version.
3. Open a reading list → **⋯** → **Edit** → **Resolution**. Each missing entry
   has **Add to Mylar**; **Select** picks several; **Add all missing**
   takes the whole list.

Folio must be able to reach that address, so a firewall or reverse proxy
in front of Mylar has to let Folio's host through to `/api`. Mylar's key
can do anything in Mylar; Folio stores it encrypted and never shows it
again. **Remove** on the same card deletes both and turns the action off.

Entries need a ComicVine id (most published reading lists carry them).
"Mark Wanted" happens once Mylar has imported the series, which can take
several minutes per series; the row shows **Requested** until then.

Once the address and key are stored, **Admin → Upcoming** also shows a
**Mylar tools** card:

- **Check wanted list** compares what Mylar is looking for with your
  library. For issues you already have it offers **Recheck files** (Mylar
  rescans the series' folder and usually marks them downloaded),
  **Refresh** (Mylar re-reads the series from ComicVine) and **Un-want**
  (Mylar stops looking for that one issue).
- **Search now** makes Mylar search for its whole wanted list at once.

The same Refresh, Recheck files and Un-want are in the ⋯ menu of a
missing entry on a reading list's Resolution tab.

On the **Upcoming** page, an admin can open a release whose series Mylar
is not tracking and choose **Add series to Mylar**. That puts the series
on Mylar's watchlist; Mylar's own settings decide which issues it then
fetches. A release Metron has not linked to ComicVine cannot be added
this way.
