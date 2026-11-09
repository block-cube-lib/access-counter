-- access-counter initial schema
-- One row per counted page view. No IP addresses, no cookies, no raw user agents.
CREATE TABLE events (
  id       INTEGER PRIMARY KEY,
  ts       INTEGER NOT NULL,  -- unix epoch seconds (UTC)
  jst_date TEXT NOT NULL,     -- local date 'YYYY-MM-DD' in the configured timezone
  path     TEXT NOT NULL,     -- page path, e.g. /blog/some-post/
  referrer TEXT,              -- referrer with the query string stripped
  ua_class TEXT,              -- coarse browser / OS class, e.g. 'Chrome / Windows'
  country  TEXT               -- ISO country code from Cloudflare (cf), never the IP itself
);
CREATE INDEX idx_events_date ON events (jst_date);
CREATE INDEX idx_events_path ON events (path);

-- Daily per-path rollup, kept in sync with events by the worker.
CREATE TABLE daily_stats (
  jst_date TEXT NOT NULL,
  path     TEXT NOT NULL,
  views    INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (jst_date, path)
);

-- Small key/value store for settings such as the badge's starting offset.
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
INSERT INTO meta (key, value) VALUES ('pv_offset', '0');
