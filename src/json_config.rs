//! Converts JSON subscriptions (Xray client configs and sing-box configs) into
//! share links, so the rest of the app handles them like any other subscription.

use base64::Engine as _;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Value};

const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Returns share links for every server in a JSON subscription, or `None`
/// when the text is not JSON at all.
pub fn json_to_links(text: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let mut links = Vec::new();
    match &value {
        Value::Array(items) => {
            for item in items {
                collect_config(item, &mut links);
            }
        }
        Value::Object(_) => collect_config(&value, &mut links),
        _ => return None,
    }
    Some(links)
}

fn collect_config(config: &Value, links: &mut Vec<String>) {
    let Some(outbounds) = config.get("outbounds").and_then(Value::as_array) else {
        return;
    };
    if outbounds
        .iter()
        .any(|outbound| outbound.get("protocol").is_some())
    {
        // Xray: one profile per config. Auto-groups rely on Xray balancers and
        // only repeat servers that are also listed on their own.
        let balanced = config
            .pointer("/routing/balancers")
            .and_then(Value::as_array)
            .is_some_and(|balancers| !balancers.is_empty());
        if balanced {
            return;
        }
        let name = text(config, "/remarks").unwrap_or_default();
        let outbound = outbounds
            .iter()
            .find(|outbound| text(outbound, "/tag") == Some("proxy"))
            .or_else(|| {
                outbounds.iter().find(|outbound| {
                    !matches!(
                        text(outbound, "/protocol"),
                        Some("freedom" | "blackhole" | "dns") | None
                    )
                })
            });
        if let Some(link) = outbound.and_then(|outbound| xray_link(outbound, name)) {
            links.push(link);
        }
        return;
    }
    // sing-box: every proxy outbound is a server.
    links.extend(outbounds.iter().filter_map(singbox_link));
}

#[derive(Default)]
struct Query(Vec<(String, String)>);

impl Query {
    fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        if !value.is_empty() {
            self.0.push((key.to_owned(), value));
        }
    }

    fn get(&self, key: &str) -> &str {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
            .unwrap_or_default()
    }

    fn encode(&self) -> String {
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(self.0.iter())
            .finish()
    }
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn number(value: &Value, pointer: &str) -> Option<u64> {
    let value = value.pointer(pointer)?;
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn flag(value: &Value, pointer: &str) -> bool {
    value.pointer(pointer).and_then(Value::as_bool) == Some(true)
}

fn list(value: &Value, pointer: &str) -> String {
    match value.pointer(pointer) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::String(text)) => text.trim().to_owned(),
        _ => String::new(),
    }
}

fn encode(value: &str) -> String {
    utf8_percent_encode(value, COMPONENT).to_string()
}

fn endpoint(host: &str, port: u64) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn link(scheme: &str, user: &str, host: &str, port: u64, query: &Query, name: &str) -> String {
    format!(
        "{scheme}://{}@{}?{}#{}",
        encode(user),
        endpoint(host, port),
        query.encode(),
        encode(name)
    )
}

fn shadowsocks_link(method: &str, password: &str, host: &str, port: u64, name: &str) -> String {
    let userinfo =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{method}:{password}"));
    format!("ss://{userinfo}@{}#{}", endpoint(host, port), encode(name))
}

fn vmess_link(
    id: &str,
    host: &str,
    port: u64,
    alter_id: u64,
    cipher: &str,
    query: &Query,
    name: &str,
) -> String {
    let network = match query.get("type") {
        "" => "tcp",
        other => other,
    };
    let path = if network == "grpc" {
        query.get("serviceName")
    } else {
        query.get("path")
    };
    let security = query.get("security");
    let payload = json!({
        "v": "2",
        "ps": name,
        "add": host,
        "port": port.to_string(),
        "id": id,
        "aid": alter_id.to_string(),
        "scy": if cipher.is_empty() { "auto" } else { cipher },
        "net": network,
        "type": query.get("headerType"),
        "host": query.get("host"),
        "path": path,
        "tls": if matches!(security, "tls" | "reality") { security } else { "" },
        "sni": query.get("sni"),
        "alpn": query.get("alpn"),
        "fp": query.get("fp"),
        "pbk": query.get("pbk"),
        "sid": query.get("sid"),
        "allowInsecure": query.get("allowInsecure"),
    });
    format!(
        "vmess://{}",
        base64::engine::general_purpose::STANDARD.encode(payload.to_string())
    )
}

// ── Xray ────────────────────────────────────────────────────────────────

fn xray_link(outbound: &Value, name: &str) -> Option<String> {
    let protocol = text(outbound, "/protocol")?;
    let stream = outbound.get("streamSettings").unwrap_or(&Value::Null);
    let mut query = Query::default();
    match protocol {
        "vless" | "vmess" => {
            let server = outbound.pointer("/settings/vnext/0")?;
            let host = text(server, "/address")?;
            let port = number(server, "/port")?;
            let user = server.pointer("/users/0")?;
            let id = text(user, "/id")?;
            xray_stream(stream, &mut query);
            if protocol == "vmess" {
                return Some(vmess_link(
                    id,
                    host,
                    port,
                    number(user, "/alterId").unwrap_or(0),
                    text(user, "/security").unwrap_or("auto"),
                    &query,
                    name,
                ));
            }
            query.set("encryption", text(user, "/encryption").unwrap_or("none"));
            query.set("flow", text(user, "/flow").unwrap_or_default());
            Some(link("vless", id, host, port, &query, name))
        }
        "trojan" => {
            let server = outbound.pointer("/settings/servers/0")?;
            xray_stream(stream, &mut query);
            Some(link(
                "trojan",
                text(server, "/password")?,
                text(server, "/address")?,
                number(server, "/port")?,
                &query,
                name,
            ))
        }
        "shadowsocks" => {
            let server = outbound.pointer("/settings/servers/0")?;
            Some(shadowsocks_link(
                text(server, "/method")?,
                text(server, "/password")?,
                text(server, "/address")?,
                number(server, "/port")?,
                name,
            ))
        }
        "hysteria" | "hysteria2" => {
            let settings = outbound.get("settings")?;
            let version = number(settings, "/version")
                .or_else(|| number(stream, "/hysteriaSettings/version"))
                .unwrap_or(2);
            if version != 2 {
                return None;
            }
            let auth =
                text(stream, "/hysteriaSettings/auth").or_else(|| text(settings, "/password"))?;
            xray_tls(stream, &mut query);
            Some(link(
                "hysteria2",
                auth,
                text(settings, "/address")?,
                number(settings, "/port")?,
                &query,
                name,
            ))
        }
        _ => None,
    }
}

fn xray_tls(stream: &Value, query: &mut Query) {
    let security = text(stream, "/security").unwrap_or("none");
    query.set("security", security);
    let settings = match security {
        "tls" => "/tlsSettings",
        "reality" => "/realitySettings",
        _ => return,
    };
    let field = |name: &str| format!("{settings}/{name}");
    query.set(
        "sni",
        text(stream, &field("serverName")).unwrap_or_default(),
    );
    query.set(
        "fp",
        text(stream, &field("fingerprint")).unwrap_or_default(),
    );
    query.set("alpn", list(stream, &field("alpn")));
    if flag(stream, &field("allowInsecure")) {
        query.set("allowInsecure", "1");
    }
    if security == "reality" {
        query.set("pbk", text(stream, &field("publicKey")).unwrap_or_default());
        query.set("sid", text(stream, &field("shortId")).unwrap_or_default());
        query.set("spx", text(stream, &field("spiderX")).unwrap_or_default());
    }
}

fn xray_stream(stream: &Value, query: &mut Query) {
    let network = match text(stream, "/network").unwrap_or("tcp") {
        "raw" => "tcp",
        "h2" => "http",
        other => other,
    };
    query.set("type", network);
    xray_tls(stream, query);
    match network {
        "tcp" => {
            query.set(
                "headerType",
                text(stream, "/tcpSettings/header/type").unwrap_or_default(),
            );
        }
        "ws" => {
            query.set("path", text(stream, "/wsSettings/path").unwrap_or_default());
            query.set(
                "host",
                text(stream, "/wsSettings/host")
                    .or_else(|| text(stream, "/wsSettings/headers/Host"))
                    .unwrap_or_default(),
            );
        }
        "grpc" => {
            query.set(
                "serviceName",
                text(stream, "/grpcSettings/serviceName").unwrap_or_default(),
            );
        }
        "http" => {
            query.set(
                "path",
                text(stream, "/httpSettings/path").unwrap_or_default(),
            );
            query.set("host", list(stream, "/httpSettings/host"));
        }
        "httpupgrade" => {
            query.set(
                "path",
                text(stream, "/httpupgradeSettings/path").unwrap_or_default(),
            );
            query.set(
                "host",
                text(stream, "/httpupgradeSettings/host").unwrap_or_default(),
            );
        }
        "xhttp" | "splithttp" => {
            query.set(
                "path",
                text(stream, "/xhttpSettings/path").unwrap_or_default(),
            );
            query.set(
                "host",
                text(stream, "/xhttpSettings/host").unwrap_or_default(),
            );
        }
        _ => {}
    }
}

// ── sing-box ────────────────────────────────────────────────────────────

fn singbox_link(outbound: &Value) -> Option<String> {
    let kind = text(outbound, "/type")?;
    if !matches!(
        kind,
        "vless" | "vmess" | "trojan" | "shadowsocks" | "hysteria2"
    ) {
        return None;
    }
    let name = text(outbound, "/tag").unwrap_or_default();
    let host = text(outbound, "/server")?;
    let port = number(outbound, "/server_port")?;
    let mut query = Query::default();
    singbox_transport(outbound, &mut query);
    singbox_tls(outbound, &mut query);
    match kind {
        "vless" => {
            query.set("encryption", "none");
            query.set("flow", text(outbound, "/flow").unwrap_or_default());
            Some(link(
                "vless",
                text(outbound, "/uuid")?,
                host,
                port,
                &query,
                name,
            ))
        }
        "vmess" => Some(vmess_link(
            text(outbound, "/uuid")?,
            host,
            port,
            number(outbound, "/alter_id").unwrap_or(0),
            text(outbound, "/security").unwrap_or("auto"),
            &query,
            name,
        )),
        "trojan" => Some(link(
            "trojan",
            text(outbound, "/password")?,
            host,
            port,
            &query,
            name,
        )),
        "shadowsocks" => Some(shadowsocks_link(
            text(outbound, "/method")?,
            text(outbound, "/password")?,
            host,
            port,
            name,
        )),
        "hysteria2" => {
            query.0.retain(|(key, _)| key != "type");
            if let Some(obfs) = text(outbound, "/obfs/type") {
                query.set("obfs", obfs);
                query.set(
                    "obfs-password",
                    text(outbound, "/obfs/password").unwrap_or_default(),
                );
            }
            Some(link(
                "hysteria2",
                text(outbound, "/password")?,
                host,
                port,
                &query,
                name,
            ))
        }
        _ => None,
    }
}

fn singbox_tls(outbound: &Value, query: &mut Query) {
    if !flag(outbound, "/tls/enabled") {
        query.set("security", "none");
        return;
    }
    let reality = flag(outbound, "/tls/reality/enabled");
    query.set("security", if reality { "reality" } else { "tls" });
    query.set(
        "sni",
        text(outbound, "/tls/server_name").unwrap_or_default(),
    );
    query.set(
        "fp",
        text(outbound, "/tls/utls/fingerprint").unwrap_or_default(),
    );
    query.set("alpn", list(outbound, "/tls/alpn"));
    if flag(outbound, "/tls/insecure") {
        query.set("allowInsecure", "1");
    }
    if reality {
        query.set(
            "pbk",
            text(outbound, "/tls/reality/public_key").unwrap_or_default(),
        );
        query.set(
            "sid",
            text(outbound, "/tls/reality/short_id").unwrap_or_default(),
        );
    }
}

fn singbox_transport(outbound: &Value, query: &mut Query) {
    let kind = text(outbound, "/transport/type").unwrap_or("tcp");
    query.set("type", kind);
    match kind {
        "ws" => {
            query.set(
                "path",
                text(outbound, "/transport/path").unwrap_or_default(),
            );
            query.set(
                "host",
                text(outbound, "/transport/headers/Host").unwrap_or_default(),
            );
        }
        "grpc" => query.set(
            "serviceName",
            text(outbound, "/transport/service_name").unwrap_or_default(),
        ),
        "http" => {
            query.set(
                "path",
                text(outbound, "/transport/path").unwrap_or_default(),
            );
            query.set("host", list(outbound, "/transport/host"));
        }
        "httpupgrade" => {
            query.set(
                "path",
                text(outbound, "/transport/path").unwrap_or_default(),
            );
            query.set(
                "host",
                text(outbound, "/transport/host").unwrap_or_default(),
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{parse_subscription, Protocol};
    use crate::singbox::build_config;

    const XRAY: &str = r#"[
      {"remarks": "🇪🇺 Быстрый | АВТО",
       "routing": {"balancers": [{"tag": "b", "selector": ["proxy"]}]},
       "outbounds": [{"tag": "proxy", "protocol": "vless", "settings": {"vnext": [{"address": "1.1.1.1", "port": 443, "users": [{"id": "a"}]}]}}]},
      {"remarks": "🇳🇱 Reality #1",
       "outbounds": [
         {"tag": "proxy", "protocol": "vless",
          "settings": {"vnext": [{"address": "nl.example.com", "port": 443, "users": [{"id": "b831381d-6324-4d53-ad4f-8cda48b30811", "encryption": "none", "flow": "xtls-rprx-vision"}]}]},
          "streamSettings": {"network": "tcp", "tcpSettings": {}, "security": "reality",
            "realitySettings": {"serverName": "www.microsoft.com", "publicKey": "NxJ78HvQV0FXkTBt2hgASN1Po2Iv3-_Kw-NLGbKoWms", "shortId": "e3", "fingerprint": "firefox"}}},
         {"tag": "direct", "protocol": "freedom"},
         {"tag": "block", "protocol": "blackhole"}]},
      {"remarks": "🇩🇪 WS TLS",
       "outbounds": [{"tag": "proxy", "protocol": "vless",
          "settings": {"vnext": [{"address": "de.example.com", "port": 443, "users": [{"id": "b831381d-6324-4d53-ad4f-8cda48b30811", "encryption": "none", "flow": ""}]}]},
          "streamSettings": {"network": "ws", "wsSettings": {"path": "/account/refresh", "host": "cdn.example.com", "headers": {}},
            "security": "tls", "tlsSettings": {"serverName": "cdn.example.com", "fingerprint": "edge", "alpn": ["http/1.1"]}},
          "mux": {"enabled": true}}]},
      {"remarks": "🇫🇮 Hysteria",
       "outbounds": [{"tag": "proxy", "protocol": "hysteria",
          "settings": {"address": "fi.example.com", "port": 443, "version": 2},
          "streamSettings": {"network": "hysteria", "hysteriaSettings": {"version": 2, "auth": "p@ss:word"},
            "security": "tls", "tlsSettings": {"serverName": "fi.example.com", "alpn": ["h3"]}}}]},
      {"remarks": "🇬🇧 gRPC VMess",
       "outbounds": [{"tag": "proxy", "protocol": "vmess",
          "settings": {"vnext": [{"address": "uk.example.com", "port": 4243, "users": [{"id": "b831381d-6324-4d53-ad4f-8cda48b30811", "security": "auto"}]}]},
          "streamSettings": {"network": "grpc", "grpcSettings": {"serviceName": "ns"}, "security": "tls", "tlsSettings": {"serverName": "uk.example.com"}}}]}
    ]"#;

    #[test]
    fn converts_xray_json_subscription() {
        let report = parse_subscription(XRAY);
        assert_eq!(report.ignored, 0);
        assert_eq!(report.nodes.len(), 4, "auto-group must be skipped");

        let reality = &report.nodes[0];
        assert_eq!(reality.protocol, Protocol::Vless);
        assert_eq!(reality.name, "🇳🇱 Reality #1");
        assert_eq!(reality.flag, "🇳🇱");
        assert_eq!(reality.host, "nl.example.com");
        assert_eq!(reality.security, "reality");
        assert_eq!(reality.flow.as_deref(), Some("xtls-rprx-vision"));
        assert_eq!(reality.parameter("sid"), Some("e3"));

        let ws = &report.nodes[1];
        assert_eq!(ws.transport, "ws");
        assert_eq!(ws.parameter("path"), Some("/account/refresh"));
        assert_eq!(ws.parameter("host"), Some("cdn.example.com"));

        let hysteria = &report.nodes[2];
        assert_eq!(hysteria.protocol, Protocol::Hysteria2);
        assert_eq!(hysteria.user, "p@ss:word");

        let vmess = &report.nodes[3];
        assert_eq!(vmess.protocol, Protocol::Vmess);
        assert_eq!(vmess.transport, "grpc");
        assert_eq!(vmess.parameter("serviceName"), Some("ns"));

        for node in &report.nodes {
            build_config(node).unwrap_or_else(|error| panic!("{}: {error}", node.name));
        }
        let config = build_config(&report.nodes[2]).unwrap();
        assert_eq!(config["outbounds"][0]["password"], "p@ss:word");
    }

    #[test]
    fn converts_singbox_json_subscription() {
        let config = r#"{"outbounds": [
          {"type": "selector", "tag": "select", "outbounds": ["de"]},
          {"type": "vless", "tag": "🇩🇪 Germany", "server": "de.example.com", "server_port": 443,
           "uuid": "b831381d-6324-4d53-ad4f-8cda48b30811", "flow": "xtls-rprx-vision",
           "tls": {"enabled": true, "server_name": "www.apple.com", "utls": {"enabled": true, "fingerprint": "chrome"},
                   "reality": {"enabled": true, "public_key": "NxJ78HvQV0FXkTBt2hgASN1Po2Iv3-_Kw-NLGbKoWms", "short_id": "ab"}}},
          {"type": "trojan", "tag": "Trojan WS", "server": "tr.example.com", "server_port": 443, "password": "secret",
           "tls": {"enabled": true, "server_name": "tr.example.com"},
           "transport": {"type": "ws", "path": "/ws", "headers": {"Host": "tr.example.com"}}},
          {"type": "shadowsocks", "tag": "SS", "server": "1.2.3.4", "server_port": 8388, "method": "aes-256-gcm", "password": "pw"},
          {"type": "hysteria2", "tag": "HY2", "server": "hy.example.com", "server_port": 443, "password": "pw",
           "obfs": {"type": "salamander", "password": "obfs"}, "tls": {"enabled": true, "server_name": "hy.example.com"}},
          {"type": "direct", "tag": "direct"}
        ]}"#;
        let report = parse_subscription(config);
        assert_eq!(report.nodes.len(), 4);
        assert_eq!(report.nodes[0].security, "reality");
        assert_eq!(report.nodes[0].name, "🇩🇪 Germany");
        assert_eq!(report.nodes[1].transport, "ws");
        assert_eq!(report.nodes[2].protocol, Protocol::Shadowsocks);
        assert_eq!(report.nodes[3].parameter("obfs"), Some("salamander"));
        for node in &report.nodes {
            build_config(node).unwrap_or_else(|error| panic!("{}: {error}", node.name));
        }
    }

    #[test]
    fn plain_text_is_not_json() {
        assert!(json_to_links("vless://a@b:1").is_none());
    }
}
