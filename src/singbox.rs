use crate::parser::{Protocol, VpnNode};
use anyhow::{anyhow, bail, Context, Result};
use directories::ProjectDirs;
use serde_json::{json, Map, Value};
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use sha2::{Digest, Sha256};
use std::fs;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use std::io::Cursor;
use std::io::{BufRead, BufReader};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use zip::ZipArchive;

pub const CORE_VERSION: &str = "1.14.0";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const CORE_AMD64_URL: &str = "https://github.com/SagerNet/sing-box/releases/download/v1.14.0/sing-box-1.14.0-windows-amd64.zip";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const CORE_AMD64_SHA256: &str = "3ffb56267da14e287be48bd10cf7e6505260125bad940b75101fbb4d5d58e5d6";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitTunnelMode {
    #[default]
    BypassSelected,
    OnlySelected,
}

#[derive(Debug, Clone)]
pub struct TunnelOptions {
    pub strict_route: bool,
    pub allow_lan: bool,
    pub secure_dns: bool,
    pub split_tunnel: bool,
    pub split_mode: SplitTunnelMode,
    pub split_process_names: Vec<String>,
    pub split_process_paths: Vec<String>,
    pub split_domains: Vec<String>,
    pub multihop_entry: Option<VpnNode>,
    pub route_flow: Vec<RouteFlowRule>,
    /// Local Clash API endpoint used to read live traffic statistics.
    pub stats_api: Option<StatsApi>,
}

#[derive(Debug, Clone)]
pub struct StatsApi {
    pub address: String,
    pub secret: String,
}

#[derive(Debug, Clone)]
pub struct RouteFlowRule {
    pub node: VpnNode,
    pub process_names: Vec<String>,
    pub process_paths: Vec<String>,
    pub domains: Vec<String>,
}

impl Default for TunnelOptions {
    fn default() -> Self {
        Self {
            strict_route: true,
            allow_lan: true,
            secure_dns: false,
            split_tunnel: false,
            split_mode: SplitTunnelMode::BypassSelected,
            split_process_names: Vec::new(),
            split_process_paths: Vec::new(),
            split_domains: Vec::new(),
            multihop_entry: None,
            route_flow: Vec::new(),
            stats_api: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub settings: PathBuf,
    pub runtime_config: PathBuf,
    pub core: PathBuf,
    pub subscriptions: PathBuf,
    pub locations: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let project = ProjectDirs::from("", "", "universe")
            .ok_or_else(|| anyhow!("Windows не вернул папку данных приложения"))?;
        Self::at(project.data_local_dir().to_path_buf())
    }

    pub fn at(root: PathBuf) -> Result<Self> {
        let subscriptions = root.join("subscriptions");
        fs::create_dir_all(&subscriptions)
            .with_context(|| format!("не удалось создать {}", subscriptions.display()))?;
        Ok(Self {
            settings: root.join("settings.json"),
            runtime_config: root.join("sing-box.json"),
            core: root.join("core").join(core_file_name()),
            locations: root.join("server-locations.json"),
            subscriptions,
            root,
        })
    }

    pub fn subscription_cache(&self, id: &str) -> PathBuf {
        let safe: String = id
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
            .collect();
        self.subscriptions.join(format!("{safe}.txt"))
    }
}

pub fn find_core(paths: &AppPaths) -> Option<PathBuf> {
    let beside_app = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(core_file_name())));
    beside_app
        .filter(|path| path.is_file())
        .or_else(|| paths.core.is_file().then(|| paths.core.clone()))
}

pub fn install_core(paths: &AppPaths) -> Result<PathBuf> {
    install_core_with_progress(paths, |_, _| {})
}

/// Downloads and verifies sing-box, calling `progress(downloaded, total)` as bytes arrive.
pub fn install_core_with_progress(
    paths: &AppPaths,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        let _ = (paths, &mut progress);
        bail!("Автоустановка sing-box сейчас поддерживает Windows x64");
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("universe/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .context("не удалось создать HTTP-клиент")?;
        let mut response = client
            .get(CORE_AMD64_URL)
            .send()
            .context("не удалось скачать sing-box с GitHub")?
            .error_for_status()
            .context("GitHub вернул ошибку при скачивании sing-box")?;
        let total = response.content_length();
        let mut bytes = Vec::with_capacity(total.unwrap_or(16 << 20) as usize);
        let mut buffer = [0_u8; 64 * 1024];
        progress(0, total);
        loop {
            let read = std::io::Read::read(&mut response, &mut buffer)
                .context("не удалось прочитать архив sing-box")?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            progress(bytes.len() as u64, total);
        }
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if digest != CORE_AMD64_SHA256 {
            bail!("контрольная сумма sing-box не совпала; файл не установлен");
        }

        if let Some(parent) = paths.core.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("не удалось создать {}", parent.display()))?;
        }
        let mut archive = ZipArchive::new(Cursor::new(bytes)).context("поврежден ZIP sing-box")?;
        let mut extracted = false;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .context("не удалось прочитать ZIP")?;
            let Some(path) = entry.enclosed_name() else {
                continue;
            };
            if path.file_name().and_then(|name| name.to_str()) == Some("sing-box.exe") {
                let mut output = fs::File::create(&paths.core)
                    .with_context(|| format!("не удалось создать {}", paths.core.display()))?;
                std::io::copy(&mut entry, &mut output)
                    .context("не удалось распаковать sing-box.exe")?;
                extracted = true;
                break;
            }
        }
        if !extracted {
            bail!("в официальном архиве не найден sing-box.exe");
        }
        Ok(paths.core.clone())
    }
}

pub fn build_config(node: &VpnNode) -> Result<Value> {
    build_config_with_options(node, &TunnelOptions::default())
}

pub fn build_config_with_options(node: &VpnNode, options: &TunnelOptions) -> Result<Value> {
    let mut proxy = build_outbound(node, "proxy")?;
    let mut outbounds = Vec::new();
    if let Some(entry) = &options.multihop_entry {
        if entry.raw == node.raw {
            bail!("входной и выходной серверы MultiHop должны отличаться");
        }
        outbounds.push(build_outbound(entry, "multihop-entry")?);
        proxy
            .as_object_mut()
            .expect("VPN outbound must be an object")
            .insert("detour".into(), json!("multihop-entry"));
    }
    outbounds.push(proxy);

    for (index, rule) in options.route_flow.iter().enumerate() {
        let tag = format!("routeflow-{index}");
        outbounds.push(build_outbound(&rule.node, &tag)?);
    }
    outbounds.push(json!({
        "type": "direct",
        "tag": "direct"
    }));

    let mut dns_servers = vec![json!({
        "type": "local",
        "tag": "local-dns"
    })];
    let dns_final = if options.secure_dns {
        dns_servers.push(json!({
            "type": "https",
            "tag": "secure-dns",
            "server": "1.1.1.1",
            "path": "/dns-query",
            "tls": {
                "enabled": true,
                "server_name": "cloudflare-dns.com"
            },
            "detour": "proxy"
        }));
        "secure-dns"
    } else {
        "local-dns"
    };

    let mut route_rules = vec![
        json!({ "action": "sniff" }),
        json!({ "protocol": "dns", "action": "hijack-dns" }),
    ];
    if options.allow_lan {
        route_rules.push(json!({
            "ip_is_private": true,
            "action": "route",
            "outbound": "direct"
        }));
    }

    for (index, flow) in options.route_flow.iter().enumerate() {
        let outbound = format!("routeflow-{index}");
        if !flow.process_names.is_empty() {
            route_rules.push(json!({
                "process_name": flow.process_names,
                "action": "route",
                "outbound": outbound
            }));
        }
        if !flow.process_paths.is_empty() {
            route_rules.push(json!({
                "process_path": flow.process_paths,
                "action": "route",
                "outbound": outbound
            }));
        }
        if !flow.domains.is_empty() {
            route_rules.push(json!({
                "domain_suffix": flow.domains,
                "action": "route",
                "outbound": outbound
            }));
        }
    }

    let split_outbound = match options.split_mode {
        SplitTunnelMode::BypassSelected => "direct",
        SplitTunnelMode::OnlySelected => "proxy",
    };
    if options.split_tunnel {
        if !options.split_process_names.is_empty() {
            route_rules.push(json!({
                "process_name": options.split_process_names,
                "action": "route",
                "outbound": split_outbound
            }));
        }
        if !options.split_process_paths.is_empty() {
            route_rules.push(json!({
                "process_path": options.split_process_paths,
                "action": "route",
                "outbound": split_outbound
            }));
        }
        if !options.split_domains.is_empty() {
            route_rules.push(json!({
                "domain_suffix": options.split_domains,
                "action": "route",
                "outbound": split_outbound
            }));
        }
    }
    let route_final = if options.split_tunnel && options.split_mode == SplitTunnelMode::OnlySelected
    {
        "direct"
    } else {
        "proxy"
    };

    let mut config = json!({
        "$schema": "https://sing-box.sagernet.org/schema.json",
        "log": {
            "level": "info",
            "timestamp": true
        },
        "dns": {
            "servers": dns_servers,
            "final": dns_final,
            "strategy": "prefer_ipv4",
            "cache_capacity": 4096
        },
        "inbounds": [
            {
                "type": "tun",
                "tag": "tun-in",
                "interface_name": "universe",
                "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
                "mtu": 1500,
                "dns_mode": "hijack",
                "auto_route": true,
                "strict_route": options.strict_route
            }
        ],
        "outbounds": outbounds,
        "route": {
            "auto_detect_interface": true,
            "default_domain_resolver": {
                "server": "local-dns",
                "strategy": "prefer_ipv4"
            },
            "rules": route_rules,
            "final": route_final
        }
    });
    if let Some(api) = &options.stats_api {
        config["experimental"] = json!({
            "clash_api": {
                "external_controller": api.address,
                "secret": api.secret
            }
        });
    }
    Ok(config)
}

fn build_outbound(node: &VpnNode, tag: &str) -> Result<Value> {
    match node.protocol {
        Protocol::Vless => build_vless(node, tag),
        Protocol::Vmess => build_vmess(node, tag),
        Protocol::Hysteria2 => build_hysteria2(node, tag),
        Protocol::Trojan => build_trojan(node, tag),
        Protocol::Shadowsocks => build_shadowsocks(node, tag),
    }
}

/// Checks that sing-box can be configured for the node without writing anything.
pub fn node_is_supported(node: &VpnNode) -> bool {
    build_outbound(node, "probe").is_ok()
}

fn build_vmess(node: &VpnNode, tag: &str) -> Result<Value> {
    let mut outbound = Map::new();
    outbound.insert("type".into(), json!("vmess"));
    outbound.insert("tag".into(), json!(tag));
    outbound.insert("server".into(), json!(node.host));
    outbound.insert("server_port".into(), json!(node.port));
    outbound.insert("uuid".into(), json!(node.user));
    let cipher = node
        .parameter("encryption")
        .filter(|value| !value.is_empty())
        .unwrap_or("auto");
    outbound.insert("security".into(), json!(cipher));
    let alter_id: u32 = node
        .parameter("alterId")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    outbound.insert("alter_id".into(), json!(alter_id));
    add_domain_resolver(node, &mut outbound);
    if let Some(tls) = build_tls(node)? {
        outbound.insert("tls".into(), tls);
    }
    if let Some(transport) = build_transport(node)? {
        outbound.insert("transport".into(), transport);
    }
    Ok(Value::Object(outbound))
}

fn build_shadowsocks(node: &VpnNode, tag: &str) -> Result<Value> {
    const METHODS: &[&str] = &[
        "2022-blake3-aes-128-gcm",
        "2022-blake3-aes-256-gcm",
        "2022-blake3-chacha20-poly1305",
        "none",
        "aes-128-gcm",
        "aes-192-gcm",
        "aes-256-gcm",
        "chacha20-ietf-poly1305",
        "xchacha20-ietf-poly1305",
        "aes-128-ctr",
        "aes-192-ctr",
        "aes-256-ctr",
        "aes-128-cfb",
        "aes-192-cfb",
        "aes-256-cfb",
        "rc4-md5",
        "chacha20-ietf",
        "xchacha20",
    ];
    let method = node.user.to_ascii_lowercase();
    if !METHODS.contains(&method.as_str()) {
        bail!("шифр Shadowsocks {method} не поддерживается");
    }
    let mut outbound = Map::new();
    outbound.insert("type".into(), json!("shadowsocks"));
    outbound.insert("tag".into(), json!(tag));
    outbound.insert("server".into(), json!(node.host));
    outbound.insert("server_port".into(), json!(node.port));
    outbound.insert("method".into(), json!(method));
    outbound.insert(
        "password".into(),
        json!(node.password.clone().unwrap_or_default()),
    );
    if let Some(plugin) = node.parameter("plugin").filter(|value| !value.is_empty()) {
        let (name, options) = plugin.split_once(';').unwrap_or((plugin, ""));
        let name = match name {
            "simple-obfs" | "obfs-local" => "obfs-local",
            "v2ray-plugin" => "v2ray-plugin",
            other => bail!("плагин Shadowsocks {other} не поддерживается"),
        };
        outbound.insert("plugin".into(), json!(name));
        outbound.insert("plugin_opts".into(), json!(options));
    }
    add_domain_resolver(node, &mut outbound);
    Ok(Value::Object(outbound))
}

fn build_vless(node: &VpnNode, tag: &str) -> Result<Value> {
    let mut outbound = Map::new();
    outbound.insert("type".into(), json!("vless"));
    outbound.insert("tag".into(), json!(tag));
    outbound.insert("server".into(), json!(node.host));
    outbound.insert("server_port".into(), json!(node.port));
    outbound.insert("uuid".into(), json!(node.user));
    if let Some(flow) = &node.flow {
        outbound.insert("flow".into(), json!(flow));
    }
    if let Some(packet_encoding) = node.parameter("packetEncoding") {
        if matches!(packet_encoding, "xudp" | "packetaddr") {
            outbound.insert("packet_encoding".into(), json!(packet_encoding));
        }
    }
    add_domain_resolver(node, &mut outbound);
    if let Some(tls) = build_tls(node)? {
        outbound.insert("tls".into(), tls);
    }
    if let Some(transport) = build_transport(node)? {
        outbound.insert("transport".into(), transport);
    }
    Ok(Value::Object(outbound))
}

fn build_hysteria2(node: &VpnNode, tag: &str) -> Result<Value> {
    let password = match &node.password {
        Some(password) => format!("{}:{}", node.user, password),
        None => node.user.clone(),
    };
    let mut outbound = Map::new();
    outbound.insert("type".into(), json!("hysteria2"));
    outbound.insert("tag".into(), json!(tag));
    outbound.insert("server".into(), json!(node.host));
    outbound.insert("server_port".into(), json!(node.port));
    outbound.insert("password".into(), json!(password));
    add_domain_resolver(node, &mut outbound);

    if let Some(obfs_type) = node.parameter("obfs") {
        if !obfs_type.is_empty() {
            let password = node
                .parameter("obfs-password")
                .or_else(|| node.parameter("obfsParam"))
                .unwrap_or_default();
            outbound.insert(
                "obfs".into(),
                json!({ "type": obfs_type, "password": password }),
            );
        }
    }
    if let Some(up) = parse_speed(node.parameter("upmbps").or_else(|| node.parameter("up"))) {
        outbound.insert("up_mbps".into(), json!(up));
    }
    if let Some(down) = parse_speed(
        node.parameter("downmbps")
            .or_else(|| node.parameter("down")),
    ) {
        outbound.insert("down_mbps".into(), json!(down));
    }
    outbound.insert("tls".into(), build_standard_tls(node));
    Ok(Value::Object(outbound))
}

fn build_trojan(node: &VpnNode, tag: &str) -> Result<Value> {
    let password = match &node.password {
        Some(password) => format!("{}:{}", node.user, password),
        None => node.user.clone(),
    };
    let mut outbound = Map::new();
    outbound.insert("type".into(), json!("trojan"));
    outbound.insert("tag".into(), json!(tag));
    outbound.insert("server".into(), json!(node.host));
    outbound.insert("server_port".into(), json!(node.port));
    outbound.insert("password".into(), json!(password));
    add_domain_resolver(node, &mut outbound);
    outbound.insert("tls".into(), build_standard_tls(node));
    if let Some(transport) = build_transport(node)? {
        outbound.insert("transport".into(), transport);
    }
    Ok(Value::Object(outbound))
}

fn build_tls(node: &VpnNode) -> Result<Option<Value>> {
    match node.security.to_ascii_lowercase().as_str() {
        "" | "none" => Ok(None),
        "tls" => Ok(Some(build_standard_tls(node))),
        "reality" => {
            let public_key = node
                .parameter("pbk")
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("в Reality-конфиге отсутствует публичный ключ pbk"))?;
            let mut tls = standard_tls_map(node);
            // sing-box requires uTLS for every Reality client, even when an
            // imported URI omitted the optional fp parameter.
            tls.entry("utls")
                .or_insert_with(|| json!({ "enabled": true, "fingerprint": "chrome" }));
            let mut reality = Map::new();
            reality.insert("enabled".into(), json!(true));
            reality.insert("public_key".into(), json!(public_key));
            if let Some(short_id) = node.parameter("sid").filter(|value| !value.is_empty()) {
                reality.insert("short_id".into(), json!(short_id));
            }
            tls.insert("reality".into(), Value::Object(reality));
            Ok(Some(Value::Object(tls)))
        }
        security => bail!("неподдерживаемый режим защиты: {security}"),
    }
}

fn build_standard_tls(node: &VpnNode) -> Value {
    Value::Object(standard_tls_map(node))
}

fn standard_tls_map(node: &VpnNode) -> Map<String, Value> {
    let mut tls = Map::new();
    tls.insert("enabled".into(), json!(true));
    tls.insert(
        "server_name".into(),
        json!(node
            .parameter("sni")
            .filter(|v| !v.is_empty())
            .unwrap_or(&node.host)),
    );
    if truthy(node.parameter("allowInsecure")) || truthy(node.parameter("insecure")) {
        tls.insert("insecure".into(), json!(true));
    }
    if let Some(alpn) = node.parameter("alpn").filter(|value| !value.is_empty()) {
        let values: Vec<&str> = alpn.split(',').filter(|value| !value.is_empty()).collect();
        if !values.is_empty() {
            tls.insert("alpn".into(), json!(values));
        }
    }
    if let Some(fingerprint) = node.parameter("fp").filter(|value| !value.is_empty()) {
        tls.insert(
            "utls".into(),
            json!({ "enabled": true, "fingerprint": fingerprint }),
        );
    }
    tls
}

fn build_transport(node: &VpnNode) -> Result<Option<Value>> {
    let kind = node.transport.to_ascii_lowercase();
    let path = node.parameter("path").filter(|value| !value.is_empty());
    let host = node
        .parameter("host")
        .or_else(|| node.parameter("authority"))
        .filter(|value| !value.is_empty());
    match kind.as_str() {
        "" | "tcp" => {
            let header = node.parameter("headerType").unwrap_or("none");
            if !matches!(header, "" | "none") {
                bail!("TCP headerType={header} пока не поддерживается");
            }
            Ok(None)
        }
        "grpc" => Ok(Some(json!({
            "type": "grpc",
            "service_name": node.parameter("serviceName").unwrap_or_default()
        }))),
        "ws" | "websocket" => {
            let mut headers = Map::new();
            if let Some(host) = host {
                headers.insert("Host".into(), json!(host));
            }
            Ok(Some(json!({
                "type": "ws",
                "path": path.unwrap_or("/"),
                "headers": headers
            })))
        }
        "http" | "h2" => Ok(Some(json!({
            "type": "http",
            "host": host.map(|value| vec![value]).unwrap_or_default(),
            "path": path.unwrap_or("/")
        }))),
        "httpupgrade" => Ok(Some(json!({
            "type": "httpupgrade",
            "host": host.unwrap_or_default(),
            "path": path.unwrap_or("/")
        }))),
        "quic" => Ok(Some(json!({ "type": "quic" }))),
        unsupported => bail!("транспорт {unsupported} не поддерживается sing-box-клиентом"),
    }
}

fn add_domain_resolver(node: &VpnNode, outbound: &mut Map<String, Value>) {
    if node.host.parse::<IpAddr>().is_err() {
        outbound.insert(
            "domain_resolver".into(),
            json!({ "server": "local-dns", "strategy": "prefer_ipv4" }),
        );
    }
}

fn truthy(value: Option<&str>) -> bool {
    value
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn parse_speed(value: Option<&str>) -> Option<u32> {
    value.and_then(|value| {
        let digits: String = value
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        digits.parse().ok()
    })
}

pub fn write_runtime_config(paths: &AppPaths, node: &VpnNode) -> Result<PathBuf> {
    write_runtime_config_with_options(paths, node, &TunnelOptions::default())
}

pub fn write_runtime_config_with_options(
    paths: &AppPaths,
    node: &VpnNode,
    options: &TunnelOptions,
) -> Result<PathBuf> {
    let config = build_config_with_options(node, options)?;
    let bytes = serde_json::to_vec_pretty(&config).context("не удалось сериализовать конфиг")?;
    fs::write(&paths.runtime_config, bytes)
        .with_context(|| format!("не удалось записать {}", paths.runtime_config.display()))?;
    Ok(paths.runtime_config.clone())
}

pub fn check_config(core: &Path, config: &Path) -> Result<()> {
    let mut command = Command::new(core);
    command.args(["check", "-c"]).arg(config);
    hide_console(&mut command);
    let output = command
        .output()
        .context("не удалось запустить проверку sing-box")?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    bail!(
        "sing-box отклонил конфиг: {}",
        if stderr.is_empty() { stdout } else { stderr }
    )
}

pub struct CoreProcess {
    child: Child,
    log_rx: Receiver<String>,
}

impl CoreProcess {
    pub fn start(core: &Path, config: &Path, working_dir: &Path) -> Result<Self> {
        let mut command = Command::new(core);
        command
            .args(["run", "-c"])
            .arg(config)
            .current_dir(working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut command);
        let mut child = command.spawn().context("не удалось запустить sing-box")?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (log_tx, log_rx) = mpsc::channel();
        if let Some(stdout) = stdout {
            let sender = log_tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let _ = sender.send(line);
                }
            });
        }
        if let Some(stderr) = stderr {
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = log_tx.send(line);
                }
            });
        }
        Ok(Self { child, log_rx })
    }

    pub fn drain_logs(&self) -> Vec<String> {
        self.log_rx.try_iter().collect()
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("не удалось проверить процесс sing-box")
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for CoreProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn core_file_name() -> &'static str {
    if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    }
}

fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_subscription;

    #[test]
    fn builds_reality_grpc_config() {
        let report = parse_subscription(
            "vless://1234@example.org:443?security=reality&type=grpc&serviceName=TunService&pbk=abc&sid=12&sni=www.microsoft.com&fp=chrome#Germany",
        );
        let config = build_config(&report.nodes[0]).unwrap();
        assert_eq!(config["outbounds"][0]["type"], "vless");
        assert_eq!(config["outbounds"][0]["transport"]["type"], "grpc");
        assert_eq!(
            config["outbounds"][0]["tls"]["reality"]["public_key"],
            "abc"
        );
    }

    #[test]
    fn builds_shadowsocks_and_vmess_configs() {
        use base64::Engine as _;
        let vmess = format!(
            "vmess://{}",
            base64::engine::general_purpose::STANDARD.encode(
                r#"{"add":"example.org","port":443,"id":"b831381d-6324-4d53-ad4f-8cda48b30811","net":"grpc","path":"svc","tls":"tls"}"#
            )
        );
        let report = parse_subscription(&format!(
            "ss://aes-256-gcm:secret@example.net:8388#SS\n{vmess}"
        ));
        assert_eq!(report.nodes.len(), 2);
        let ss = build_config(&report.nodes[0]).unwrap();
        assert_eq!(ss["outbounds"][0]["type"], "shadowsocks");
        assert_eq!(ss["outbounds"][0]["method"], "aes-256-gcm");
        let vmess = build_config(&report.nodes[1]).unwrap();
        assert_eq!(vmess["outbounds"][0]["type"], "vmess");
        assert_eq!(vmess["outbounds"][0]["transport"]["service_name"], "svc");
        assert_eq!(vmess["outbounds"][0]["tls"]["enabled"], true);
    }

    #[test]
    fn adds_stats_api_when_requested() {
        let report = parse_subscription("vless://1234@example.org:443?security=tls#Germany");
        let options = TunnelOptions {
            stats_api: Some(StatsApi {
                address: "127.0.0.1:9999".to_owned(),
                secret: "s".to_owned(),
            }),
            ..TunnelOptions::default()
        };
        let config = build_config_with_options(&report.nodes[0], &options).unwrap();
        assert_eq!(
            config["experimental"]["clash_api"]["external_controller"],
            "127.0.0.1:9999"
        );
        assert!(build_config(&report.nodes[0])
            .unwrap()
            .get("experimental")
            .is_none());
    }

    #[test]
    fn builds_hysteria_config() {
        let report = parse_subscription(
            "hysteria2://secret@example.org:8443?sni=example.org&alpn=h3#Germany",
        );
        let config = build_config(&report.nodes[0]).unwrap();
        assert_eq!(config["outbounds"][0]["type"], "hysteria2");
        assert_eq!(config["outbounds"][0]["password"], "secret");
    }

    #[test]
    fn builds_bypass_split_tunnel_rules() {
        let report = parse_subscription("vless://1234@example.org:443?security=tls#Germany");
        let options = TunnelOptions {
            split_tunnel: true,
            split_process_names: vec!["chrome.exe".to_owned()],
            split_domains: vec!["example.com".to_owned()],
            ..TunnelOptions::default()
        };
        let config = build_config_with_options(&report.nodes[0], &options).unwrap();
        let rules = config["route"]["rules"].as_array().unwrap();
        assert!(rules.iter().any(|rule| {
            rule["process_name"][0] == "chrome.exe" && rule["outbound"] == "direct"
        }));
        assert!(rules.iter().any(|rule| {
            rule["domain_suffix"][0] == "example.com" && rule["outbound"] == "direct"
        }));
        assert_eq!(config["route"]["final"], "proxy");
    }

    #[test]
    fn builds_only_selected_mode_with_secure_dns() {
        let report = parse_subscription("vless://1234@example.org:443?security=tls#Germany");
        let options = TunnelOptions {
            secure_dns: true,
            split_tunnel: true,
            split_mode: SplitTunnelMode::OnlySelected,
            split_process_names: vec!["game.exe".to_owned()],
            ..TunnelOptions::default()
        };
        let config = build_config_with_options(&report.nodes[0], &options).unwrap();
        assert_eq!(config["route"]["final"], "direct");
        assert_eq!(config["dns"]["final"], "secure-dns");
        assert_eq!(config["dns"]["servers"][1]["type"], "https");
        assert!(config["route"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["process_name"][0] == "game.exe" && rule["outbound"] == "proxy"));
    }

    #[test]
    fn builds_real_multihop_chain() {
        let report = parse_subscription(
            "vless://entry@example.org:443?security=tls#Entry\n\
             trojan://exit@example.net:443?security=tls#Exit",
        );
        let options = TunnelOptions {
            multihop_entry: Some(report.nodes[0].clone()),
            ..TunnelOptions::default()
        };
        let config = build_config_with_options(&report.nodes[1], &options).unwrap();
        let outbounds = config["outbounds"].as_array().unwrap();
        assert_eq!(outbounds[0]["tag"], "multihop-entry");
        assert_eq!(outbounds[1]["tag"], "proxy");
        assert_eq!(outbounds[1]["detour"], "multihop-entry");
        assert_eq!(config["route"]["final"], "proxy");
    }

    #[test]
    fn builds_routeflow_outbound_and_independent_rules() {
        let report = parse_subscription(
            "vless://default@example.org:443?security=tls#Default\n\
             vless://video@example.net:443?security=tls#Video",
        );
        let options = TunnelOptions {
            route_flow: vec![RouteFlowRule {
                node: report.nodes[1].clone(),
                process_names: vec!["player.exe".to_owned()],
                process_paths: Vec::new(),
                domains: vec!["youtube.com".to_owned()],
            }],
            ..TunnelOptions::default()
        };
        let config = build_config_with_options(&report.nodes[0], &options).unwrap();
        let outbounds = config["outbounds"].as_array().unwrap();
        assert!(outbounds
            .iter()
            .any(|outbound| outbound["tag"] == "routeflow-0"));
        let rules = config["route"]["rules"].as_array().unwrap();
        assert!(rules.iter().any(|rule| {
            rule["process_name"][0] == "player.exe" && rule["outbound"] == "routeflow-0"
        }));
        assert!(rules.iter().any(|rule| {
            rule["domain_suffix"][0] == "youtube.com" && rule["outbound"] == "routeflow-0"
        }));
    }
}
