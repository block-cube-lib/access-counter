//! access-counter: a tiny, privacy-first pageview counter for static sites.
//!
//! A small JavaScript beacon on each page POSTs `{p, r}` (path, referrer) to
//! `POST /count`. The worker validates the origin, drops known bots, and
//! stores one row per view in D1 plus a daily per-path rollup.
//! `GET /badge.svg` renders the total pageview count as an SVG badge.
//!
//! Privacy by design: no cookies, no IP addresses stored (only the country
//! code Cloudflare derives from it), no raw user-agent strings (only a coarse
//! browser/OS class), and referrer query strings are stripped.

use serde::Deserialize;
use wasm_bindgen::JsValue;
use worker::{event, Context, Env, Headers, Request, Response, Result};

const BOT_MARKERS: &[&str] = &[
    "bot",
    "crawler",
    "spider",
    "slurp",
    "curl",
    "wget",
    "python",
    "headless",
    "scraper",
    "facebookexternalhit",
    "pingdom",
    "lighthouse",
    "prerender",
];

#[derive(Deserialize)]
struct Beacon {
    /// Page path, e.g. "/blog/some-post/".
    p: String,
    /// document.referrer (may be empty).
    #[serde(default)]
    r: String,
}

#[derive(Deserialize)]
struct TotalRow {
    total: f64,
}

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    let method = req.method();
    let path = req.path();

    match (method, path.as_str()) {
        (worker::Method::Post, "/count") => handle_count(req, env).await,
        (worker::Method::Options, "/count") => handle_preflight(req, env),
        (worker::Method::Get, "/badge.svg") => handle_badge(env).await,
        (worker::Method::Get, "/") => {
            Response::ok("access-counter is running. POST /count, GET /badge.svg\n")
        }
        _ => Response::error("not found", 404),
    }
}

fn allowed_origins(env: &Env) -> Vec<String> {
    env.var("ALLOWED_ORIGINS")
        .map(|v| {
            v.to_string()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn origin_allowed(req: &Request, allowed: &[String]) -> Option<String> {
    if allowed.is_empty() {
        // No allowlist configured: accept, echoing back the request origin if any.
        return req.headers().get("origin").ok().flatten();
    }
    if let Ok(Some(origin)) = req.headers().get("origin") {
        if allowed.iter().any(|a| a == &origin) {
            return Some(origin);
        }
    }
    if let Ok(Some(referer)) = req.headers().get("referer") {
        // Compare the referer's *origin* exactly — a prefix match would let
        // "https://example.com.evil.tld" pass for "https://example.com".
        if let Ok(url) = worker::Url::parse(&referer) {
            let origin = url.origin().ascii_serialization();
            if allowed.iter().any(|a| a == &origin) {
                return Some(origin);
            }
        }
    }
    None
}

fn tz_offset_minutes(env: &Env) -> i64 {
    env.var("TZ_OFFSET_MINUTES")
        .ok()
        .and_then(|v| v.to_string().parse::<i64>().ok())
        .unwrap_or(540)
}

fn is_bot(ua: &str) -> bool {
    let lower = ua.to_lowercase();
    BOT_MARKERS.iter().any(|m| lower.contains(m))
}

/// Map a user-agent string to a coarse "Browser / OS" class.
/// The raw string is never stored.
fn ua_class(ua: &str) -> String {
    let browser = if ua.contains("Edg/") || ua.contains("EdgA/") || ua.contains("EdgiOS/") {
        "Edge"
    } else if ua.contains("OPR/") || ua.contains("Opera") || ua.contains("OPiOS/") {
        "Opera"
    } else if ua.contains("SamsungBrowser") {
        "Samsung Internet"
    } else if ua.contains("Firefox/") || ua.contains("FxiOS/") {
        "Firefox"
    } else if ua.contains("Chrome/") || ua.contains("CriOS") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "Other"
    };
    let os = if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        "macOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Other"
    };
    format!("{browser} / {os}")
}

/// Strip the query string and fragment from a referrer URL.
fn clean_referrer(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let end = trimmed
        .find(|c| c == '?' || c == '#')
        .unwrap_or(trimmed.len());
    let cleaned = &trimmed[..end];
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.chars().take(512).collect())
    }
}

/// Convert days since the unix epoch to a (year, month, day) civil date.
/// Howard Hinnant's algorithm, public domain.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn local_date(unix_secs: i64, offset_minutes: i64) -> String {
    let local = unix_secs + offset_minutes * 60;
    let days = local.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn cors_headers(origin: Option<String>) -> Result<Headers> {
    let headers = Headers::new();
    if let Some(o) = origin {
        headers.set("Access-Control-Allow-Origin", &o)?;
    }
    headers.set("Vary", "Origin")?;
    Ok(headers)
}

fn handle_preflight(req: Request, env: Env) -> Result<Response> {
    let origin = origin_allowed(&req, &allowed_origins(&env));
    let headers = cors_headers(origin)?;
    headers.set("Access-Control-Allow-Methods", "POST, OPTIONS")?;
    headers.set("Access-Control-Allow-Headers", "content-type")?;
    headers.set("Access-Control-Max-Age", "86400")?;
    Ok(Response::empty()?.with_headers(headers).with_status(204))
}

async fn handle_count(mut req: Request, env: Env) -> Result<Response> {
    let allowed = allowed_origins(&env);
    let origin = origin_allowed(&req, &allowed);
    if !allowed.is_empty() && origin.is_none() {
        return Response::error("forbidden origin", 403);
    }

    let ua = req.headers().get("user-agent").ok().flatten().unwrap_or_default();
    let headers = cors_headers(origin)?;

    // Silently drop bots: answer 204 so nothing can be probed from responses.
    if is_bot(&ua) {
        return Ok(Response::empty()?.with_headers(headers).with_status(204));
    }

    // Beacons are tiny; refuse oversized bodies instead of buffering them.
    // Fail closed: an unparsable Content-Length drops the request too.
    if let Ok(Some(len)) = req.headers().get("content-length") {
        if len.parse::<usize>().map_or(true, |n| n > 4096) {
            return Ok(Response::empty()?.with_headers(headers).with_status(204));
        }
    }
    let body = req.text().await.unwrap_or_default();
    if body.len() > 4096 {
        return Ok(Response::empty()?.with_headers(headers).with_status(204));
    }
    let beacon: Beacon = match serde_json::from_str(&body) {
        Ok(b) => b,
        Err(_) => return Ok(Response::empty()?.with_headers(headers).with_status(204)),
    };
    if !beacon.p.starts_with('/')
        || beacon.p.len() > 512
        || beacon.p.contains(|c: char| c == '?' || c == '#' || c.is_control())
    {
        return Ok(Response::empty()?.with_headers(headers).with_status(204));
    }

    let ts = (worker::Date::now().as_millis() / 1000) as i64;
    let date = local_date(ts, tz_offset_minutes(&env));
    let referrer = clean_referrer(&beacon.r);
    let ua_class = ua_class(&ua);
    let country = req.cf().and_then(|cf| cf.country());

    let db = env.d1("DB")?;
    let stmt_events = db
        .prepare(
            "INSERT INTO events (ts, jst_date, path, referrer, ua_class, country) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&[
            JsValue::from_f64(ts as f64),
            JsValue::from_str(&date),
            JsValue::from_str(&beacon.p),
            match &referrer {
                Some(r) => JsValue::from_str(r),
                None => JsValue::NULL,
            },
            JsValue::from_str(&ua_class),
            match &country {
                Some(c) => JsValue::from_str(c),
                None => JsValue::NULL,
            },
        ])?;
    let stmt_daily = db
        .prepare(
            "INSERT INTO daily_stats (jst_date, path, views) VALUES (?1, ?2, 1) \
             ON CONFLICT (jst_date, path) DO UPDATE SET views = views + 1",
        )
        .bind(&[JsValue::from_str(&date), JsValue::from_str(&beacon.p)])?;
    // One batch = one transaction, so events and the rollup cannot drift apart.
    db.batch(vec![stmt_events, stmt_daily]).await?;

    Ok(Response::empty()?.with_headers(headers).with_status(204))
}

async fn handle_badge(env: Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let total: f64 = db
        .prepare(
            "SELECT COALESCE((SELECT SUM(views) FROM daily_stats), 0) \
                  + COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'pv_offset'), 0) AS total",
        )
        .first::<TotalRow>(None)
        .await?
        .map(|row| row.total)
        .unwrap_or(0.0);
    let total = total.max(0.0) as u64;

    let digits = total.to_string();
    // shields.io-style badge: dark label segment + blue value segment.
    let label = "PV";
    let label_w = 26;
    let value_w = digits.len() as u64 * 7 + 12;
    let width = label_w + value_w;
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"20\" role=\"img\" aria-label=\"PV: {digits}\">\
<title>PV: {digits}</title>\
<rect width=\"{label_w}\" height=\"20\" fill=\"#555\"/>\
<rect x=\"{label_w}\" width=\"{value_w}\" height=\"20\" fill=\"#007ec6\"/>\
<g fill=\"#fff\" font-family=\"Verdana,DejaVu Sans,sans-serif\" font-size=\"11\">\
<text x=\"{lx}\" y=\"14\" text-anchor=\"middle\">{label}</text>\
<text x=\"{vx}\" y=\"14\" text-anchor=\"middle\">{digits}</text>\
</g></svg>",
        lx = label_w / 2,
        vx = label_w + value_w / 2,
    );

    let headers = Headers::new();
    headers.set("Content-Type", "image/svg+xml")?;
    headers.set("Cache-Control", "public, max-age=60")?;
    headers.set("Cross-Origin-Resource-Policy", "cross-origin")?;
    Ok(Response::ok(svg)?.with_headers(headers))
}
