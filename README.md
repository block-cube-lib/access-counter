# access-counter

A tiny, privacy-first access counter / pageview analytics for static sites,
written in **Rust** on Cloudflare Workers + D1.

You deploy it to your own Cloudflare account, paste a small JavaScript beacon
into your pages, and pageviews are counted in your own database. No external
analytics service, no cookies, no third party seeing your visitors.

## Features

- **Privacy by design**
  - No cookies, no visitor identifiers, no fingerprinting.
  - IP addresses are never stored (only the country code Cloudflare derives).
  - Raw user-agent strings are never stored — only a coarse class such as
    `Chrome / Windows`.
  - Referrer query strings are stripped before storing.
- **Bot filtering**: known bot/crawler user agents are silently dropped.
- **Prefetch/prerender safe**: the beacon waits until a prerendered page is
  actually shown before counting.
- **Self-exclusion**: visit your site once with `?nocount=1` and that browser
  is never counted again (`?nocount=0` re-enables counting).
- **SVG badge**: `GET /badge.svg` renders your total pageview count,
  shields.io style, ready to embed anywhere.
- **Daily rollup**: a `daily_stats` table is maintained next to the raw
  events, so reports are cheap SQL queries.
- Runs entirely within Cloudflare's free tier for typical personal-site
  traffic.

## Architecture

```
your pages ── beacon (POST {p, r}) ──▶ Worker ──▶ D1 (events + daily_stats)
                                          └── GET /badge.svg (total PV)
```

## Deploy

Prerequisites: a Cloudflare account, [wrangler](https://developers.cloudflare.com/workers/wrangler/),
a Rust toolchain with the `wasm32-unknown-unknown` target and
[`worker-build`](https://github.com/cloudflare/workers-rs).

```sh
# 1. Create the database
wrangler d1 create access-counter
#    → put the returned database_id into wrangler.toml

# 2. Edit wrangler.toml
#    - database_id: yours
#    - ALLOWED_ORIGINS: your site's origin(s), comma-separated
#    - TZ_OFFSET_MINUTES: your timezone offset from UTC (540 = JST)

# 3. Apply the schema and deploy
wrangler d1 migrations apply access-counter --remote
wrangler deploy
```

## Add the beacon to your site

Copy the snippet from [`examples/beacon.html`](examples/beacon.html), set
`ENDPOINT` to your worker's `/count` URL, and paste it before `</body>` on
every page. For Zola, putting it in `templates/base.html` covers the whole
site.

## Viewing the numbers

There is deliberately no dashboard in the MVP — the data is plain SQL:

```sh
# Total pageviews
wrangler d1 execute access-counter --remote \
  --command "SELECT SUM(views) AS total_pv FROM daily_stats"

# Pageviews per day (last 14 days)
wrangler d1 execute access-counter --remote \
  --command "SELECT jst_date, SUM(views) AS pv FROM daily_stats GROUP BY jst_date ORDER BY jst_date DESC LIMIT 14"

# Most viewed pages (last 30 days)
wrangler d1 execute access-counter --remote \
  --command "SELECT path, SUM(views) AS pv FROM daily_stats WHERE jst_date >= date('now','-30 days') GROUP BY path ORDER BY pv DESC LIMIT 20"

# Top referrers (last 30 days)
wrangler d1 execute access-counter --remote \
  --command "SELECT referrer, COUNT(*) AS c FROM events WHERE referrer IS NOT NULL AND ts >= unixepoch('now','-30 days') GROUP BY referrer ORDER BY c DESC LIMIT 20"
```

## Badge

```html
<img src="https://YOUR-WORKER-URL/badge.svg" alt="total pageviews">
```

If you are migrating from another counter and want the badge to continue from
your existing total, store the starting value in the `meta` table:

```sql
UPDATE meta SET value = '12345' WHERE key = 'pv_offset';
```

## Configuration (`[vars]` in wrangler.toml)

| Name                | Meaning                                                        | Default |
| ------------------- | -------------------------------------------------------------- | ------- |
| `ALLOWED_ORIGINS`   | Comma-separated origins allowed to send beacons. Empty = any.  | —       |
| `TZ_OFFSET_MINUTES` | UTC offset in minutes used for the daily rollup (540 = UTC+9). | `540`   |

## Notes and limitations

- Bot filtering is user-agent based and best-effort, like every tool in this
  class. Numbers are "close enough to be useful", not forensic truth.
- If the worker is down, pageviews for that period are simply missing; your
  site itself is unaffected.
- One deployment serves one site (the allowlist can name several origins of
  the same site). Run one deployment per site.

## License

MIT — see [LICENSE](LICENSE).
