use base64::Engine as _;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Protocol {
    Vless,
    Vmess,
    Hysteria2,
    Trojan,
    Shadowsocks,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vless => "VLESS",
            Self::Vmess => "VMess",
            Self::Hysteria2 => "Hysteria2",
            Self::Trojan => "Trojan",
            Self::Shadowsocks => "Shadowsocks",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Vless => "VLESS",
            Self::Vmess => "VMess",
            Self::Hysteria2 => "HY2",
            Self::Trojan => "Trojan",
            Self::Shadowsocks => "SS",
        }
    }

    pub fn scheme(self) -> &'static str {
        match self {
            Self::Vless => "vless",
            Self::Vmess => "vmess",
            Self::Hysteria2 => "hysteria2",
            Self::Trojan => "trojan",
            Self::Shadowsocks => "ss",
        }
    }

    /// UDP-only transports cannot be probed with a TCP handshake.
    pub fn is_udp(self) -> bool {
        matches!(self, Self::Hysteria2)
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnNode {
    pub raw: String,
    pub protocol: Protocol,
    pub name: String,
    pub country: String,
    pub flag: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub transport: String,
    pub security: String,
    pub flow: Option<String>,
    pub whitelist: bool,
    pub params: BTreeMap<String, String>,
    /// Identifier of the subscription (or key list) the node was imported from.
    #[serde(default)]
    pub source: String,
}

impl VpnNode {
    pub fn endpoint(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn searchable_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.name,
            self.country,
            self.host,
            self.protocol.label(),
            self.transport,
            self.security
        )
        .to_lowercase()
    }

    pub fn parameter(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(String::as_str)
    }

    /// Panels often publish "nodes" that only carry text such as the remaining
    /// days or traffic. They point to an unroutable address and must not be
    /// offered as servers.
    pub fn is_placeholder(&self) -> bool {
        let host = self.host.trim_matches(|c| c == '[' || c == ']');
        // Remnawave and others answer with a zero-UUID "server" carrying the
        // message, e.g. when the device ID (HWID) is missing.
        let zero_user = self.user == "00000000-0000-0000-0000-000000000000";
        zero_user
            || self.port <= 1
            || matches!(host, "0.0.0.0" | "127.0.0.1" | "::" | "::1" | "localhost")
    }

    /// Display name without a leading flag emoji.
    pub fn title(&self) -> &str {
        let rest = self
            .name
            .trim_start()
            .trim_start_matches(|c: char| {
                ('\u{1F1E6}'..='\u{1F1FF}').contains(&c) || c == '\u{FE0F}'
            })
            .trim();
        if rest.is_empty() {
            &self.name
        } else {
            rest
        }
    }
}

#[derive(Debug, Default)]
pub struct ParseReport {
    pub nodes: Vec<VpnNode>,
    pub ignored: usize,
    pub duplicates: usize,
}

/// Parses a plain or base64-encoded list of share links. The provider order is
/// preserved because subscriptions usually list their best servers first.
pub fn parse_subscription(input: &str) -> ParseReport {
    let decoded = decode_subscription_text(input);
    let mut report = ParseReport::default();
    let mut seen = HashSet::new();

    for (index, line) in decoded.lines().enumerate() {
        let raw = line.trim().trim_start_matches('\u{feff}');
        if raw.is_empty() || raw.starts_with('#') || raw.starts_with("//") {
            continue;
        }

        if !seen.insert(raw.to_owned()) {
            report.duplicates += 1;
            continue;
        }

        match parse_node(raw, index + 1) {
            Ok(node) => report.nodes.push(node),
            Err(_) => report.ignored += 1,
        }
    }
    report
}

/// Turns a subscription body into share links: JSON configs are converted,
/// plain lists are returned as-is and base64 bodies are decoded.
pub fn decode_subscription_text(input: &str) -> String {
    let trimmed = input.trim().trim_start_matches('\u{feff}');
    // Xray / sing-box JSON configs (they may contain https:// links themselves).
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        if let Some(links) = crate::json_config::json_to_links(trimmed) {
            return links.join("\n");
        }
    }
    if trimmed.contains("://") {
        return trimmed.to_owned();
    }
    let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    match decode_base64_text(&compact) {
        Some(text) if text.contains("://") => text,
        _ => trimmed.to_owned(),
    }
}

pub fn decode_base64_text(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let engines = [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        &base64::engine::general_purpose::URL_SAFE,
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ];
    let unpadded = value.trim_end_matches('=');
    for engine in engines {
        for candidate in [value, unpadded] {
            if let Ok(bytes) = engine.decode(candidate) {
                if let Ok(text) = String::from_utf8(bytes) {
                    return Some(text);
                }
            }
        }
    }
    None
}

pub fn looks_like_share_link(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    [
        "vless://",
        "vmess://",
        "trojan://",
        "hysteria2://",
        "hy2://",
        "ss://",
    ]
    .iter()
    .any(|scheme| lower.starts_with(scheme))
}

fn parse_node(raw: &str, line_number: usize) -> Result<VpnNode, String> {
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("vmess://") {
        return parse_vmess(raw, line_number);
    }
    if lower.starts_with("ss://") {
        return parse_shadowsocks(raw, line_number);
    }
    let parsed = Url::parse(raw).map_err(|error| format!("line {line_number}: {error}"))?;
    let protocol = match parsed.scheme().to_ascii_lowercase().as_str() {
        "vless" => Protocol::Vless,
        "hysteria2" | "hy2" => Protocol::Hysteria2,
        "trojan" => Protocol::Trojan,
        other => return Err(format!("line {line_number}: unsupported scheme {other}")),
    };

    let host = parsed
        .host_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("line {line_number}: missing host"))?
        .trim_matches(|c| c == '[' || c == ']')
        .to_owned();
    let port = parsed
        .port()
        .ok_or_else(|| format!("line {line_number}: missing port"))?;
    let user = decode(parsed.username());
    if user.is_empty() {
        return Err(format!("line {line_number}: missing user/password"));
    }
    let password = parsed.password().map(decode);

    let mut params = BTreeMap::new();
    for (key, value) in parsed.query_pairs() {
        params.insert(key.into_owned(), value.into_owned());
    }

    let fragment = parsed.fragment().map(decode).unwrap_or_default();
    let transport = match protocol {
        Protocol::Hysteria2 => "QUIC".to_owned(),
        _ => params
            .get("type")
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_else(|| "tcp".to_owned()),
    };
    let security = match protocol {
        Protocol::Vless => params
            .get("security")
            .cloned()
            .or_else(|| params.contains_key("pbk").then(|| "reality".to_owned()))
            .unwrap_or_else(|| "none".to_owned()),
        _ => params
            .get("security")
            .cloned()
            .unwrap_or_else(|| "tls".to_owned()),
    };
    let flow = params.get("flow").filter(|v| !v.is_empty()).cloned();

    Ok(finish_node(NodeDraft {
        raw,
        protocol,
        fragment: &fragment,
        line_number,
        host,
        port,
        user,
        password,
        transport,
        security,
        flow,
        params,
    }))
}

struct NodeDraft<'a> {
    raw: &'a str,
    protocol: Protocol,
    fragment: &'a str,
    line_number: usize,
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    transport: String,
    security: String,
    flow: Option<String>,
    params: BTreeMap<String, String>,
}

fn finish_node(draft: NodeDraft<'_>) -> VpnNode {
    let fragment = draft.fragment;
    let lower = fragment.to_lowercase();
    let whitelist = lower.contains("белые списки") || lower.contains("whitelist");
    let without_kind = fragment
        .replace("[Белые списки]", "")
        .replace("[белые списки]", "")
        .replace("[Whitelist]", "")
        .replace("[whitelist]", "");
    // Variation selectors render as wide blanks in the UI font.
    let without_kind = without_kind
        .replace('\u{FE0F}', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let (flag, country) = split_flag(&without_kind);
    let name = if without_kind.is_empty() {
        format!("Сервер {}", draft.line_number)
    } else {
        without_kind.clone()
    };
    VpnNode {
        raw: draft.raw.to_owned(),
        protocol: draft.protocol,
        name,
        country: if country.trim().is_empty() {
            "Неизвестно".to_owned()
        } else {
            country.trim().to_owned()
        },
        flag,
        host: draft.host,
        port: draft.port,
        user: draft.user,
        password: draft.password,
        transport: draft.transport,
        security: draft.security,
        flow: draft.flow,
        whitelist,
        params: draft.params,
        source: String::new(),
    }
}

fn parse_vmess(raw: &str, line_number: usize) -> Result<VpnNode, String> {
    let payload = &raw["vmess://".len()..];
    let payload = payload.split('#').next().unwrap_or_default();
    let text = decode_base64_text(&decode(payload))
        .ok_or_else(|| format!("line {line_number}: vmess payload is not base64"))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("line {line_number}: vmess json: {error}"))?;
    let field = |key: &str| -> String {
        match json.get(key) {
            Some(serde_json::Value::String(value)) => value.trim().to_owned(),
            Some(serde_json::Value::Number(value)) => value.to_string(),
            Some(serde_json::Value::Bool(value)) => value.to_string(),
            _ => String::new(),
        }
    };
    let host = field("add")
        .trim_matches(|c| c == '[' || c == ']')
        .to_owned();
    if host.is_empty() {
        return Err(format!("line {line_number}: vmess without address"));
    }
    let port: u16 = field("port")
        .parse()
        .map_err(|_| format!("line {line_number}: vmess without port"))?;
    let user = field("id");
    if user.is_empty() {
        return Err(format!("line {line_number}: vmess without id"));
    }
    let network = match field("net").to_ascii_lowercase().as_str() {
        "" => "tcp".to_owned(),
        other => other.to_owned(),
    };
    let mut params = BTreeMap::new();
    let mut put = |key: &str, value: String| {
        if !value.is_empty() {
            params.insert(key.to_owned(), value);
        }
    };
    put("type", network.clone());
    put("host", field("host"));
    if network == "grpc" {
        put("serviceName", field("path"));
    } else {
        put("path", field("path"));
    }
    put("headerType", field("type"));
    put("sni", field("sni"));
    put("alpn", field("alpn"));
    put("fp", field("fp"));
    put("encryption", field("scy"));
    put("alterId", field("aid"));
    if matches!(field("allowInsecure").as_str(), "1" | "true") {
        put("allowInsecure", "1".to_owned());
    }
    let security = match field("tls").to_ascii_lowercase().as_str() {
        "tls" => "tls".to_owned(),
        "reality" => "reality".to_owned(),
        _ => "none".to_owned(),
    };
    if security == "reality" {
        put("pbk", field("pbk"));
        put("sid", field("sid"));
    }
    let name = field("ps");
    Ok(finish_node(NodeDraft {
        raw,
        protocol: Protocol::Vmess,
        fragment: &name,
        line_number,
        host,
        port,
        user,
        password: None,
        transport: network,
        security,
        flow: None,
        params,
    }))
}

fn parse_shadowsocks(raw: &str, line_number: usize) -> Result<VpnNode, String> {
    let body = &raw["ss://".len()..];
    let (body, fragment) = match body.split_once('#') {
        Some((body, fragment)) => (body, decode(fragment)),
        None => (body, String::new()),
    };
    let (body, query) = match body.split_once('?') {
        Some((body, query)) => (body.trim_end_matches('/'), query),
        None => (body.trim_end_matches('/'), ""),
    };

    // SIP002: userinfo@host:port, legacy: base64(method:password@host:port)
    let (userinfo, server) = match body.rsplit_once('@') {
        Some((userinfo, server)) => {
            let userinfo = decode(userinfo);
            let userinfo = if userinfo.contains(':') {
                userinfo
            } else {
                decode_base64_text(&userinfo)
                    .ok_or_else(|| format!("line {line_number}: invalid ss userinfo"))?
            };
            (userinfo, server.to_owned())
        }
        None => {
            let text = decode_base64_text(&decode(body))
                .ok_or_else(|| format!("line {line_number}: invalid legacy ss link"))?;
            let (userinfo, server) = text
                .rsplit_once('@')
                .ok_or_else(|| format!("line {line_number}: invalid legacy ss link"))?;
            (userinfo.to_owned(), server.to_owned())
        }
    };
    let (method, password) = userinfo
        .split_once(':')
        .ok_or_else(|| format!("line {line_number}: ss without password"))?;
    let (host, port) = split_host_port(&server)
        .ok_or_else(|| format!("line {line_number}: ss without host or port"))?;
    if method.is_empty() || password.is_empty() {
        return Err(format!("line {line_number}: ss without method or password"));
    }

    let mut params = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        params.insert(key.into_owned(), value.into_owned());
    }
    Ok(finish_node(NodeDraft {
        raw,
        protocol: Protocol::Shadowsocks,
        fragment: &fragment,
        line_number,
        host,
        port,
        user: method.to_owned(),
        password: Some(password.to_owned()),
        transport: "tcp".to_owned(),
        security: "none".to_owned(),
        flow: None,
        params,
    }))
}

fn split_host_port(value: &str) -> Option<(String, u16)> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        return Some((host.to_owned(), port.parse().ok()?));
    }
    let (host, port) = value.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_owned(), port.trim_end_matches('/').parse().ok()?))
}

fn decode(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

/// Splits a leading flag emoji (regional indicators) from the rest of a name.
pub fn split_flag(value: &str) -> (String, String) {
    let value = value.trim_start();
    let mut flag = String::new();
    let mut consumed = 0;
    for (offset, character) in value.char_indices() {
        let regional = ('\u{1F1E6}'..='\u{1F1FF}').contains(&character);
        let variation = character == '\u{FE0F}';
        if regional || variation {
            flag.push(character);
            consumed = offset + character.len_utf8();
        } else {
            break;
        }
    }
    (flag, value[consumed..].trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vless_reality() {
        let input = "vless://1234@example.org:443?security=reality&type=grpc&serviceName=TunService&pbk=abc&sni=example.com#%F0%9F%87%A9%F0%9F%87%AA%D0%93%D0%B5%D1%80%D0%BC%D0%B0%D0%BD%D0%B8%D1%8F";
        let report = parse_subscription(input);
        assert_eq!(report.nodes.len(), 1);
        let node = &report.nodes[0];
        assert_eq!(node.protocol, Protocol::Vless);
        assert_eq!(node.country, "Германия");
        assert_eq!(node.transport, "grpc");
        assert_eq!(node.security, "reality");
    }

    #[test]
    fn parses_hysteria_and_marks_whitelist() {
        let input =
            "hysteria2://secret@example.org:8443?sni=example.org#🇷🇺Россия%20[Белые%20списки]";
        let report = parse_subscription(input);
        assert_eq!(report.nodes.len(), 1);
        assert!(report.nodes[0].whitelist);
        assert_eq!(report.nodes[0].protocol, Protocol::Hysteria2);
    }

    #[test]
    fn removes_exact_duplicates() {
        let line = "trojan://secret@example.org:443#Server";
        let report = parse_subscription(&format!("{line}\n{line}"));
        assert_eq!(report.nodes.len(), 1);
        assert_eq!(report.duplicates, 1);
    }

    #[test]
    fn keeps_provider_order() {
        let report = parse_subscription(
            "trojan://a@z.example:443#Zeta\nvless://b@a.example:443?security=tls#Alpha",
        );
        assert_eq!(report.nodes[0].name, "Zeta");
        assert_eq!(report.nodes[1].name, "Alpha");
    }

    #[test]
    fn decodes_base64_subscription_body() {
        let plain =
            "vless://1234@example.org:443?security=tls#One\ntrojan://pw@example.net:443#Two";
        let encoded = base64::engine::general_purpose::STANDARD.encode(plain);
        let report = parse_subscription(&encoded);
        assert_eq!(report.nodes.len(), 2);
        assert_eq!(report.nodes[1].protocol, Protocol::Trojan);
    }

    #[test]
    fn parses_shadowsocks_sip002_and_legacy() {
        let userinfo =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("aes-256-gcm:secret");
        let sip002 = format!("ss://{userinfo}@1.2.3.4:8388#%F0%9F%87%B3%F0%9F%87%B1Amsterdam");
        let legacy = format!(
            "ss://{}#Legacy",
            base64::engine::general_purpose::STANDARD
                .encode("chacha20-ietf-poly1305:pw@example.com:443")
        );
        let plain = "ss://2022-blake3-aes-128-gcm:a2V5@example.net:443#Plain";
        let report = parse_subscription(&format!("{sip002}\n{legacy}\n{plain}"));
        assert_eq!(report.ignored, 0);
        assert_eq!(report.nodes.len(), 3);
        assert_eq!(report.nodes[0].user, "aes-256-gcm");
        assert_eq!(report.nodes[0].password.as_deref(), Some("secret"));
        assert_eq!(report.nodes[0].country, "Amsterdam");
        assert_eq!(report.nodes[1].host, "example.com");
        assert_eq!(report.nodes[1].port, 443);
        assert_eq!(report.nodes[2].user, "2022-blake3-aes-128-gcm");
    }

    #[test]
    fn parses_vmess_json_link() {
        let json = r#"{"v":"2","ps":"Tokyo","add":"example.org","port":"443","id":"b831381d-6324-4d53-ad4f-8cda48b30811","aid":"0","scy":"auto","net":"ws","type":"none","host":"cdn.example.org","path":"/ws","tls":"tls","sni":"cdn.example.org"}"#;
        let link = format!(
            "vmess://{}",
            base64::engine::general_purpose::STANDARD.encode(json)
        );
        let report = parse_subscription(&link);
        assert_eq!(report.nodes.len(), 1);
        let node = &report.nodes[0];
        assert_eq!(node.protocol, Protocol::Vmess);
        assert_eq!(node.transport, "ws");
        assert_eq!(node.security, "tls");
        assert_eq!(node.parameter("path"), Some("/ws"));
        assert_eq!(node.name, "Tokyo");
    }

    #[test]
    fn detects_placeholder_nodes() {
        let report = parse_subscription(
            "vless://00000000-0000-0000-0000-000000000000@0.0.0.0:1?security=none#Осталось%2012%20дней",
        );
        assert!(report.nodes[0].is_placeholder());
    }

    #[test]
    fn strips_variation_selectors_from_names() {
        let report = parse_subscription(
            "trojan://pw@example.org:443#%F0%9F%87%B3%F0%9F%87%B1%20%F0%9F%93%B1%20%E2%AD%90%EF%B8%8F%20%20Mobile%20%231",
        );
        let node = &report.nodes[0];
        assert_eq!(node.flag, "🇳🇱");
        assert_eq!(node.title(), "📱 ⭐ Mobile #1");
        assert!(!node.name.contains('\u{FE0F}'));
    }

    #[test]
    fn zero_uuid_message_is_a_placeholder() {
        // What a Remnawave panel sends when the device ID (HWID) is missing.
        let report = parse_subscription(
            "vless://00000000-0000-0000-0000-000000000000@error.cdn-global.pro:443?security=none&type=tcp&headerType=none#%E2%9D%8C%20%D0%A2%D1%80%D0%B5%D0%B1%D1%83%D0%B5%D1%82%D1%81%D1%8F%20HWID",
        );
        assert!(report.nodes[0].is_placeholder());
    }
}
