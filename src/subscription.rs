//! User subscriptions: remote share-link feeds and manually pasted keys,
//! together with the service information panels publish in HTTP headers.

use crate::parser::{decode_base64_text, looks_like_share_link, parse_subscription};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_USER_AGENT: &str = "v2rayN/7.10.0";
pub const DEFAULT_UPDATE_HOURS: u32 = 12;
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Information a provider publishes about the subscription.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubscriptionMeta {
    pub title: String,
    pub has_traffic: bool,
    pub upload: u64,
    pub download: u64,
    /// Traffic limit in bytes, `0` means unlimited.
    pub total: u64,
    /// Expiry as a unix timestamp, `0` means it never expires.
    pub expire: u64,
    pub update_interval_hours: u32,
    pub support_url: String,
    pub web_page_url: String,
    pub announce: String,
    /// Text the provider placed into fake "servers" (days left, traffic…).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Subscription {
    pub id: String,
    /// Name chosen by the user. Empty means "use the provider title".
    pub name: String,
    /// Remote subscription address. Empty for manually added keys.
    pub url: String,
    /// Manually pasted keys (one share link per line).
    pub keys: String,
    pub enabled: bool,
    pub auto_update: bool,
    pub meta: SubscriptionMeta,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_error: String,
    pub node_count: usize,
    pub skipped: usize,
}

impl Default for Subscription {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            keys: String::new(),
            enabled: true,
            auto_update: true,
            meta: SubscriptionMeta::default(),
            created_at: 0,
            updated_at: 0,
            last_error: String::new(),
            node_count: 0,
            skipped: 0,
        }
    }
}

impl Subscription {
    pub fn remote(name: &str, url: &str) -> Self {
        Self {
            id: new_id(url),
            name: name.trim().to_owned(),
            url: url.trim().to_owned(),
            created_at: now_unix(),
            ..Self::default()
        }
    }

    pub fn manual(name: &str, keys: &str) -> Self {
        Self {
            id: new_id(keys),
            name: name.trim().to_owned(),
            keys: keys.trim().to_owned(),
            auto_update: false,
            created_at: now_unix(),
            updated_at: now_unix(),
            ..Self::default()
        }
    }

    pub fn is_remote(&self) -> bool {
        !self.url.trim().is_empty()
    }

    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.trim().to_owned();
        }
        if !self.meta.title.trim().is_empty() {
            return self.meta.title.trim().to_owned();
        }
        if self.is_remote() {
            if let Some(host) = url::Url::parse(self.url.trim())
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
            {
                return host;
            }
            return "Подписка".to_owned();
        }
        "Мои ключи".to_owned()
    }

    pub fn traffic_used(&self) -> u64 {
        self.meta.upload.saturating_add(self.meta.download)
    }

    /// Share of the traffic limit already used, `None` for unlimited plans.
    pub fn traffic_ratio(&self) -> Option<f32> {
        (self.meta.has_traffic && self.meta.total > 0)
            .then(|| (self.traffic_used() as f64 / self.meta.total as f64).clamp(0.0, 1.0) as f32)
    }

    pub fn seconds_left(&self, now: u64) -> Option<i64> {
        (self.meta.expire > 0).then(|| self.meta.expire as i64 - now as i64)
    }

    pub fn update_interval_hours(&self, default_hours: u32) -> u32 {
        if self.meta.update_interval_hours > 0 {
            self.meta.update_interval_hours.clamp(1, 24 * 7)
        } else {
            default_hours.max(1)
        }
    }

    pub fn is_due(&self, now: u64, default_hours: u32) -> bool {
        self.is_remote()
            && self.enabled
            && self.auto_update
            && now.saturating_sub(self.updated_at)
                >= u64::from(self.update_interval_hours(default_hours)) * 3600
    }

    /// Whether the plan needs the user's attention (expires soon / almost no traffic).
    pub fn warning(&self, now: u64) -> Option<String> {
        if let Some(seconds) = self.seconds_left(now) {
            if seconds <= 0 {
                return Some("Подписка истекла".to_owned());
            }
            if seconds < 3 * 86_400 {
                return Some(format!(
                    "Подписка истекает через {}",
                    format_remaining(seconds)
                ));
            }
        }
        if let Some(ratio) = self.traffic_ratio() {
            if ratio >= 1.0 {
                return Some("Трафик закончился".to_owned());
            }
            if ratio >= 0.9 {
                return Some(format!(
                    "Осталось {} трафика",
                    format_bytes(self.meta.total.saturating_sub(self.traffic_used()))
                ));
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
pub struct FetchedSubscription {
    pub body: String,
    pub meta: SubscriptionMeta,
}

/// What the user typed into the "add" field.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportInput {
    Empty,
    Url(String),
    Keys { count: usize, summary: String },
    Unknown,
}

pub fn classify_import(text: &str) -> ImportInput {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ImportInput::Empty;
    }
    if let Some(url) = extract_subscription_url(trimmed) {
        return ImportInput::Url(url);
    }
    let report = parse_subscription(trimmed);
    let nodes: Vec<_> = report
        .nodes
        .iter()
        .filter(|node| !node.is_placeholder())
        .collect();
    if nodes.is_empty() {
        return ImportInput::Unknown;
    }
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for node in &nodes {
        let label = node.protocol.label();
        match counts.iter_mut().find(|(name, _)| *name == label) {
            Some((_, count)) => *count += 1,
            None => counts.push((label, 1)),
        }
    }
    let summary = counts
        .iter()
        .map(|(name, count)| format!("{count} {name}"))
        .collect::<Vec<_>>()
        .join(" · ");
    ImportInput::Keys {
        count: nodes.len(),
        summary,
    }
}

/// Accepts plain http(s) links and the import deep links other clients use.
pub fn extract_subscription_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.lines().count() > 1 || looks_like_share_link(text) {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return url::Url::parse(text).ok().map(|url| url.to_string());
    }
    for prefix in [
        "happ://add/",
        "v2raytun://import/",
        "hiddify://import/",
        "streisand://import/",
        "clash://install-config?url=",
        "sing-box://import-remote-profile?url=",
    ] {
        if lower.starts_with(prefix) {
            let rest = &text[prefix.len()..];
            let rest = rest.split('#').next().unwrap_or_default();
            let decoded = percent_encoding::percent_decode_str(rest)
                .decode_utf8_lossy()
                .into_owned();
            return extract_subscription_url(&decoded);
        }
    }
    None
}

/// Downloads a subscription. `device` adds the HWID headers that panels with
/// device limits require (the same set Happ and Incy send).
pub fn fetch_subscription(
    url: &str,
    user_agent: &str,
    device: Option<&crate::device::DeviceInfo>,
) -> Result<FetchedSubscription, String> {
    let user_agent = if user_agent.trim().is_empty() {
        DEFAULT_USER_AGENT
    } else {
        user_agent.trim()
    };
    let client = reqwest::blocking::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client.get(url.trim());
    if let Some(device) = device {
        for (name, value) in device.headers() {
            request = request.header(name, value);
        }
    }
    let response = request
        .send()
        .map_err(|error| format!("Не удалось загрузить подписку: {}", short_error(&error)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => format!("Доступ запрещён ({status}) — проверьте ссылку"),
            404 => "Подписка не найдена (404) — ссылка устарела или удалена".to_owned(),
            _ => format!("Сервер подписки вернул ошибку {status}"),
        });
    }
    let mut meta = SubscriptionMeta::default();
    apply_headers(
        &mut meta,
        response.headers().iter().map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        }),
    );
    let bytes = response
        .bytes()
        .map_err(|error| format!("Не удалось прочитать подписку: {}", short_error(&error)))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err("Подписка больше 10 МБ и была отклонена".to_owned());
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let body = crate::parser::decode_subscription_text(&text);
    extract_body_meta(&body, &mut meta);
    let report = parse_subscription(&body);
    if report.nodes.is_empty() {
        let head = text.trim_start();
        return Err(if head.starts_with("proxies:") {
            "Сервер вернул Clash-конфиг — укажите в настройках другой User-Agent подписок"
                .to_owned()
        } else if head.starts_with('{') || head.starts_with('[') {
            "В JSON-конфиге нет поддерживаемых серверов".to_owned()
        } else if head.starts_with('<') {
            "Вместо подписки пришла HTML-страница — проверьте ссылку".to_owned()
        } else {
            "В подписке не найдено поддерживаемых серверов".to_owned()
        });
    }
    Ok(FetchedSubscription { body, meta })
}

fn short_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "превышено время ожидания".to_owned()
    } else if error.is_connect() {
        "нет соединения с сервером".to_owned()
    } else {
        error.to_string()
    }
}

pub fn apply_headers(meta: &mut SubscriptionMeta, headers: impl Iterator<Item = (String, String)>) {
    for (name, value) in headers {
        apply_meta_field(meta, &name, &value);
        if name == "content-disposition" && meta.title.is_empty() {
            if let Some(name) = content_disposition_filename(&value) {
                meta.title = name;
            }
        }
    }
}

fn apply_meta_field(meta: &mut SubscriptionMeta, name: &str, value: &str) {
    match name.trim().to_ascii_lowercase().as_str() {
        "subscription-userinfo" => parse_userinfo(meta, value),
        "profile-title" => meta.title = decode_header_text(value),
        "profile-update-interval" => {
            meta.update_interval_hours = value.trim().parse().unwrap_or_default()
        }
        "support-url" => meta.support_url = value.trim().to_owned(),
        "profile-web-page-url" => meta.web_page_url = value.trim().to_owned(),
        "announce" => meta.announce = decode_header_text(value),
        _ => {}
    }
}

/// `upload=1; download=2; total=3; expire=4`
pub fn parse_userinfo(meta: &mut SubscriptionMeta, value: &str) {
    for part in value.split(';') {
        let Some((key, number)) = part.split_once('=') else {
            continue;
        };
        let number = number.trim();
        let parsed = number
            .parse::<u64>()
            .ok()
            .or_else(|| {
                number
                    .parse::<f64>()
                    .ok()
                    .map(|value| value.max(0.0) as u64)
            })
            .unwrap_or_default();
        match key.trim().to_ascii_lowercase().as_str() {
            "upload" => meta.upload = parsed,
            "download" => meta.download = parsed,
            "total" => meta.total = parsed,
            "expire" => meta.expire = parsed,
            _ => continue,
        }
        meta.has_traffic = true;
    }
}

/// Header values may be plain text or `base64:...`.
pub fn decode_header_text(value: &str) -> String {
    let value = value.trim();
    let decoded = match value.strip_prefix("base64:") {
        Some(payload) => decode_base64_text(payload).unwrap_or_default(),
        None => percent_encoding::percent_decode_str(value)
            .decode_utf8_lossy()
            .into_owned(),
    };
    // Variation selectors render as wide blanks in the UI font.
    decoded.replace('\u{FE0F}', "").trim().to_owned()
}

fn content_disposition_filename(value: &str) -> Option<String> {
    for part in value.split(';') {
        let part = part.trim();
        if let Some(encoded) = part.strip_prefix("filename*=") {
            let encoded = encoded.trim_matches('"');
            let encoded = encoded.split("''").last().unwrap_or(encoded);
            let name = percent_encoding::percent_decode_str(encoded)
                .decode_utf8_lossy()
                .into_owned();
            return Some(name).filter(|name| !name.trim().is_empty());
        }
    }
    for part in value.split(';') {
        if let Some(name) = part.trim().strip_prefix("filename=") {
            let name = name.trim_matches('"').trim().to_owned();
            return Some(name).filter(|name| !name.is_empty());
        }
    }
    None
}

/// Some providers repeat the headers as `#key: value` lines inside the body.
pub fn extract_body_meta(body: &str, meta: &mut SubscriptionMeta) {
    for line in body.lines() {
        let Some(comment) = line.trim().strip_prefix('#') else {
            continue;
        };
        if let Some((key, value)) = comment.split_once(':') {
            let key = key.trim().to_ascii_lowercase();
            let empty = match key.as_str() {
                "profile-title" => meta.title.is_empty(),
                "announce" => meta.announce.is_empty(),
                "support-url" => meta.support_url.is_empty(),
                "profile-web-page-url" => meta.web_page_url.is_empty(),
                "profile-update-interval" => meta.update_interval_hours == 0,
                "subscription-userinfo" => !meta.has_traffic,
                _ => false,
            };
            if empty {
                apply_meta_field(meta, &key, value);
            }
        }
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub fn new_id(seed: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let digest = Sha256::digest(format!("{seed}\0{nanos}").as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_remaining(seconds: i64) -> String {
    if seconds <= 0 {
        return "0 дней".to_owned();
    }
    if seconds >= 86_400 {
        // Round to the nearest day: "22 days 23 hours" reads as 23 days.
        let days = (seconds + 43_200) / 86_400;
        return format!("{days} {}", plural(days, "день", "дня", "дней"));
    }
    let hours = (seconds / 3600).max(1);
    format!("{hours} {}", plural(hours, "час", "часа", "часов"))
}

pub fn plural<'a>(value: i64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let value = value.unsigned_abs();
    if value % 100 / 10 == 1 {
        return many;
    }
    match value % 10 {
        1 => one,
        2..=4 => few,
        _ => many,
    }
}

/// Formats a unix timestamp as a local-agnostic `DD.MM.YYYY` (UTC) date.
pub fn format_date(timestamp: u64) -> String {
    let days = (timestamp / 86_400) as i64;
    // Civil-from-days (Howard Hinnant), valid for the whole u64 range we use.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day:02}.{month:02}.{year}")
}

pub fn format_ago(timestamp: u64, now: u64) -> String {
    if timestamp == 0 {
        return "ещё не обновлялась".to_owned();
    }
    let seconds = now.saturating_sub(timestamp) as i64;
    if seconds < 60 {
        "только что".to_owned()
    } else if seconds < 3600 {
        let minutes = seconds / 60;
        format!(
            "{minutes} {} назад",
            plural(minutes, "минуту", "минуты", "минут")
        )
    } else if seconds < 86_400 {
        let hours = seconds / 3600;
        format!("{hours} {} назад", plural(hours, "час", "часа", "часов"))
    } else {
        let days = seconds / 86_400;
        format!("{days} {} назад", plural(days, "день", "дня", "дней"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn parses_userinfo_and_titles() {
        let mut meta = SubscriptionMeta::default();
        apply_headers(
            &mut meta,
            [
                (
                    "subscription-userinfo".to_owned(),
                    "upload=1024; download=2048; total=10737418240; expire=1893456000".to_owned(),
                ),
                (
                    "profile-title".to_owned(),
                    format!(
                        "base64:{}",
                        base64::engine::general_purpose::STANDARD.encode("Мой VPN")
                    ),
                ),
                ("profile-update-interval".to_owned(), "6".to_owned()),
                ("announce".to_owned(), "Привет%20мир".to_owned()),
            ]
            .into_iter(),
        );
        assert!(meta.has_traffic);
        assert_eq!(meta.upload, 1024);
        assert_eq!(meta.total, 10_737_418_240);
        assert_eq!(meta.expire, 1_893_456_000);
        assert_eq!(meta.title, "Мой VPN");
        assert_eq!(meta.update_interval_hours, 6);
        assert_eq!(meta.announce, "Привет мир");
    }

    #[test]
    fn reads_meta_from_body_comments() {
        let mut meta = SubscriptionMeta::default();
        extract_body_meta(
            "#profile-title: Body title\n#subscription-userinfo: upload=0; download=5; total=0; expire=0\nvless://a@b:1",
            &mut meta,
        );
        assert_eq!(meta.title, "Body title");
        assert!(meta.has_traffic);
        assert_eq!(meta.download, 5);
    }

    #[test]
    fn classifies_import_input() {
        assert_eq!(
            classify_import("https://sub.example.com/abc"),
            ImportInput::Url("https://sub.example.com/abc".to_owned())
        );
        assert_eq!(
            classify_import("happ://add/https://sub.example.com/abc"),
            ImportInput::Url("https://sub.example.com/abc".to_owned())
        );
        match classify_import(
            "vless://1234@example.org:443?security=tls#A\ntrojan://pw@example.net:443#B",
        ) {
            ImportInput::Keys { count, .. } => assert_eq!(count, 2),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(classify_import("hello"), ImportInput::Unknown);
        assert_eq!(classify_import("  "), ImportInput::Empty);
    }

    #[test]
    fn subscription_state_helpers() {
        let mut subscription = Subscription::remote("", "https://sub.example.com/x");
        assert_eq!(subscription.display_name(), "sub.example.com");
        subscription.meta.title = "Provider".to_owned();
        assert_eq!(subscription.display_name(), "Provider");
        subscription.meta.has_traffic = true;
        subscription.meta.total = 100;
        subscription.meta.download = 95;
        assert!(subscription.warning(0).is_some());
        subscription.updated_at = 1000;
        assert!(!subscription.is_due(1000 + 3600, 12));
        assert!(subscription.is_due(1000 + 13 * 3600, 12));
    }

    #[test]
    fn formats_values() {
        assert_eq!(format_bytes(1536), "1.5 КБ");
        assert_eq!(format_bytes(10_737_418_240), "10.0 ГБ");
        assert_eq!(format_remaining(5 * 86_400 + 10), "5 дней");
        assert_eq!(format_remaining(86_400), "1 день");
        assert_eq!(format_date(1_893_456_000), "01.01.2030");
    }
}
