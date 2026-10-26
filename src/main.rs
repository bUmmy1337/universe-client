#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod map;
#[cfg(target_os = "windows")]
mod native_tray;
mod pages;
mod theme;
mod welcome;

use eframe::egui::{self, Vec2};
use map::{
    country_coordinates, haversine_km, parse_world_map, GeoPoint, MapCameraAnimation,
    RouteAnimation, RoutePhase, WorldMap,
};
#[cfg(target_os = "windows")]
use native_tray::AppTray;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::BufRead;
use std::net::{IpAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};
use universe::device::{random_hwid, DeviceInfo};
use universe::parser::{parse_subscription, VpnNode};
use universe::singbox::{
    check_config, find_core, install_core_with_progress, node_is_supported,
    write_runtime_config_with_options, AppPaths, CoreProcess, RouteFlowRule, SplitTunnelMode,
    StatsApi, TunnelOptions,
};
use universe::subscription::{
    fetch_subscription, new_id, now_unix, FetchedSubscription, Subscription, DEFAULT_UPDATE_HOURS,
    DEFAULT_USER_AGENT,
};

const APP_NAME: &str = "universe";
/// Free community feed offered as an optional preset (servers of unknown owners).
const PUBLIC_FEED_URL: &str = "https://7ec.ru/vpn/configs";
const BRAND_ICON: &[u8] = include_bytes!("../assets/brand-icon.png");
const RECONNECT_DELAYS: [u64; 5] = [2, 4, 8, 15, 30];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    protocol_filter: String,
    source_filter: String,
    server_sort: String,
    auto_refresh: bool,
    update_interval_hours: u32,
    user_agent: String,
    /// Send the device ID (HWID) with subscription requests.
    send_hwid: bool,
    /// Custom device ID after a reset; empty means "derived from this computer".
    hwid: String,
    auto_connect: bool,
    start_minimized: bool,
    kill_switch: bool,
    secure_dns: bool,
    allow_lan: bool,
    split_tunnel: bool,
    split_mode: String,
    split_apps: String,
    split_domains: String,
    multihop_enabled: bool,
    multihop_entry: String,
    multihop_exit: String,
    route_flow_enabled: bool,
    route_flow: Vec<RouteFlowSetting>,
    last_server: String,
    sidebar_collapsed: bool,
    /// The welcome screen was completed (or skipped).
    onboarding_done: bool,
    subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
struct RouteFlowSetting {
    enabled: bool,
    server: String,
    apps: String,
    domains: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            protocol_filter: "all".to_owned(),
            source_filter: "all".to_owned(),
            server_sort: "provider".to_owned(),
            auto_refresh: true,
            update_interval_hours: DEFAULT_UPDATE_HOURS,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            send_hwid: true,
            hwid: String::new(),
            auto_connect: false,
            start_minimized: false,
            kill_switch: true,
            secure_dns: false,
            allow_lan: true,
            split_tunnel: false,
            split_mode: "bypass".to_owned(),
            split_apps: String::new(),
            split_domains: String::new(),
            multihop_enabled: false,
            multihop_entry: String::new(),
            multihop_exit: String::new(),
            route_flow_enabled: false,
            route_flow: Vec::new(),
            last_server: String::new(),
            sidebar_collapsed: false,
            onboarding_done: false,
            subscriptions: Vec::new(),
        }
    }
}

enum AppEvent {
    SubscriptionFetched {
        id: String,
        result: Result<FetchedSubscription, String>,
    },
    CoreInstalled(Result<PathBuf, String>),
    CoreProgress {
        downloaded: u64,
        total: Option<u64>,
    },
    OriginLocation(Result<GeoPoint, String>),
    ServerLocations(HashMap<String, ServerLocation>),
    ConnectFinished {
        operation: u64,
        raw: String,
        name: String,
        result: Result<CoreProcess, String>,
    },
    DisconnectFinished {
        operation: u64,
    },
    Latency {
        raw: String,
        value: Option<u128>,
    },
    PublicIp {
        operation: u64,
        raw: String,
        result: Result<String, String>,
    },
    Traffic {
        operation: u64,
        up: u64,
        down: u64,
    },
    ZapretStatus(Option<String>),
    #[cfg(target_os = "windows")]
    Tray(TrayAction),
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy)]
enum TrayAction {
    Show,
    Settings,
    ToggleConnection { hide_after: bool },
    Exit,
}

#[derive(Debug, Deserialize)]
struct IpLocationResponse {
    latitude: f32,
    longitude: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerLocation {
    ip: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    country_code: String,
    region: String,
    city: String,
    latitude: f32,
    longitude: f32,
}

#[derive(Debug, Deserialize)]
struct IpWhoResponse {
    success: bool,
    #[serde(default)]
    ip: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    city: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    country_code: String,
    latitude: Option<f32>,
    longitude: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Servers,
    Subscriptions,
    Settings,
    Journal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MultiHopPick {
    Entry,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Info,
    Success,
    Error,
}

struct JournalLine {
    time: String,
    text: String,
    kind: LineKind,
}

#[derive(Default)]
struct TrafficStats {
    up_rate: u64,
    down_rate: u64,
    up_total: u64,
    down_total: u64,
    history: VecDeque<(u64, u64)>,
}

struct ImportDialog {
    editing: Option<String>,
    name: String,
    input: String,
    auto_update: bool,
    opened_at: Instant,
}

#[derive(Default)]
struct MapView {
    zoom: f32,
    pan: Vec2,
    camera_animation: Option<MapCameraAnimation>,
    picker_country: Option<String>,
    picker_region: Option<String>,
    picker_opened_at: Option<Instant>,
    focus_country: Option<String>,
    /// The first frame centers the camera on Europe instead of longitude 0.
    initialized: bool,
}

struct UniverseApp {
    ctx: egui::Context,
    paths: AppPaths,
    settings: Settings,
    page: Page,
    nodes: Vec<VpnNode>,
    selected: Option<usize>,
    search: String,
    event_tx: Sender<AppEvent>,
    event_rx: Receiver<AppEvent>,
    sub_bodies: HashMap<String, String>,
    refreshing: HashSet<String>,
    last_attempt: HashMap<String, Instant>,
    last_due_check: Instant,
    installing_core: bool,
    core_progress: Option<(u64, Option<u64>)>,
    core_error: Option<String>,
    /// First-run welcome screen is shown instead of the regular layout.
    welcome: bool,
    core_path: Option<PathBuf>,
    process: Option<CoreProcess>,
    connected_raw: Option<String>,
    connected_since: Option<Instant>,
    public_ip: Option<String>,
    ip_loading: bool,
    latencies: HashMap<String, Option<u128>>,
    pending_latency: HashSet<String>,
    auto_pinged: bool,
    core_logs: VecDeque<String>,
    journal: VecDeque<JournalLine>,
    notice: String,
    notice_error: bool,
    notice_at: Instant,
    selection_changed_at: Instant,
    world_map: WorldMap,
    brand_texture: egui::TextureHandle,
    /// This computer as reported to subscription servers.
    device: DeviceInfo,
    map: MapView,
    window_maximized: bool,
    /// Whether the OS blurs what is behind the window (acrylic / blur).
    backdrop: bool,
    origin_location: Option<GeoPoint>,
    route_animation: RouteAnimation,
    connection_operation: u64,
    connection_busy: bool,
    pending_stats_api: Option<(u64, StatsApi)>,
    stats_stop: Option<Arc<AtomicBool>>,
    traffic: TrafficStats,
    server_locations: HashMap<String, ServerLocation>,
    server_geo_loading: bool,
    server_geo_refresh_pending: bool,
    multihop_pick: Option<MultiHopPick>,
    startup_connect_pending: bool,
    hide_on_start: bool,
    reconnect_at: Option<Instant>,
    reconnect_attempts: usize,
    recommendation_applied: bool,
    selection_was_manual: bool,
    zapret_process: Option<String>,
    zapret_warning_dismissed: bool,
    import: Option<ImportDialog>,
    delete_confirm: Option<(String, Instant)>,
    collapsed_groups: HashSet<String>,
    journal_tab: String,
    #[cfg(target_os = "windows")]
    tray: Option<AppTray>,
    exit_requested: bool,
}

impl UniverseApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::configure_fonts(&cc.egui_ctx);
        theme::configure_style(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let brand_texture = load_brand_texture(&cc.egui_ctx);
        #[cfg(target_os = "windows")]
        let backdrop = apply_window_backdrop(cc);
        #[cfg(not(target_os = "windows"))]
        let backdrop = false;
        let paths = AppPaths::discover().expect("не удалось определить папку данных universe");
        let server_locations = fs::read(&paths.locations)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let mut settings: Settings = fs::read(&paths.settings)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if settings.user_agent.trim().is_empty() {
            settings.user_agent = DEFAULT_USER_AGENT.to_owned();
        }
        if settings.update_interval_hours == 0 {
            settings.update_interval_hours = DEFAULT_UPDATE_HOURS;
        }

        let mut sub_bodies = HashMap::new();
        for subscription in settings.subscriptions.iter().filter(|sub| sub.is_remote()) {
            if let Ok(body) = fs::read_to_string(paths.subscription_cache(&subscription.id)) {
                sub_bodies.insert(subscription.id.clone(), body);
            }
        }

        // Existing users already have servers and never see the welcome screen.
        if !settings.subscriptions.is_empty() {
            settings.onboarding_done = true;
        }
        let welcome = !settings.onboarding_done;
        let device = DeviceInfo::current();
        let generated_hwid = device.hwid.is_empty() && settings.hwid.trim().is_empty();
        if generated_hwid {
            settings.hwid = random_hwid();
        }
        let startup_connect_pending = settings.auto_connect;
        let hide_on_start = settings.start_minimized;
        let (event_tx, event_rx) = mpsc::channel();
        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            core_path: find_core(&paths),
            paths,
            settings,
            page: Page::Home,
            nodes: Vec::new(),
            selected: None,
            search: String::new(),
            event_tx,
            event_rx,
            sub_bodies,
            refreshing: HashSet::new(),
            last_attempt: HashMap::new(),
            last_due_check: Instant::now(),
            installing_core: false,
            core_progress: None,
            core_error: None,
            welcome,
            process: None,
            connected_raw: None,
            connected_since: None,
            public_ip: None,
            ip_loading: false,
            latencies: HashMap::new(),
            pending_latency: HashSet::new(),
            auto_pinged: false,
            core_logs: VecDeque::new(),
            journal: VecDeque::new(),
            notice: String::new(),
            notice_error: false,
            notice_at: Instant::now(),
            selection_changed_at: Instant::now(),
            world_map: parse_world_map(),
            brand_texture,
            device,
            map: MapView {
                zoom: 1.0,
                ..MapView::default()
            },
            window_maximized: false,
            backdrop,
            origin_location: None,
            route_animation: RouteAnimation::new(),
            connection_operation: 0,
            connection_busy: false,
            pending_stats_api: None,
            stats_stop: None,
            traffic: TrafficStats::default(),
            server_locations,
            server_geo_loading: false,
            server_geo_refresh_pending: false,
            multihop_pick: None,
            startup_connect_pending,
            hide_on_start,
            reconnect_at: None,
            reconnect_attempts: 0,
            recommendation_applied: false,
            selection_was_manual: false,
            zapret_process: None,
            zapret_warning_dismissed: false,
            import: None,
            delete_confirm: None,
            collapsed_groups: HashSet::new(),
            journal_tab: "events".to_owned(),
            #[cfg(target_os = "windows")]
            tray: None,
            exit_requested: false,
        };
        app.log(
            LineKind::Info,
            format!("{APP_NAME} {} запущен", env!("CARGO_PKG_VERSION")),
        );
        if generated_hwid {
            app.persist_settings();
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(main_hwnd) = native_tray::main_window_handle(cc) {
                match AppTray::new(app.event_tx.clone(), cc.egui_ctx.clone(), main_hwnd) {
                    Ok(tray) => app.tray = Some(tray),
                    Err(error) => {
                        app.set_error(format!("Не удалось создать значок в трее: {error}"))
                    }
                }
            }
        }
        app.rebuild_nodes();
        if let Some(index) = app
            .nodes
            .iter()
            .position(|node| node.raw == app.settings.last_server)
        {
            app.selected = Some(index);
            app.selection_was_manual = true;
        }
        if app.settings.multihop_enabled {
            app.ensure_multihop_defaults();
        }
        app.begin_origin_location_lookup();
        app.begin_zapret_check();
        if app.settings.auto_refresh {
            app.refresh_all(false);
        }
        if app.core_path.is_none() {
            app.begin_core_install();
        }
        if app.settings.subscriptions.is_empty() {
            app.page = Page::Subscriptions;
            app.set_notice("Добавьте подписку или ключи, чтобы начать");
        } else if app.notice.is_empty() {
            app.set_notice("Готово к подключению");
        }
        app
    }

    // ── journal & notices ────────────────────────────────────────────────

    fn log(&mut self, kind: LineKind, text: impl Into<String>) {
        self.journal.push_back(JournalLine {
            time: local_time(),
            text: text.into(),
            kind,
        });
        while self.journal.len() > 500 {
            self.journal.pop_front();
        }
    }

    fn set_notice(&mut self, message: impl Into<String>) {
        let message = message.into();
        if message != self.notice || self.notice_error {
            self.log(LineKind::Info, message.clone());
        }
        self.notice = message;
        self.notice_error = false;
        self.notice_at = Instant::now();
    }

    fn set_success(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log(LineKind::Success, message.clone());
        self.notice = message;
        self.notice_error = false;
        self.notice_at = Instant::now();
    }

    fn set_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log(LineKind::Error, message.clone());
        self.notice = message;
        self.notice_error = true;
        self.notice_at = Instant::now();
    }

    // ── subscriptions ────────────────────────────────────────────────────

    fn subscription(&self, id: &str) -> Option<&Subscription> {
        self.settings.subscriptions.iter().find(|sub| sub.id == id)
    }

    fn subscription_of(&self, node: &VpnNode) -> Option<&Subscription> {
        self.subscription(&node.source)
    }

    /// Re-parses every subscription and rebuilds the server list, keeping the
    /// current selection when the server still exists.
    fn rebuild_nodes(&mut self) {
        let current = self
            .selected
            .and_then(|index| self.nodes.get(index))
            .map(|node| node.raw.clone());
        // Until the user picks a server this session, the remembered one wins:
        // it may only appear once its subscription has been downloaded.
        let selected_raw = match current {
            Some(raw) if self.selection_was_manual || self.settings.last_server.is_empty() => raw,
            _ => self.settings.last_server.clone(),
        };
        let mut nodes = Vec::new();
        let mut seen = HashSet::new();
        for subscription in &mut self.settings.subscriptions {
            let text = if subscription.is_remote() {
                self.sub_bodies
                    .get(&subscription.id)
                    .cloned()
                    .unwrap_or_default()
            } else {
                subscription.keys.clone()
            };
            let report = parse_subscription(&text);
            let mut notes = Vec::new();
            let mut count = 0;
            let mut skipped = report.ignored;
            for mut node in report.nodes {
                if node.is_placeholder() {
                    notes.push(node.name.clone());
                    continue;
                }
                if !node_is_supported(&node) {
                    skipped += 1;
                    continue;
                }
                if !seen.insert(node.raw.clone()) {
                    continue;
                }
                node.source = subscription.id.clone();
                count += 1;
                if subscription.enabled {
                    nodes.push(node);
                }
            }
            subscription.node_count = count;
            subscription.skipped = skipped;
            subscription.meta.notes = notes;
        }
        self.nodes = nodes;
        let restored = self.nodes.iter().position(|node| node.raw == selected_raw);
        if restored.is_some() && selected_raw == self.settings.last_server {
            self.selection_was_manual = true;
        }
        self.selected = restored.or_else(|| (!self.nodes.is_empty()).then_some(0));
        self.apply_server_location_labels();
        self.begin_server_location_lookup();
        if self.settings.multihop_enabled {
            self.ensure_multihop_defaults();
        }
    }

    fn refresh_all(&mut self, manual: bool) {
        let ids: Vec<String> = self
            .settings
            .subscriptions
            .iter()
            .filter(|sub| sub.is_remote() && (sub.enabled || manual))
            .map(|sub| sub.id.clone())
            .collect();
        if ids.is_empty() && manual {
            self.set_notice("Нет подписок по ссылке для обновления");
        }
        for id in ids {
            self.refresh_subscription(&id);
        }
    }

    fn refresh_subscription(&mut self, id: &str) {
        if self.refreshing.contains(id) {
            return;
        }
        let Some(subscription) = self.subscription(id) else {
            return;
        };
        if !subscription.is_remote() {
            return;
        }
        let url = subscription.url.clone();
        let user_agent = self.settings.user_agent.clone();
        let device = self.subscription_device();
        self.refreshing.insert(id.to_owned());
        self.last_attempt.insert(id.to_owned(), Instant::now());
        let sender = self.event_tx.clone();
        let ctx = self.ctx.clone();
        let id = id.to_owned();
        thread::spawn(move || {
            let result = fetch_subscription(&url, &user_agent, device.as_ref());
            let _ = sender.send(AppEvent::SubscriptionFetched { id, result });
            ctx.request_repaint();
        });
    }

    /// Device headers for subscription requests, or `None` when disabled.
    fn subscription_device(&self) -> Option<DeviceInfo> {
        self.settings.send_hwid.then(|| DeviceInfo {
            hwid: self.effective_hwid(),
            ..self.device.clone()
        })
    }

    fn effective_hwid(&self) -> String {
        if self.settings.hwid.trim().is_empty() {
            self.device.hwid.clone()
        } else {
            self.settings.hwid.trim().to_owned()
        }
    }

    fn check_due_subscriptions(&mut self) {
        if !self.settings.auto_refresh || self.last_due_check.elapsed() < Duration::from_secs(30) {
            return;
        }
        self.last_due_check = Instant::now();
        let now = now_unix();
        let due: Vec<String> = self
            .settings
            .subscriptions
            .iter()
            .filter(|sub| sub.is_due(now, self.settings.update_interval_hours))
            .filter(|sub| {
                self.last_attempt
                    .get(&sub.id)
                    .is_none_or(|attempt| attempt.elapsed() > Duration::from_secs(600))
            })
            .map(|sub| sub.id.clone())
            .collect();
        for id in due {
            self.refresh_subscription(&id);
        }
    }

    fn on_subscription_fetched(&mut self, id: String, result: Result<FetchedSubscription, String>) {
        self.refreshing.remove(&id);
        let Some(position) = self
            .settings
            .subscriptions
            .iter()
            .position(|sub| sub.id == id)
        else {
            return;
        };
        match result {
            Ok(fetched) => {
                let _ = fs::write(self.paths.subscription_cache(&id), &fetched.body);
                self.sub_bodies.insert(id.clone(), fetched.body);
                let first_load = {
                    let subscription = &mut self.settings.subscriptions[position];
                    let first = subscription.updated_at == 0 && subscription.node_count == 0;
                    subscription.meta = fetched.meta;
                    subscription.updated_at = now_unix();
                    subscription.last_error.clear();
                    first
                };
                let had_nodes = !self.nodes.is_empty();
                self.rebuild_nodes();
                if let Some(error) = self.empty_subscription_error(position) {
                    let name = self.settings.subscriptions[position].display_name();
                    self.settings.subscriptions[position].last_error = error.clone();
                    self.persist_settings();
                    self.set_error(format!("«{name}»: {error}"));
                    return;
                }
                self.persist_settings();
                let subscription = &self.settings.subscriptions[position];
                let message = format!(
                    "«{}» обновлена · {}",
                    subscription.display_name(),
                    pages::server_count_label(subscription.node_count)
                );
                if first_load && !had_nodes && !self.nodes.is_empty() {
                    self.selected = Some(0);
                    self.selection_changed_at = Instant::now();
                }
                self.set_success(message);
            }
            Err(error) => {
                let name = {
                    let subscription = &mut self.settings.subscriptions[position];
                    subscription.last_error = error.clone();
                    subscription.display_name()
                };
                self.persist_settings();
                self.set_error(format!("«{name}»: {error}"));
            }
        }
    }

    /// Explains why a successfully downloaded subscription has no servers,
    /// using the provider's own message when it sent one.
    fn empty_subscription_error(&self, position: usize) -> Option<String> {
        let subscription = &self.settings.subscriptions[position];
        if subscription.node_count > 0 {
            return None;
        }
        let note = subscription.meta.notes.first().cloned();
        let mentions_device = note.as_deref().is_some_and(|note| {
            let note = note.to_lowercase();
            note.contains("устройств") || note.contains("hwid") || note.contains("device")
        });
        let mut error = note
            .map(|note| universe::parser::split_flag(&note).1)
            .map(|note| {
                note.trim_start_matches(['❌', '⚠', '!', ' '])
                    .trim()
                    .to_owned()
            })
            .filter(|note| !note.is_empty())
            .unwrap_or_else(|| "В подписке нет поддерживаемых серверов".to_owned());
        if mentions_device {
            error.push_str(if self.settings.send_hwid {
                " — сервис не принял HWID, проверьте лимит устройств в подписке"
            } else {
                " — включите «Передавать HWID» в настройках"
            });
        }
        Some(error)
    }

    fn add_subscription(&mut self, subscription: Subscription) {
        let id = subscription.id.clone();
        let remote = subscription.is_remote();
        let had_nodes = !self.nodes.is_empty();
        self.settings.subscriptions.push(subscription);
        self.rebuild_nodes();
        self.persist_settings();
        if remote {
            self.set_notice("Загружаю подписку…");
            self.refresh_subscription(&id);
        } else {
            let count = self
                .subscription(&id)
                .map(|sub| sub.node_count)
                .unwrap_or_default();
            if !had_nodes {
                if let Some(index) = self.nodes.iter().position(|node| node.source == id) {
                    self.selected = Some(index);
                    self.selection_changed_at = Instant::now();
                }
            }
            self.set_success(format!(
                "Ключи добавлены · {}",
                pages::server_count_label(count)
            ));
        }
    }

    fn remove_subscription(&mut self, id: &str) {
        let Some(position) = self
            .settings
            .subscriptions
            .iter()
            .position(|sub| sub.id == id)
        else {
            return;
        };
        let removed = self.settings.subscriptions.remove(position);
        let _ = fs::remove_file(self.paths.subscription_cache(id));
        self.sub_bodies.remove(id);
        self.refreshing.remove(id);
        if self.settings.source_filter == id {
            self.settings.source_filter = "all".to_owned();
        }
        self.rebuild_nodes();
        self.persist_settings();
        self.set_notice(format!("Подписка «{}» удалена", removed.display_name()));
    }

    fn add_public_feed(&mut self) {
        if self
            .settings
            .subscriptions
            .iter()
            .any(|sub| sub.url == PUBLIC_FEED_URL)
        {
            self.set_notice("Публичные серверы уже добавлены");
            return;
        }
        let mut subscription = Subscription::remote("Публичные серверы", PUBLIC_FEED_URL);
        subscription.meta.announce =
            "Бесплатные серверы неизвестных владельцев. Не используйте их для важных данных."
                .to_owned();
        self.add_subscription(subscription);
    }

    // ── background lookups ───────────────────────────────────────────────

    fn begin_core_install(&mut self) {
        if self.installing_core {
            return;
        }
        self.installing_core = true;
        self.core_progress = Some((0, None));
        self.core_error = None;
        self.set_notice(format!(
            "Скачиваю VPN-ядро sing-box {}…",
            universe::singbox::CORE_VERSION
        ));
        let paths = self.paths.clone();
        let sender = self.event_tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let mut last_report = Instant::now();
            let progress_sender = sender.clone();
            let progress_ctx = ctx.clone();
            let result = install_core_with_progress(&paths, |downloaded, total| {
                let finished = total.is_some_and(|total| downloaded >= total);
                if finished || last_report.elapsed() >= Duration::from_millis(80) {
                    last_report = Instant::now();
                    let _ = progress_sender.send(AppEvent::CoreProgress { downloaded, total });
                    progress_ctx.request_repaint();
                }
            })
            .map_err(|error| error.to_string());
            let _ = sender.send(AppEvent::CoreInstalled(result));
            ctx.request_repaint();
        });
    }

    fn begin_latency_test(&mut self, nodes: Vec<VpnNode>) {
        let nodes: Vec<VpnNode> = nodes
            .into_iter()
            .filter(|node| !node.protocol.is_udp() && !self.pending_latency.contains(&node.raw))
            .collect();
        if nodes.is_empty() {
            return;
        }
        for node in &nodes {
            self.pending_latency.insert(node.raw.clone());
        }
        let queue = Arc::new(Mutex::new(nodes));
        for _ in 0..8 {
            let queue = Arc::clone(&queue);
            let sender = self.event_tx.clone();
            let ctx = self.ctx.clone();
            thread::spawn(move || {
                // The closure releases the lock before the slow TCP probe runs.
                let next = || queue.lock().ok().and_then(|mut queue| queue.pop());
                while let Some(node) = next() {
                    let value = measure_tcp_latency(&node);
                    let _ = sender.send(AppEvent::Latency {
                        raw: node.raw,
                        value,
                    });
                    ctx.request_repaint();
                }
            });
        }
    }

    fn begin_origin_location_lookup(&self) {
        let sender = self.event_tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let result = (|| -> Result<GeoPoint, String> {
                let location = http_client(Duration::from_secs(10))?
                    .get("https://ipapi.co/json/")
                    .send()
                    .map_err(|error| error.to_string())?
                    .error_for_status()
                    .map_err(|error| error.to_string())?
                    .json::<IpLocationResponse>()
                    .map_err(|error| error.to_string())?;
                if !location.latitude.is_finite() || !location.longitude.is_finite() {
                    return Err("сервис геолокации вернул некорректные координаты".to_owned());
                }
                Ok(GeoPoint {
                    latitude: location.latitude.clamp(-85.0, 85.0),
                    longitude: location.longitude.clamp(-180.0, 180.0),
                })
            })();
            let _ = sender.send(AppEvent::OriginLocation(result));
            ctx.request_repaint();
        });
    }

    fn begin_zapret_check(&self) {
        let sender = self.event_tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let detected = detect_zapret_process();
            let _ = sender.send(AppEvent::ZapretStatus(detected));
            ctx.request_repaint();
        });
    }

    fn begin_server_location_lookup(&mut self) {
        if self.server_geo_loading {
            self.server_geo_refresh_pending = true;
            return;
        }
        let mut targets = BTreeMap::<String, u16>::new();
        for node in &self.nodes {
            if self
                .server_locations
                .get(&node.host)
                .is_none_or(|location| {
                    location.country.trim().is_empty() || location.country_code.trim().is_empty()
                })
            {
                targets.entry(node.host.clone()).or_insert(node.port);
            }
        }
        if targets.is_empty() {
            return;
        }

        self.server_geo_loading = true;
        let queue = Arc::new(Mutex::new(
            targets.into_iter().collect::<Vec<(String, u16)>>(),
        ));
        let discovered = Arc::new(Mutex::new(HashMap::<String, ServerLocation>::new()));
        let sender = self.event_tx.clone();
        let cache_path = self.paths.locations.clone();
        let mut cached = self.server_locations.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let Ok(client) = http_client(Duration::from_secs(10)) else {
                let _ = sender.send(AppEvent::ServerLocations(cached));
                ctx.request_repaint();
                return;
            };
            let mut workers = Vec::new();
            for _ in 0..4 {
                let queue = Arc::clone(&queue);
                let discovered = Arc::clone(&discovered);
                let client = client.clone();
                workers.push(thread::spawn(move || loop {
                    let target = queue.lock().ok().and_then(|mut queue| queue.pop());
                    let Some((host, port)) = target else {
                        break;
                    };
                    if let Some(location) = lookup_server_location(&client, &host, port) {
                        if let Ok(mut locations) = discovered.lock() {
                            locations.insert(host, location);
                        }
                    }
                }));
            }
            for worker in workers {
                let _ = worker.join();
            }
            if let Ok(locations) = discovered.lock() {
                cached.extend(locations.clone());
            }
            if let Ok(bytes) = serde_json::to_vec_pretty(&cached) {
                let _ = fs::write(cache_path, bytes);
            }
            let _ = sender.send(AppEvent::ServerLocations(cached));
            ctx.request_repaint();
        });
    }

    fn apply_server_location_labels(&mut self) {
        for node in &mut self.nodes {
            let Some(location) = self.server_locations.get(&node.host) else {
                continue;
            };
            if !location.country.trim().is_empty() {
                node.country = location.country.trim().to_owned();
            }
            if node.flag.is_empty() || theme::flag_code(&node.flag).is_none() {
                if let Some(flag) = theme::country_code_flag(&location.country_code) {
                    node.flag = flag;
                }
            }
        }
    }

    fn begin_public_ip_lookup(&mut self, operation: u64, raw: String) {
        self.public_ip = None;
        self.ip_loading = true;
        let sender = self.event_tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1200));
            let result = (|| -> Result<String, String> {
                let value = http_client(Duration::from_secs(12))?
                    .get("https://api64.ipify.org")
                    .send()
                    .map_err(|error| error.to_string())?
                    .error_for_status()
                    .map_err(|error| error.to_string())?
                    .text()
                    .map_err(|error| error.to_string())?;
                let value = value.trim();
                IpAddr::from_str(value).map_err(|_| "сервис вернул некорректный IP".to_owned())?;
                Ok(value.to_owned())
            })();
            let _ = sender.send(AppEvent::PublicIp {
                operation,
                raw,
                result,
            });
            ctx.request_repaint();
        });
    }

    fn start_traffic_monitor(&mut self, operation: u64, api: StatsApi) {
        self.stop_traffic_monitor();
        let stop = Arc::new(AtomicBool::new(false));
        self.stats_stop = Some(Arc::clone(&stop));
        let sender = self.event_tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let Ok(client) = reqwest::blocking::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(2))
                .timeout(None::<Duration>)
                .build()
            else {
                return;
            };
            let mut failures = 0;
            while !stop.load(Ordering::Relaxed) && failures < 30 {
                let response = client
                    .get(format!("http://{}/traffic", api.address))
                    .bearer_auth(&api.secret)
                    .send()
                    .and_then(|response| response.error_for_status());
                let Ok(response) = response else {
                    failures += 1;
                    thread::sleep(Duration::from_millis(700));
                    continue;
                };
                for line in std::io::BufReader::new(response).lines() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(line) = line else {
                        break;
                    };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    failures = 0;
                    let up = value.get("up").and_then(|v| v.as_u64()).unwrap_or_default();
                    let down = value
                        .get("down")
                        .and_then(|v| v.as_u64())
                        .unwrap_or_default();
                    if sender
                        .send(AppEvent::Traffic {
                            operation,
                            up,
                            down,
                        })
                        .is_err()
                    {
                        return;
                    }
                    ctx.request_repaint();
                }
                failures += 1;
                thread::sleep(Duration::from_millis(500));
            }
        });
    }

    fn stop_traffic_monitor(&mut self) {
        if let Some(stop) = self.stats_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.traffic = TrafficStats::default();
    }

    // ── events ───────────────────────────────────────────────────────────

    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::SubscriptionFetched { id, result } => {
                    self.on_subscription_fetched(id, result)
                }
                AppEvent::CoreProgress { downloaded, total } => {
                    if self.installing_core {
                        self.core_progress = Some((downloaded, total));
                    }
                }
                AppEvent::CoreInstalled(result) => {
                    self.installing_core = false;
                    self.core_progress = None;
                    if let Err(error) = &result {
                        self.core_error = Some(error.clone());
                    }
                    match result {
                        Ok(path) => {
                            self.core_path = Some(path);
                            self.set_success(format!(
                                "VPN-ядро sing-box {} установлено",
                                universe::singbox::CORE_VERSION
                            ));
                        }
                        Err(error) => self.set_error(format!("Ядро не установлено: {error}")),
                    }
                }
                AppEvent::OriginLocation(result) => {
                    if let Ok(location) = result {
                        self.origin_location = Some(location);
                        if self.route_animation.phase != RoutePhase::Idle
                            && self.route_animation.from.is_none()
                        {
                            self.route_animation.from = Some(location);
                        }
                        self.apply_initial_recommendation();
                    }
                }
                AppEvent::ServerLocations(locations) => {
                    self.server_locations.extend(locations);
                    self.apply_server_location_labels();
                    self.server_geo_loading = false;
                    self.apply_initial_recommendation();
                    if self.server_geo_refresh_pending {
                        self.server_geo_refresh_pending = false;
                        self.begin_server_location_lookup();
                    }
                }
                AppEvent::ConnectFinished {
                    operation,
                    raw,
                    name,
                    result,
                } => {
                    if operation != self.connection_operation {
                        if let Ok(mut stale_process) = result {
                            thread::spawn(move || stale_process.stop());
                        }
                        continue;
                    }
                    self.connection_busy = false;
                    match result {
                        Ok(process) => {
                            self.reconnect_at = None;
                            self.process = Some(process);
                            self.connected_raw = Some(raw.clone());
                            self.connected_since = Some(Instant::now());
                            self.set_success(format!("Защищено · {name}"));
                            if let Some((pending, api)) = self.pending_stats_api.take() {
                                if pending == operation {
                                    self.start_traffic_monitor(operation, api);
                                }
                            }
                            self.begin_public_ip_lookup(operation, raw);
                        }
                        Err(error) => {
                            self.route_animation.failed();
                            self.set_error(error);
                            self.schedule_reconnect();
                        }
                    }
                }
                AppEvent::DisconnectFinished { operation } => {
                    if operation == self.connection_operation {
                        self.connection_busy = false;
                        self.set_notice("VPN отключён");
                    }
                }
                AppEvent::Latency { raw, value } => {
                    self.pending_latency.remove(&raw);
                    self.latencies.insert(raw, value);
                }
                AppEvent::PublicIp {
                    operation,
                    raw,
                    result,
                } => {
                    if operation == self.connection_operation
                        && self.connected_raw.as_deref() == Some(raw.as_str())
                    {
                        self.ip_loading = false;
                        self.public_ip = Some(match result {
                            Ok(ip) => {
                                self.log(LineKind::Success, format!("Внешний IP: {ip}"));
                                ip
                            }
                            Err(_) => "не удалось определить".to_owned(),
                        });
                        self.route_animation.connected();
                    }
                }
                AppEvent::Traffic {
                    operation,
                    up,
                    down,
                } => {
                    if operation == self.connection_operation && self.process.is_some() {
                        self.traffic.up_rate = up;
                        self.traffic.down_rate = down;
                        self.traffic.up_total += up;
                        self.traffic.down_total += down;
                        self.traffic.history.push_back((up, down));
                        while self.traffic.history.len() > 60 {
                            self.traffic.history.pop_front();
                        }
                    }
                }
                AppEvent::ZapretStatus(process) => {
                    if !self.zapret_warning_dismissed {
                        self.zapret_process = process;
                    }
                }
                #[cfg(target_os = "windows")]
                AppEvent::Tray(action) => self.handle_tray_action(action, ctx),
            }
        }
        let _ = ctx;
    }

    fn poll_process(&mut self) {
        let mut exited = None;
        if let Some(process) = &mut self.process {
            for line in process.drain_logs() {
                self.core_logs.push_back(line);
            }
            while self.core_logs.len() > 600 {
                self.core_logs.pop_front();
            }
            match process.try_wait() {
                Ok(Some(status)) => exited = Some(format!("VPN-ядро остановилось: {status}")),
                Err(error) => exited = Some(error.to_string()),
                Ok(None) => {}
            }
        }
        if let Some(message) = exited {
            if self
                .connected_since
                .is_some_and(|since| since.elapsed() > Duration::from_secs(30))
            {
                self.reconnect_attempts = 0;
            }
            self.process = None;
            self.stop_traffic_monitor();
            self.connection_busy = false;
            self.connected_raw = None;
            self.connected_since = None;
            self.public_ip = None;
            self.ip_loading = false;
            self.route_animation.failed();
            self.set_error(message);
            self.schedule_reconnect();
        }
    }

    /// Kill switch keeps trying with a growing delay, then gives up.
    fn schedule_reconnect(&mut self) {
        if !self.settings.kill_switch {
            return;
        }
        let Some(delay) = RECONNECT_DELAYS.get(self.reconnect_attempts).copied() else {
            self.reconnect_at = None;
            self.set_error("Не удалось восстановить соединение — выберите другой сервер");
            return;
        };
        self.reconnect_attempts += 1;
        self.reconnect_at = Some(Instant::now() + Duration::from_secs(delay));
        self.log(
            LineKind::Info,
            format!(
                "Повторное подключение через {delay} с (попытка {} из {})",
                self.reconnect_attempts,
                RECONNECT_DELAYS.len()
            ),
        );
    }

    // ── connection ───────────────────────────────────────────────────────

    fn connect(&mut self, user_initiated: bool) {
        if self.connection_busy {
            return;
        }
        if user_initiated {
            self.reconnect_attempts = 0;
        }
        let Some(index) = self.connection_target_index() else {
            self.set_error(if self.settings.multihop_enabled {
                "Выходной сервер MultiHop недоступен — выберите другой"
            } else {
                "Сначала выберите сервер"
            });
            return;
        };
        self.selected = Some(index);
        let Some(node) = self.nodes.get(index).cloned() else {
            self.set_error("Выбранный сервер больше недоступен");
            return;
        };
        if self.settings.multihop_enabled {
            let Some(entry) = self
                .nodes
                .iter()
                .find(|candidate| candidate.raw == self.settings.multihop_entry)
            else {
                self.set_error("Выберите входной сервер MultiHop");
                return;
            };
            if entry.raw == node.raw {
                self.set_error("Для MultiHop выберите два разных сервера");
                return;
            }
        }
        let target = self.node_point(&node);
        let via = self
            .settings
            .multihop_enabled
            .then(|| {
                self.nodes
                    .iter()
                    .find(|candidate| candidate.raw == self.settings.multihop_entry)
                    .and_then(|entry| self.node_point(entry))
            })
            .flatten();
        self.route_animation
            .begin(self.origin_location, via, target);
        let Some(core) = self.core_path.clone().or_else(|| find_core(&self.paths)) else {
            self.route_animation.failed();
            self.set_error("VPN-ядро ещё не установлено");
            if !self.installing_core {
                self.begin_core_install();
            }
            return;
        };
        self.connection_operation = self.connection_operation.wrapping_add(1);
        let operation = self.connection_operation;
        self.connection_busy = true;
        self.stop_traffic_monitor();
        let old_process = self.process.take();
        self.core_logs.clear();
        self.connected_raw = None;
        self.connected_since = None;
        self.public_ip = None;
        self.ip_loading = false;
        self.settings.last_server = node.raw.clone();
        self.persist_settings();
        let display = node.title().to_owned();
        if self.settings.multihop_enabled {
            let entry = self
                .nodes
                .iter()
                .find(|candidate| candidate.raw == self.settings.multihop_entry)
                .map(|candidate| candidate.title().to_owned())
                .unwrap_or_else(|| "?".to_owned());
            self.set_notice(format!("MultiHop · {entry} → {display}"));
        } else {
            self.set_notice(format!("Подключаюсь · {display}"));
        }

        let mut tunnel_options = self.tunnel_options();
        let stats_api = free_local_port().map(|port| StatsApi {
            address: format!("127.0.0.1:{port}"),
            secret: new_id("stats"),
        });
        tunnel_options.stats_api = stats_api.clone();
        self.pending_stats_api = stats_api.map(|api| (operation, api));

        let sender = self.event_tx.clone();
        let paths = self.paths.clone();
        let raw = node.raw.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            if let Some(mut process) = old_process {
                process.stop();
            }
            let result = (|| {
                let config = write_runtime_config_with_options(&paths, &node, &tunnel_options)?;
                check_config(&core, &config)?;
                CoreProcess::start(&core, &config, &paths.root)
            })()
            .map_err(|error| error.to_string());
            let _ = sender.send(AppEvent::ConnectFinished {
                operation,
                raw,
                name: display,
                result,
            });
            ctx.request_repaint();
        });
        self.ctx.request_repaint();
    }

    fn disconnect(&mut self) {
        if self.connection_busy {
            return;
        }
        self.route_animation.disconnect();
        self.reconnect_at = None;
        self.reconnect_attempts = 0;
        self.connection_operation = self.connection_operation.wrapping_add(1);
        let operation = self.connection_operation;
        self.stop_traffic_monitor();
        let process = self.process.take();
        self.connected_raw = None;
        self.connected_since = None;
        self.public_ip = None;
        self.ip_loading = false;
        let Some(mut process) = process else {
            self.set_notice("VPN отключён");
            return;
        };
        self.set_notice("Отключаю VPN…");
        self.connection_busy = true;
        let sender = self.event_tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            process.stop();
            let _ = sender.send(AppEvent::DisconnectFinished { operation });
            ctx.request_repaint();
        });
    }

    /// Main button: connect, switch to the selected server, or disconnect.
    fn toggle_connection(&mut self) {
        if self.connection_busy {
            return;
        }
        let target = self
            .connection_target_index()
            .and_then(|index| self.nodes.get(index))
            .map(|node| node.raw.clone());
        let same = target.is_some() && target == self.connected_raw;
        if same || (self.process.is_some() && target.is_none()) {
            self.disconnect();
        } else if self.reconnect_at.is_some() && self.process.is_none() {
            self.reconnect_at = None;
            self.reconnect_attempts = 0;
            self.route_animation.disconnect();
            self.set_notice("Повторное подключение отменено");
        } else {
            self.connect(true);
        }
    }

    fn node_point(&self, node: &VpnNode) -> Option<GeoPoint> {
        self.server_locations
            .get(&node.host)
            .map(|location| GeoPoint {
                longitude: location.longitude,
                latitude: location.latitude,
            })
            .or_else(|| {
                country_coordinates(&node.country).map(|(longitude, latitude)| GeoPoint {
                    longitude,
                    latitude,
                })
            })
    }

    fn select_node(&mut self, index: usize) {
        if index >= self.nodes.len() {
            return;
        }
        if let Some(stage) = self.multihop_pick {
            let node = self.nodes[index].clone();
            match stage {
                MultiHopPick::Entry if node.raw == self.settings.multihop_exit => {
                    self.set_error("Входной и выходной серверы MultiHop должны отличаться");
                    return;
                }
                MultiHopPick::Exit if node.raw == self.settings.multihop_entry => {
                    self.set_error("Входной и выходной серверы MultiHop должны отличаться");
                    return;
                }
                MultiHopPick::Entry => {
                    self.settings.multihop_entry = node.raw.clone();
                    self.set_notice(format!("Вход MultiHop · {}", node.title()));
                }
                MultiHopPick::Exit => {
                    self.settings.multihop_exit = node.raw.clone();
                    self.selected = Some(index);
                    self.selection_changed_at = Instant::now();
                    self.set_notice(format!("Выход MultiHop · {}", node.title()));
                }
            }
            self.multihop_pick = None;
            self.selection_was_manual = true;
            self.persist_settings();
            return;
        }
        self.selection_was_manual = true;
        if self.selected != Some(index) {
            self.selected = Some(index);
            self.selection_changed_at = Instant::now();
        }
        self.settings.last_server = self.nodes[index].raw.clone();
        if self.settings.multihop_enabled {
            self.settings.multihop_exit = self.nodes[index].raw.clone();
            self.ensure_multihop_defaults();
        }
        self.persist_settings();
        if !self.latencies.contains_key(&self.nodes[index].raw) {
            self.begin_latency_test(vec![self.nodes[index].clone()]);
        }
    }

    fn tunnel_options(&self) -> TunnelOptions {
        let (process_names, process_paths) = split_process_entries(&self.settings.split_apps);
        let domains = settings_entries(&self.settings.split_domains)
            .into_iter()
            .filter_map(|entry| normalize_domain_rule(&entry))
            .collect();
        let multihop_entry = self
            .settings
            .multihop_enabled
            .then(|| {
                self.nodes
                    .iter()
                    .find(|node| node.raw == self.settings.multihop_entry)
                    .cloned()
            })
            .flatten();
        let route_flow = if self.settings.route_flow_enabled {
            self.settings
                .route_flow
                .iter()
                .filter(|rule| rule.enabled)
                .filter_map(|rule| {
                    let node = self
                        .nodes
                        .iter()
                        .find(|node| node.raw == rule.server)?
                        .clone();
                    let (process_names, process_paths) = split_process_entries(&rule.apps);
                    let domains = settings_entries(&rule.domains)
                        .into_iter()
                        .filter_map(|entry| normalize_domain_rule(&entry))
                        .collect::<Vec<_>>();
                    (!process_names.is_empty() || !process_paths.is_empty() || !domains.is_empty())
                        .then_some(RouteFlowRule {
                            node,
                            process_names,
                            process_paths,
                            domains,
                        })
                })
                .collect()
        } else {
            Vec::new()
        };
        TunnelOptions {
            strict_route: self.settings.kill_switch,
            allow_lan: self.settings.allow_lan,
            secure_dns: self.settings.secure_dns,
            split_tunnel: self.settings.split_tunnel,
            split_mode: if self.settings.split_mode == "only" {
                SplitTunnelMode::OnlySelected
            } else {
                SplitTunnelMode::BypassSelected
            },
            split_process_names: process_names,
            split_process_paths: process_paths,
            split_domains: domains,
            multihop_entry,
            route_flow,
            stats_api: None,
        }
    }

    fn ensure_multihop_defaults(&mut self) {
        if self.nodes.len() < 2 {
            return;
        }
        let exit_is_valid = self
            .nodes
            .iter()
            .any(|node| node.raw == self.settings.multihop_exit);
        if !exit_is_valid {
            self.settings.multihop_exit = self
                .selected
                .and_then(|index| self.nodes.get(index))
                .unwrap_or(&self.nodes[0])
                .raw
                .clone();
        }
        let entry_is_valid = self.nodes.iter().any(|node| {
            node.raw == self.settings.multihop_entry && node.raw != self.settings.multihop_exit
        });
        if !entry_is_valid {
            if let Some(entry) = self
                .nodes
                .iter()
                .find(|node| node.raw != self.settings.multihop_exit)
            {
                self.settings.multihop_entry = entry.raw.clone();
            }
        }
        if let Some(index) = self
            .nodes
            .iter()
            .position(|node| node.raw == self.settings.multihop_exit)
        {
            self.selected = Some(index);
        }
    }

    fn set_multihop(&mut self, enabled: bool) {
        if self.settings.multihop_enabled == enabled {
            return;
        }
        self.settings.multihop_enabled = enabled;
        if enabled {
            self.ensure_multihop_defaults();
        } else {
            self.multihop_pick = None;
        }
        self.persist_settings();
        if self.process.is_some() {
            self.set_notice("Режим изменён · переподключитесь, чтобы применить");
        }
    }

    fn connection_target_index(&self) -> Option<usize> {
        resolve_connection_target(
            &self.nodes,
            self.selected,
            self.settings.multihop_enabled,
            &self.settings.multihop_exit,
        )
    }

    fn synchronize_multihop_target(&mut self) {
        if self.settings.multihop_enabled {
            self.selected = self.connection_target_index();
        }
    }

    fn connected_node(&self) -> Option<&VpnNode> {
        let raw = self.connected_raw.as_deref()?;
        self.nodes.iter().find(|node| node.raw == raw)
    }

    fn persist_settings(&mut self) {
        let result = serde_json::to_vec_pretty(&self.settings)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                let temporary = self.paths.settings.with_extension("json.tmp");
                fs::write(&temporary, bytes)?;
                fs::rename(&temporary, &self.paths.settings)?;
                Ok(())
            });
        if let Err(error) = result {
            self.set_error(format!("Не удалось сохранить настройки: {error}"));
        }
    }

    fn visible_indices(&self) -> Vec<usize> {
        let query = self.search.trim().to_lowercase();
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node_matches_filters(node, &self.settings.protocol_filter))
            .filter(|(_, node)| {
                self.settings.source_filter == "all" || node.source == self.settings.source_filter
            })
            .filter(|(_, node)| query.is_empty() || node.searchable_text().contains(&query))
            .map(|(index, _)| index)
            .collect()
    }

    fn recommended_index(&self) -> Option<(usize, f32)> {
        let origin = self.origin_location?;
        (0..self.nodes.len())
            .filter_map(|index| {
                let node = self.nodes.get(index)?;
                let location = self.server_locations.get(&node.host)?;
                let distance = haversine_km(
                    origin,
                    GeoPoint {
                        longitude: location.longitude,
                        latitude: location.latitude,
                    },
                );
                distance.is_finite().then_some((index, distance))
            })
            .min_by(|(_, left), (_, right)| left.total_cmp(right))
    }

    fn apply_initial_recommendation(&mut self) {
        if self.settings.multihop_enabled
            || self.recommendation_applied
            || self.selection_was_manual
            || self.connected_raw.is_some()
        {
            return;
        }
        let Some((index, _)) = self.recommended_index() else {
            return;
        };
        self.selected = Some(index);
        self.selection_changed_at = Instant::now();
        self.recommendation_applied = true;
        self.set_notice(format!(
            "Рекомендуем ближайший сервер · {}",
            self.nodes[index].title()
        ));
    }

    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "windows")]
        if self.tray.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn show_from_tray(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    #[cfg(target_os = "windows")]
    fn handle_tray_action(&mut self, action: TrayAction, ctx: &egui::Context) {
        match action {
            TrayAction::Show => self.show_from_tray(ctx),
            TrayAction::Settings => {
                self.page = Page::Settings;
                self.show_from_tray(ctx);
            }
            TrayAction::ToggleConnection { hide_after } => {
                if !self.connection_busy {
                    if self.process.is_some() {
                        self.disconnect();
                    } else {
                        if !self.settings.multihop_enabled && self.selected.is_none() {
                            if let Some((index, _)) = self.recommended_index() {
                                self.selected = Some(index);
                                self.selection_changed_at = Instant::now();
                            }
                        }
                        self.connect(true);
                    }
                }
                if hide_after {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
            }
            TrayAction::Exit => {
                self.exit_requested = true;
                self.reconnect_at = None;
                self.stop_traffic_monitor();
                if let Some(mut process) = self.process.take() {
                    process.stop();
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn finish_welcome(&mut self, page: Page) {
        self.welcome = false;
        self.settings.onboarding_done = true;
        self.page = page;
        self.persist_settings();
    }

    fn open_path(&mut self, target: &str) {
        let lower = target.to_ascii_lowercase();
        let is_url = lower.starts_with("https://") || lower.starts_with("http://");
        let is_local = std::path::Path::new(target).exists();
        if !is_url && !is_local {
            self.set_error("Не удалось открыть ссылку");
            return;
        }
        #[cfg(target_os = "windows")]
        let result = Command::new("explorer.exe").arg(target).spawn();
        #[cfg(not(target_os = "windows"))]
        let result = Command::new("xdg-open").arg(target).spawn();
        if let Err(error) = result {
            self.set_error(format!("Не удалось открыть: {error}"));
        }
    }
}

impl eframe::App for UniverseApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.hide_on_start {
            self.hide_on_start = false;
            self.hide_to_tray(ctx);
        }
        self.handle_events(ctx);
        self.synchronize_multihop_target();
        self.poll_process();
        if !self.exit_requested && ctx.input(|input| input.viewport().close_requested()) {
            #[cfg(target_os = "windows")]
            if self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
            }
        }
        if self.startup_connect_pending
            && self.core_path.is_some()
            && !self.nodes.is_empty()
            && !self.connection_busy
        {
            self.startup_connect_pending = false;
            self.connect(true);
        }
        if self
            .reconnect_at
            .is_some_and(|deadline| Instant::now() >= deadline)
            && self.process.is_none()
            && !self.connection_busy
        {
            self.reconnect_at = None;
            self.connect(false);
        }
        self.check_due_subscriptions();
        #[cfg(target_os = "windows")]
        {
            let tray_name = self.connected_node().map(|node| node.title().to_owned());
            let can_connect =
                self.connection_target_index().is_some() || self.recommended_index().is_some();
            if let Some(tray) = &mut self.tray {
                tray.sync(tray_name.as_deref(), self.connection_busy, can_connect);
            }
        }
        let route_animating = self.route_animation.tick();
        self.draw(ctx);
        pages::handle_window_resize(ctx, self.window_maximized);
        if self.process.is_some()
            || !self.refreshing.is_empty()
            || self.installing_core
            || !self.pending_latency.is_empty()
            || self.ip_loading
            || self.reconnect_at.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if route_animating {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        // Keep draining tray events and timers even while the window is hidden.
        ctx.request_repaint_after(Duration::from_millis(500));
        #[cfg(target_os = "windows")]
        if self.tray.is_some() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_traffic_monitor();
        if let Some(mut process) = self.process.take() {
            process.stop();
        }
    }
}

fn http_client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("universe/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5).min(timeout))
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())
}

fn lookup_server_location(
    client: &reqwest::blocking::Client,
    host: &str,
    port: u16,
) -> Option<ServerLocation> {
    let ip = IpAddr::from_str(host.trim()).ok().or_else(|| {
        (host, port)
            .to_socket_addrs()
            .ok()?
            .map(|address| address.ip())
            .find(|address| !address.is_loopback() && !address.is_unspecified())
    })?;
    let response = client
        .get(format!("https://ipwho.is/{ip}?lang=ru"))
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.json::<IpWhoResponse>())
        .ok()?;
    let (Some(latitude), Some(longitude)) = (response.latitude, response.longitude) else {
        return None;
    };
    if !response.success || !latitude.is_finite() || !longitude.is_finite() {
        return None;
    }
    let region = if response.region.trim().is_empty() {
        if response.city.trim().is_empty() {
            "Другой регион".to_owned()
        } else {
            response.city.trim().to_owned()
        }
    } else {
        response.region.trim().to_owned()
    };
    Some(ServerLocation {
        ip: if response.ip.trim().is_empty() {
            ip.to_string()
        } else {
            response.ip
        },
        country: response.country.trim().to_owned(),
        country_code: response.country_code.trim().to_uppercase(),
        region,
        city: response.city.trim().to_owned(),
        latitude: latitude.clamp(-85.0, 85.0),
        longitude: longitude.clamp(-180.0, 180.0),
    })
}

fn measure_tcp_latency(node: &VpnNode) -> Option<u128> {
    let addresses: Vec<_> = (node.host.as_str(), node.port)
        .to_socket_addrs()
        .ok()?
        .collect();
    let started = Instant::now();
    addresses.iter().find_map(|address| {
        TcpStream::connect_timeout(address, Duration::from_millis(2000)).ok()
    })?;
    Some(started.elapsed().as_millis())
}

fn free_local_port() -> Option<u16> {
    TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()
        .map(|address| address.port())
}

fn split_process_entries(value: &str) -> (Vec<String>, Vec<String>) {
    let mut names = Vec::new();
    let mut paths = Vec::new();
    for entry in settings_entries(value) {
        if entry.contains('\\') || entry.contains('/') || entry.contains(':') {
            paths.push(entry);
        } else {
            names.push(entry);
        }
    }
    (names, paths)
}

fn settings_entries(value: &str) -> Vec<String> {
    let mut entries = Vec::new();
    for entry in value.split(['\n', '\r', ',', ';']) {
        let entry = entry.trim().trim_matches('"').trim_matches('\'');
        if !entry.is_empty() && !entries.iter().any(|existing| existing == entry) {
            entries.push(entry.to_owned());
        }
    }
    entries
}

fn normalize_domain_rule(value: &str) -> Option<String> {
    let mut domain = value.trim().to_ascii_lowercase();
    if let Ok(url) = url::Url::parse(&domain) {
        domain = url.host_str()?.to_owned();
    } else {
        domain = domain
            .trim_start_matches("*.")
            .trim_start_matches('.')
            .split('/')
            .next()
            .unwrap_or_default()
            .to_owned();
    }
    (!domain.is_empty()
        && domain.contains('.')
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
    .then_some(domain)
}

fn resolve_connection_target(
    nodes: &[VpnNode],
    selected: Option<usize>,
    multihop_enabled: bool,
    multihop_exit: &str,
) -> Option<usize> {
    if multihop_enabled {
        nodes.iter().position(|node| node.raw == multihop_exit)
    } else {
        selected.filter(|index| *index < nodes.len())
    }
}

fn node_matches_filters(node: &VpnNode, protocol_filter: &str) -> bool {
    protocol_filter == "all" || node.protocol.scheme() == protocol_filter
}

fn detect_zapret_process() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = Command::new("tasklist.exe")
            .args(["/FO", "CSV", "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        let processes = String::from_utf8_lossy(&output.stdout).to_lowercase();
        for process in ["winws2.exe", "winws.exe"] {
            if processes.contains(process) {
                return Some(process.to_owned());
            }
        }
    }
    None
}

fn local_time() -> String {
    #[cfg(target_os = "windows")]
    {
        let mut time = unsafe { std::mem::zeroed() };
        unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut time) };
        format!("{:02}:{:02}:{:02}", time.wHour, time.wMinute, time.wSecond)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let seconds = now_unix() % 86_400;
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }
}

/// Dark acrylic behind the window (blur on Windows 10) plus native rounded corners.
#[cfg(target_os = "windows")]
fn apply_window_backdrop(cc: &eframe::CreationContext<'_>) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_ROUND,
    };
    use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};

    if let Some(hwnd) = native_tray::main_window_handle(cc) {
        let dark: i32 = 1;
        let corner = DWMWCP_ROUND;
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as _,
                (&dark as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as _,
                (&corner as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
    let mut version: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    version.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    let build = if unsafe { GetVersionExW(&mut version) } != 0 {
        version.dwBuildNumber
    } else {
        0
    };
    let tint = Some((14, 14, 18, 120));
    // The legacy acrylic path stutters while dragging, so older builds get plain blur.
    if build >= 22_523 {
        window_vibrancy::apply_acrylic(cc, tint).is_ok()
    } else {
        window_vibrancy::apply_blur(cc, tint).is_ok()
    }
}

fn load_brand_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let decoded = image::load_from_memory(BRAND_ICON)
        .expect("embedded brand icon must be a valid PNG")
        .to_rgba8();
    let size = [decoded.width() as usize, decoded.height() as usize];
    let pixels = decoded.into_raw();
    ctx.load_texture(
        "universe-brand-icon",
        egui::ColorImage::from_rgba_unmultiplied(size, &pixels),
        egui::TextureOptions::LINEAR,
    )
}

fn load_window_icon() -> egui::IconData {
    let source = image::load_from_memory(BRAND_ICON)
        .expect("embedded brand icon must be a valid PNG")
        .to_rgba8();
    let resized = image::imageops::resize(&source, 256, 256, image::imageops::FilterType::Lanczos3);
    egui::IconData {
        rgba: resized.into_raw(),
        width: 256,
        height: 256,
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id(APP_NAME)
            .with_icon(load_window_icon())
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(true)
            .with_resizable(true)
            .with_inner_size([920.0, 600.0])
            .with_min_inner_size([920.0, 600.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(UniverseApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FALLBACK_CONFIGS: &str = include_str!("../assets/default-configs.txt");

    #[test]
    fn multihop_connection_uses_chosen_exit_even_when_another_server_is_selected() {
        let report = parse_subscription(
            "vless://entry@example.org:443?security=tls#Entry\n\
             trojan://exit@example.net:443?security=tls#Exit\n\
             trojan://nearest@example.com:443?security=tls#Nearest",
        );
        let nodes = report.nodes;
        assert_eq!(nodes.len(), 3);
        let exit = nodes[1].raw.clone();
        assert_eq!(
            resolve_connection_target(&nodes, Some(2), true, &exit),
            Some(1)
        );
        assert_eq!(
            resolve_connection_target(&nodes, Some(2), false, &exit),
            Some(2)
        );
        assert_eq!(
            resolve_connection_target(&nodes, Some(2), true, "missing"),
            None
        );
    }

    #[test]
    fn legacy_settings_load_and_gain_subscription_defaults() {
        let settings: Settings = serde_json::from_str(
            r#"{"list_filter":"whitelist","auto_refresh":false,"kill_switch":false}"#,
        )
        .unwrap();
        assert!(!settings.auto_refresh);
        assert!(!settings.kill_switch);
        assert!(settings.subscriptions.is_empty());
        assert_eq!(settings.user_agent, DEFAULT_USER_AGENT);
        let saved = serde_json::to_string(&settings).unwrap();
        assert!(!saved.contains("list_filter"));
    }

    #[test]
    fn subscriptions_round_trip_through_settings() {
        let mut settings = Settings::default();
        let mut subscription = Subscription::remote("Мой VPN", "https://sub.example.com/abc");
        subscription.meta.total = 42;
        settings.subscriptions.push(subscription.clone());
        let json = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.subscriptions, vec![subscription]);
    }

    #[test]
    fn bundled_countries_have_map_points() {
        let report = parse_subscription(FALLBACK_CONFIGS);
        let missing: Vec<_> = report
            .nodes
            .iter()
            .filter(|node| country_coordinates(&node.country).is_none())
            .map(|node| node.country.clone())
            .collect();
        assert!(missing.is_empty(), "Нет координат для: {missing:?}");
    }

    #[test]
    fn natural_earth_map_is_loaded_and_triangulated() {
        let map = parse_world_map();
        assert!(map.polygons.len() > 200);
        assert!(map
            .polygons
            .iter()
            .all(|polygon| !polygon.triangles.is_empty()));
    }

    #[test]
    fn country_code_is_converted_to_the_matching_flag() {
        assert_eq!(theme::country_code_flag("nl").as_deref(), Some("🇳🇱"));
        assert_eq!(theme::country_code_flag("DE").as_deref(), Some("🇩🇪"));
        assert_eq!(theme::country_code_flag("bad"), None);
    }

    #[test]
    fn indonesia_is_not_confused_with_india() {
        assert_eq!(country_coordinates("Индонезия"), Some((118.0, -2.5)));
        assert_eq!(country_coordinates("Индия"), Some((78.9, 22.7)));
    }

    #[test]
    fn old_server_location_cache_remains_readable() {
        let location: ServerLocation = serde_json::from_str(
            r#"{"ip":"1.1.1.1","region":"Test","city":"Test","latitude":1.0,"longitude":2.0}"#,
        )
        .unwrap();
        assert!(location.country.is_empty());
        assert!(location.country_code.is_empty());
    }

    #[test]
    fn split_tunnel_entries_are_cleaned_and_deduplicated() {
        assert_eq!(
            settings_entries("chrome.exe\n chrome.exe ; C:\\Apps\\game.exe"),
            vec!["chrome.exe", "C:\\Apps\\game.exe"]
        );
        assert_eq!(
            normalize_domain_rule("https://www.youtube.com/watch?v=1").as_deref(),
            Some("www.youtube.com")
        );
        assert_eq!(
            normalize_domain_rule("*.github.com").as_deref(),
            Some("github.com")
        );
        assert_eq!(normalize_domain_rule("not a domain"), None);
    }

    #[test]
    fn protocol_filter_matches_scheme() {
        let node = parse_subscription("vless://1234@example.org:443?security=tls#Germany")
            .nodes
            .remove(0);
        assert!(node_matches_filters(&node, "all"));
        assert!(node_matches_filters(&node, "vless"));
        assert!(!node_matches_filters(&node, "trojan"));
    }

    #[test]
    fn nearest_server_distance_uses_geographic_coordinates() {
        let moscow = GeoPoint {
            longitude: 37.6173,
            latitude: 55.7558,
        };
        let saint_petersburg = GeoPoint {
            longitude: 30.3351,
            latitude: 59.9343,
        };
        let distance = haversine_km(moscow, saint_petersburg);
        assert!((620.0..650.0).contains(&distance));
        assert_eq!(haversine_km(moscow, moscow), 0.0);
    }
}
