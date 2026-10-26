//! Every screen of the app. Layout follows the reference: a navigation
//! sidebar, a breadcrumb header and content built from rounded cards.

use super::*;
use crate::map::{
    clamp_map_pan, ease_out_cubic, geo_to_pos, map_world_rect, min_map_zoom, paint_hop_badge,
    zoom_map_at, MapCamera,
};
use crate::theme::{
    alpha, button, caption, card_frame, flag_tile, icon_button, info_row, info_value, inset_frame,
    lucide_image, mix, mono, paint_flag, paint_icon, paint_radial_glow, progress_bar, regular,
    search_field, segmented, semi, text, text_heavy, text_semi, titled_card, toggle, ButtonKind,
    ACCENT, ACCENT_HOVER, AMBER, BG, BORDER, BORDER_STRONG, CARD, CARD_HOVER, ELEVATED, FAINT,
    FIELD, GREEN, MUTED, ON_ACCENT, RED, SIDEBAR, TEXT, TEXT_DIM,
};
use eframe::egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, Sense, Stroke,
    UiBuilder,
};
use lucide_static_svg::Icon;
use universe::subscription::{
    classify_import, format_ago, format_bytes, format_date, format_remaining, ImportInput,
};

const SIDEBAR_WIDTH: f32 = 216.0;
const SIDEBAR_COLLAPSED_WIDTH: f32 = 68.0;

pub fn server_count_label(count: usize) -> String {
    let ending = universe::subscription::plural(count as i64, "сервер", "сервера", "серверов");
    format!("{count} {ending}")
}

fn rate_label(bytes_per_second: u64) -> String {
    format!("{}/с", format_bytes(bytes_per_second))
}

fn read_clipboard() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

fn latency_style(value: Option<&Option<u128>>, pending: bool, udp: bool) -> (String, Color32) {
    if pending {
        return ("···".to_owned(), MUTED);
    }
    if udp {
        return ("UDP".to_owned(), FAINT);
    }
    match value {
        Some(Some(ms)) => (
            format!("{ms} мс"),
            if *ms < 150 {
                GREEN
            } else if *ms < 400 {
                AMBER
            } else {
                RED
            },
        ),
        Some(None) => ("нет ответа".to_owned(), RED),
        None => ("—".to_owned(), FAINT),
    }
}

fn paint_text_truncated(
    painter: &egui::Painter,
    pos: Pos2,
    align: Align2,
    value: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Rect {
    let mut job = egui::text::LayoutJob::simple_singleline(value.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(12.0));
    let galley = painter.layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

/// Floating glass-like surface used for overlays on top of the map.
fn paint_surface(painter: &egui::Painter, rect: Rect, radius: f32) {
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(110),
        }
        .as_shape(rect, radius),
    );
    painter.rect_filled(rect, radius, alpha(SIDEBAR, 242));
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0_f32, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
}

pub(crate) fn paint_painter_badge(
    painter: &egui::Painter,
    right: f32,
    center_y: f32,
    label: &str,
    color: Color32,
) -> f32 {
    let galley = painter.layout_no_wrap(label.to_owned(), semi(10.5), color);
    let size = galley.size() + vec2(14.0, 6.0);
    let rect = Rect::from_min_size(pos2(right - size.x, center_y - size.y / 2.0), size);
    painter.rect_filled(rect, size.y * 0.5, alpha(color, 24));
    painter.rect_stroke(
        rect,
        size.y * 0.5,
        Stroke::new(1.0_f32, alpha(color, 44)),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    rect.left()
}

pub fn handle_window_resize(ctx: &egui::Context, maximized: bool) {
    if maximized {
        return;
    }
    let Some(pointer) = ctx.input(|input| input.pointer.hover_pos()) else {
        return;
    };
    let screen = ctx.screen_rect();
    let margin = 5.0;
    let left = pointer.x <= screen.left() + margin;
    let right = pointer.x >= screen.right() - margin;
    let top = pointer.y <= screen.top() + margin;
    let bottom = pointer.y >= screen.bottom() - margin;
    let direction = match (left, right, top, bottom) {
        (true, _, true, _) => Some(egui::ResizeDirection::NorthWest),
        (_, true, true, _) => Some(egui::ResizeDirection::NorthEast),
        (true, _, _, true) => Some(egui::ResizeDirection::SouthWest),
        (_, true, _, true) => Some(egui::ResizeDirection::SouthEast),
        (true, _, _, _) => Some(egui::ResizeDirection::West),
        (_, true, _, _) => Some(egui::ResizeDirection::East),
        (_, _, true, _) => Some(egui::ResizeDirection::North),
        (_, _, _, true) => Some(egui::ResizeDirection::South),
        _ => None,
    };
    let Some(direction) = direction else {
        return;
    };
    ctx.set_cursor_icon(match direction {
        egui::ResizeDirection::NorthWest | egui::ResizeDirection::SouthEast => {
            egui::CursorIcon::ResizeNwSe
        }
        egui::ResizeDirection::NorthEast | egui::ResizeDirection::SouthWest => {
            egui::CursorIcon::ResizeNeSw
        }
        egui::ResizeDirection::East | egui::ResizeDirection::West => {
            egui::CursorIcon::ResizeHorizontal
        }
        egui::ResizeDirection::North | egui::ResizeDirection::South => {
            egui::CursorIcon::ResizeVertical
        }
    });
    if ctx.input(|input| input.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Idle,
    Connecting,
    Disconnecting,
    Reconnecting,
    Connected,
}

impl UniverseApp {
    fn connection_state(&self) -> ConnectionState {
        if self.connection_busy {
            if self.route_animation.phase == RoutePhase::Disconnecting {
                ConnectionState::Disconnecting
            } else {
                ConnectionState::Connecting
            }
        } else if self.process.is_some() {
            ConnectionState::Connected
        } else if self.reconnect_at.is_some() {
            ConnectionState::Reconnecting
        } else {
            ConnectionState::Idle
        }
    }

    pub(crate) fn draw(&mut self, ctx: &egui::Context) {
        if self.welcome && !self.settings.subscriptions.is_empty() && self.import.is_none() {
            // A subscription was added from the welcome screen.
            self.finish_welcome(Page::Home);
        }
        if self.welcome {
            self.draw_welcome(ctx);
            return;
        }
        self.paint_window(ctx);
        self.sidebar(ctx);
        let mut picker = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 22,
                right: 18,
                top: 0,
                bottom: 18,
            }))
            .show(ctx, |ui| {
                self.header(ui, ctx);
                match self.page {
                    Page::Home => picker = self.home_page(ui, ctx),
                    Page::Servers => self.servers_page(ui),
                    Page::Subscriptions => self.subscriptions_page(ui),
                    Page::Settings => self.settings_page(ui),
                    Page::Journal => self.journal_page(ui),
                }
            });
        if let Some((country, anchor, canvas)) = picker {
            self.server_picker_overlay(ctx, &country, anchor, canvas);
        }
        self.paint_window_border(ctx);
        self.handle_global_paste(ctx);
        self.import_dialog(ctx);
        self.toast(ctx);
    }

    fn window_radius(&self) -> f32 {
        if self.window_maximized {
            0.0
        } else {
            // Matches the radius Windows 11 uses for rounded windows.
            8.0
        }
    }

    pub(crate) fn paint_window(&self, ctx: &egui::Context) {
        let rect = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::background());
        painter.rect_filled(
            rect,
            self.window_radius(),
            if self.backdrop { alpha(BG, 176) } else { BG },
        );
        let glow = if self.process.is_some() {
            GREEN
        } else {
            ACCENT
        };
        paint_radial_glow(
            &painter,
            pos2(rect.right() - rect.width() * 0.22, rect.top() + 110.0),
            rect.width() * 0.3,
            alpha(glow, 22),
        );
    }

    fn sidebar_width(&self, ctx: &egui::Context) -> f32 {
        let collapsed = ctx.animate_bool_with_time(
            egui::Id::new("sidebar_collapsed"),
            self.settings.sidebar_collapsed,
            0.2,
        );
        egui::lerp(SIDEBAR_WIDTH..=SIDEBAR_COLLAPSED_WIDTH, collapsed)
    }

    pub(crate) fn paint_window_border(&self, ctx: &egui::Context) {
        let rect = ctx.screen_rect();
        let sidebar = self.sidebar_width(ctx);
        // Background layer, painted after the panels: above them, below dialogs.
        let painter = ctx.layer_painter(egui::LayerId::background());
        if !self.welcome {
            painter.line_segment(
                [
                    pos2(rect.left() + sidebar, rect.top()),
                    pos2(rect.left() + sidebar, rect.bottom()),
                ],
                Stroke::new(1.0_f32, BORDER),
            );
        }
        painter.rect_stroke(
            rect,
            self.window_radius(),
            Stroke::new(1.0_f32, BORDER_STRONG),
            egui::StrokeKind::Inside,
        );
    }

    // ── sidebar ──────────────────────────────────────────────────────────

    fn sidebar(&mut self, ctx: &egui::Context) {
        let radius = self.window_radius() as u8;
        let width = self.sidebar_width(ctx);
        let compact = width < 150.0;
        if width > SIDEBAR_COLLAPSED_WIDTH + 0.5 && width < SIDEBAR_WIDTH - 0.5 {
            ctx.request_repaint();
        }
        egui::SidePanel::left("navigation")
            .exact_width(width)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(alpha(SIDEBAR, if self.backdrop { 150 } else { 255 }))
                    .corner_radius(CornerRadius {
                        nw: radius,
                        sw: radius,
                        ne: 0,
                        se: 0,
                    })
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 14,
                        top: 18,
                        bottom: 14,
                    }),
            )
            .show(ctx, |ui| {
                let brand = ui
                    .horizontal(|ui| {
                        let (logo, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                        egui::Image::from_texture(&self.brand_texture)
                            .fit_to_exact_size(logo.size())
                            .corner_radius(10)
                            .paint_at(ui, logo);
                        if !compact {
                            ui.add_space(2.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.add_space(1.0);
                                ui.label(text_heavy(APP_NAME, 19.0, TEXT));
                                ui.label(text("vpn client", 11.5, MUTED));
                            });
                        }
                    })
                    .response;
                let drag = ui.interact(brand.rect, ui.id().with("brand_drag"), Sense::drag());
                if drag.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.add_space(24.0);
                nav_caption(ui, "подключение", compact);
                self.nav_item(ui, Page::Home, Icon::House, "главная", None, compact);
                let servers = (!self.nodes.is_empty()).then(|| self.nodes.len().to_string());
                self.nav_item(ui, Page::Servers, Icon::Server, "серверы", servers, compact);
                ui.add_space(10.0);
                nav_caption(ui, "профили", compact);
                let subscriptions = (!self.settings.subscriptions.is_empty())
                    .then(|| self.settings.subscriptions.len().to_string());
                self.nav_item(
                    ui,
                    Page::Subscriptions,
                    Icon::Layers,
                    "подписки",
                    subscriptions,
                    compact,
                );
                ui.add_space(10.0);
                nav_caption(ui, "система", compact);
                self.nav_item(
                    ui,
                    Page::Settings,
                    Icon::SlidersHorizontal,
                    "настройки",
                    None,
                    compact,
                );
                self.nav_item(ui, Page::Journal, Icon::ScrollText, "журнал", None, compact);

                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    self.sidebar_status(ui, compact);
                    if self.core_path.is_none() {
                        ui.add_space(8.0);
                        self.core_card(ui, compact);
                    }
                });
            });
    }

    fn nav_item(
        &mut self,
        ui: &mut egui::Ui,
        page: Page,
        icon: Icon,
        label: &str,
        counter: Option<String>,
        compact: bool,
    ) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
        let response = if compact {
            response.on_hover_text(label)
        } else {
            response
        };
        let ctx = ui.ctx().clone();
        let active = ctx.animate_bool(response.id.with("active"), self.page == page);
        let hover = ctx.animate_bool(response.id.with("hover"), response.hovered());
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            10.0,
            alpha(
                ELEVATED,
                (255.0 * active + 110.0 * hover * (1.0 - active)) as u8,
            ),
        );
        if active > 0.01 {
            painter.rect_stroke(
                rect,
                10.0,
                Stroke::new(1.0_f32, alpha(BORDER_STRONG, (255.0 * active) as u8)),
                egui::StrokeKind::Inside,
            );
        }
        let color = mix(mix(MUTED, TEXT_DIM, hover), TEXT, active);
        let icon_x = if compact {
            rect.center().x
        } else {
            rect.left() + 21.0
        };
        paint_icon(
            ui,
            icon,
            Rect::from_center_size(pos2(icon_x, rect.center().y), Vec2::splat(16.0)),
            mix(color, ACCENT, active),
        );
        if compact {
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.page = page;
            }
            return;
        }
        painter.text(
            pos2(rect.left() + 40.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            if self.page == page {
                semi(13.5)
            } else {
                regular(13.5)
            },
            color,
        );
        if let Some(counter) = counter {
            painter.text(
                pos2(rect.right() - 12.0, rect.center().y),
                Align2::RIGHT_CENTER,
                counter,
                regular(11.5),
                FAINT,
            );
        }
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            self.page = page;
        }
    }

    fn sidebar_status(&mut self, ui: &mut egui::Ui, compact: bool) {
        let state = self.connection_state();
        let (color, title, icon) = match state {
            ConnectionState::Connected => (GREEN, "защищено", Icon::ShieldCheck),
            ConnectionState::Connecting => (ACCENT, "подключение…", Icon::Shield),
            ConnectionState::Disconnecting => (MUTED, "отключение…", Icon::Shield),
            ConnectionState::Reconnecting => (AMBER, "переподключение…", Icon::Shield),
            ConnectionState::Idle => (MUTED, "не подключено", Icon::ShieldOff),
        };
        let detail = match state {
            ConnectionState::Connected => {
                let uptime = self
                    .connected_since
                    .map(|since| format_duration(since.elapsed()))
                    .unwrap_or_default();
                let name = self
                    .connected_node()
                    .map(|node| node.title().to_owned())
                    .unwrap_or_default();
                format!("{uptime} · {name}")
            }
            _ => self
                .connection_target_index()
                .and_then(|index| self.nodes.get(index))
                .map(|node| node.title().to_owned())
                .unwrap_or_else(|| "сервер не выбран".to_owned()),
        };
        if compact {
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
            let center = rect.center();
            ui.painter().circle_filled(center, 18.0, alpha(color, 30));
            ui.painter()
                .circle_stroke(center, 18.0, Stroke::new(1.0_f32, alpha(color, 70)));
            paint_icon(
                ui,
                icon,
                Rect::from_center_size(center, Vec2::splat(17.0)),
                color,
            );
            if response
                .on_hover_text(format!("{title}\n{detail}"))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.page = Page::Home;
            }
            if state == ConnectionState::Connected {
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
            return;
        }
        let response = card_frame()
            .inner_margin(egui::Margin::same(11))
            .corner_radius(13)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (avatar, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
                    ui.painter()
                        .circle_filled(avatar.center(), 18.0, alpha(color, 30));
                    ui.painter().circle_stroke(
                        avatar.center(),
                        18.0,
                        Stroke::new(1.0_f32, alpha(color, 70)),
                    );
                    paint_icon(
                        ui,
                        icon,
                        Rect::from_center_size(avatar.center(), Vec2::splat(17.0)),
                        color,
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(text_semi(title, 13.5, TEXT));
                        ui.add(egui::Label::new(text(detail, 11.5, MUTED)).truncate());
                    });
                });
            })
            .response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            self.page = Page::Home;
        }
        if state == ConnectionState::Connected {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
    }

    fn core_card(&mut self, ui: &mut egui::Ui, compact: bool) {
        if compact {
            let hint = if self.installing_core {
                "VPN-ядро скачивается…"
            } else {
                "VPN-ядро не установлено — нажмите, чтобы установить"
            };
            if button(
                ui,
                ButtonKind::Danger,
                Some(Icon::Download),
                "",
                vec2(ui.available_width(), 34.0),
                !self.installing_core,
            )
            .on_hover_text(hint)
            .clicked()
            {
                self.begin_core_install();
            }
            return;
        }
        egui::Frame::new()
            .fill(alpha(AMBER, 14))
            .stroke(Stroke::new(1.0_f32, alpha(AMBER, 50)))
            .corner_radius(12)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(text_semi("VPN-ядро не установлено", 12.5, TEXT));
                ui.label(text(
                    "sing-box скачивается с GitHub и проверяется по SHA-256",
                    11.0,
                    MUTED,
                ));
                ui.add_space(4.0);
                let label = match (self.installing_core, self.core_progress) {
                    (true, Some((downloaded, Some(total)))) if total > 0 => {
                        format!("скачиваю {}%", downloaded * 100 / total)
                    }
                    (true, _) => "скачиваю…".to_owned(),
                    (false, _) => "установить".to_owned(),
                };
                if button(
                    ui,
                    ButtonKind::Secondary,
                    Some(Icon::Download),
                    &label,
                    vec2(ui.available_width(), 30.0),
                    !self.installing_core,
                )
                .clicked()
                {
                    self.begin_core_install();
                }
            });
    }

    // ── header ───────────────────────────────────────────────────────────

    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::hover());
        let (group, page) = match self.page {
            Page::Home => ("подключение", "главная"),
            Page::Servers => ("подключение", "серверы"),
            Page::Subscriptions => ("профили", "подписки"),
            Page::Settings => ("система", "настройки"),
            Page::Journal => ("система", "журнал"),
        };
        let y = rect.center().y;
        let toggle_rect = Rect::from_min_size(pos2(rect.left() - 6.0, y - 15.0), Vec2::splat(30.0));
        let collapsed = self.settings.sidebar_collapsed;
        let toggle = ui
            .scope_builder(UiBuilder::new().max_rect(toggle_rect), |ui| {
                icon_button(
                    ui,
                    if collapsed {
                        Icon::PanelLeftOpen
                    } else {
                        Icon::PanelLeftClose
                    },
                    if collapsed {
                        "Развернуть боковую панель"
                    } else {
                        "Свернуть боковую панель"
                    },
                    30.0,
                )
            })
            .inner;
        if toggle.clicked() {
            self.settings.sidebar_collapsed = !collapsed;
            self.persist_settings();
        }
        let painter = ui.painter();
        let mut x = toggle_rect.right() + 8.0;
        for (part, font, color) in [
            (group, regular(13.5), MUTED),
            (" / ", regular(13.5), FAINT),
            (page, semi(13.5), TEXT),
        ] {
            let drawn = painter.text(pos2(x, y), Align2::LEFT_CENTER, part, font, color);
            x = drawn.right();
        }

        let controls = self.window_controls(ui, ctx, rect.right(), y);

        let drag_rect = Rect::from_min_max(
            pos2(toggle_rect.right() + 4.0, rect.top()),
            pos2(controls.left() - 8.0, rect.bottom()),
        );
        let drag = ui.interact(
            drag_rect,
            ui.id().with("window_drag_region"),
            Sense::click_and_drag(),
        );
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            self.window_maximized = !self.window_maximized;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.window_maximized));
        }
    }

    /// Minimize / maximize / hide-to-tray buttons, right-aligned at `right`.
    pub(crate) fn window_controls(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        right: f32,
        y: f32,
    ) -> Rect {
        let controls = Rect::from_min_max(pos2(right - 104.0, y - 16.0), pos2(right, y + 16.0));
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(controls)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icon_button(ui, Icon::X, "Свернуть в трей", 32.0).clicked() {
                    self.hide_to_tray(ctx);
                }
                let maximize_hint = if self.window_maximized {
                    "Восстановить"
                } else {
                    "Развернуть"
                };
                let maximize_icon = if self.window_maximized {
                    Icon::Copy
                } else {
                    Icon::Square
                };
                if icon_button(ui, maximize_icon, maximize_hint, 32.0).clicked() {
                    self.window_maximized = !self.window_maximized;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.window_maximized));
                }
                if icon_button(ui, Icon::Minus, "Свернуть", 32.0).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            },
        );
        controls
    }

    // ── home ─────────────────────────────────────────────────────────────

    fn home_page(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) -> Option<(String, Pos2, Rect)> {
        let area = ui.available_rect_before_wrap();
        let panel_width = 290.0_f32.min(area.width() * 0.4).max(250.0);
        let gap = 12.0;
        let map_rect = Rect::from_min_max(
            area.min,
            pos2(area.right() - panel_width - gap, area.bottom()),
        );
        let panel_rect = Rect::from_min_max(pos2(area.right() - panel_width, area.top()), area.max);
        let picker = self.map_card(ui, ctx, map_rect);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(panel_rect)
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("connection_panel")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.connection_panel(ui));
            },
        );
        ui.advance_cursor_after_rect(area);
        picker
    }

    fn connection_panel(&mut self, ui: &mut egui::Ui) {
        ui.set_width(ui.available_width() - 2.0);
        let target = self
            .connection_target_index()
            .and_then(|index| self.nodes.get(index))
            .cloned();
        let connected = self.connected_node().cloned();
        let state = self.connection_state();
        let same = target
            .as_ref()
            .is_some_and(|node| self.connected_raw.as_deref() == Some(node.raw.as_str()));
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

        // server card — the "signed in as" block of the reference
        card_frame()
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let row = vec2(ui.available_width(), 46.0);
                ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, Icon::Pencil, "Выбрать сервер", 30.0).clicked()
                    {
                        self.page = Page::Servers;
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        if let (Some(entry), Some(exit)) = (&multihop_entry, &target) {
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(66.0, 40.0), Sense::hover());
                            let first =
                                Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(42.0, 32.0));
                            paint_flag(ui, first, &entry.flag, None);
                            let second = first.translate(vec2(24.0, 0.0));
                            ui.painter().rect_filled(second.expand(2.0), 9.0, CARD);
                            paint_flag(ui, second, &exit.flag, None);
                        } else {
                            flag_tile(
                                ui,
                                target.as_ref().map(|node| node.flag.as_str()).unwrap_or(""),
                                vec2(46.0, 40.0),
                                same.then_some(alpha(GREEN, 120)),
                            );
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(text(
                                if self.settings.multihop_enabled {
                                    "цепочка multihop"
                                } else {
                                    "сервер"
                                },
                                11.5,
                                MUTED,
                            ));
                            let title = match (&multihop_entry, &target) {
                                (Some(entry), Some(exit)) => {
                                    format!("{} → {}", entry.title(), exit.title())
                                }
                                (_, Some(node)) => node.title().to_owned(),
                                _ => "не выбран".to_owned(),
                            };
                            ui.add(egui::Label::new(text_semi(title, 15.5, TEXT)).truncate());
                            let subtitle = target
                                .as_ref()
                                .map(|node| {
                                    format!(
                                        "{} · {} · {}",
                                        short_country(&node.country),
                                        node.protocol.label(),
                                        node.transport.to_lowercase()
                                    )
                                })
                                .unwrap_or_else(|| "выберите сервер на карте".to_owned());
                            ui.add(egui::Label::new(text(subtitle, 11.5, MUTED)).truncate());
                        });
                    });
                });
            });

        ui.add_space(12.0);
        caption(ui, "статус");
        card_frame()
            .inner_margin(egui::Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (state_text, state_color) = match state {
                    ConnectionState::Connected => (
                        format!(
                            "защищено · {}",
                            self.connected_since
                                .map(|since| format_duration(since.elapsed()))
                                .unwrap_or_default()
                        ),
                        GREEN,
                    ),
                    ConnectionState::Connecting => ("подключение…".to_owned(), ACCENT),
                    ConnectionState::Disconnecting => ("отключение…".to_owned(), MUTED),
                    ConnectionState::Reconnecting => ("переподключение…".to_owned(), AMBER),
                    ConnectionState::Idle => ("не подключено".to_owned(), MUTED),
                };
                info_row(ui, state_color, "соединение", |ui| {
                    info_value(ui, state_text, state_color)
                });
                let (ip, ip_color) = match (&connected, &self.public_ip) {
                    (None, _) => ("—".to_owned(), FAINT),
                    (Some(_), Some(ip)) => (ip.clone(), TEXT),
                    (Some(_), None) => ("определяю…".to_owned(), MUTED),
                };
                info_row(
                    ui,
                    if self.public_ip.is_some() {
                        ACCENT
                    } else {
                        FAINT
                    },
                    "внешний IP",
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(ip).font(mono(12.5)).color(ip_color),
                            )
                            .truncate(),
                        );
                    },
                );
                if let Some(node) = target.clone() {
                    let pending = self.pending_latency.contains(&node.raw);
                    let (latency, color) = latency_style(
                        self.latencies.get(&node.raw),
                        pending,
                        node.protocol.is_udp(),
                    );
                    let mut test = false;
                    info_row(ui, color, "задержка", |ui| {
                        test = ui
                            .add(
                                egui::Label::new(text_semi(latency, 13.0, color))
                                    .sense(Sense::click()),
                            )
                            .on_hover_text("Проверить задержку ещё раз")
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked();
                    });
                    if test && !pending {
                        self.latencies.remove(&node.raw);
                        self.begin_latency_test(vec![node]);
                    }
                }
                if state == ConnectionState::Connected {
                    let speed = format!(
                        "↓ {}   ↑ {}",
                        rate_label(self.traffic.down_rate),
                        rate_label(self.traffic.up_rate)
                    );
                    info_row(ui, ACCENT, "скорость", |ui| {
                        info_value(ui, speed, TEXT)
                    });
                    let total = format_bytes(self.traffic.down_total + self.traffic.up_total);
                    info_row(ui, ACCENT, "за сеанс", |ui| {
                        info_value(ui, total, TEXT)
                    });
                    if self.traffic.history.len() > 1 {
                        ui.add_space(4.0);
                        paint_sparkline(ui, &self.traffic.history);
                    }
                }
                let mut multihop = self.settings.multihop_enabled;
                info_row(ui, ACCENT, "режим", |ui| {
                    if segmented(
                        ui,
                        "route_mode",
                        &mut multihop,
                        &[(false, "обычный"), (true, "multihop")],
                        176.0,
                        26.0,
                    ) {
                        // applied below, outside of the closure
                    }
                });
                if multihop != self.settings.multihop_enabled {
                    self.set_multihop(multihop);
                }
            });

        if self.settings.multihop_enabled {
            ui.add_space(12.0);
            caption(ui, "цепочка multihop");
            card_frame()
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let exit = target.clone();
                    for (stage, label, node) in [
                        (MultiHopPick::Entry, "вход", multihop_entry.clone()),
                        (MultiHopPick::Exit, "выход", exit),
                    ] {
                        let active = self.multihop_pick == Some(stage);
                        let response = egui::Frame::new()
                            .fill(if active {
                                alpha(ACCENT, 20)
                            } else {
                                Color32::TRANSPARENT
                            })
                            .stroke(Stroke::new(
                                1.0_f32,
                                if active {
                                    alpha(ACCENT, 90)
                                } else {
                                    Color32::TRANSPARENT
                                },
                            ))
                            .corner_radius(10)
                            .inner_margin(egui::Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    paint_hop_number(
                                        ui,
                                        if stage == MultiHopPick::Entry {
                                            "1"
                                        } else {
                                            "2"
                                        },
                                    );
                                    ui.label(text(label, 12.5, MUTED));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.add(lucide_image(Icon::ChevronRight, 14.0, MUTED));
                                        let name = node
                                            .as_ref()
                                            .map(|node| node.title().to_owned())
                                            .unwrap_or_else(|| "выбрать".to_owned());
                                        ui.add(
                                            egui::Label::new(text_semi(name, 13.0, TEXT))
                                                .truncate(),
                                        );
                                    });
                                });
                            })
                            .response
                            .interact(Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if response.clicked() {
                            self.multihop_pick = if active { None } else { Some(stage) };
                            self.map.picker_country = None;
                            self.map.picker_region = None;
                        }
                    }
                    if self.multihop_pick.is_some() {
                        ui.add_space(2.0);
                        ui.label(text(
                            "нажмите на сервер на карте или в списке серверов",
                            11.5,
                            ACCENT,
                        ));
                    }
                });
        }

        ui.add_space(14.0);
        let (label, kind, enabled) = match state {
            ConnectionState::Connecting => ("подключение…", ButtonKind::Primary, false),
            ConnectionState::Disconnecting => ("отключение…", ButtonKind::Secondary, false),
            ConnectionState::Reconnecting => {
                ("отменить переподключение", ButtonKind::Secondary, true)
            }
            ConnectionState::Connected if same => ("отключить", ButtonKind::Secondary, true),
            ConnectionState::Connected => ("переключиться", ButtonKind::Primary, true),
            ConnectionState::Idle if target.is_some() => ("подключить", ButtonKind::Primary, true),
            ConnectionState::Idle => ("выберите сервер", ButtonKind::Primary, false),
        };
        if button(
            ui,
            kind,
            Some(Icon::Power),
            label,
            vec2(ui.available_width(), 48.0),
            enabled,
        )
        .clicked()
        {
            self.toggle_connection();
        }
        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            ui.add(
                egui::Label::new(text(
                    &self.notice,
                    12.0,
                    if self.notice_error { RED } else { MUTED },
                ))
                .wrap(),
            );
        });

        if let Some(subscription) = target
            .as_ref()
            .and_then(|node| self.subscription_of(node))
            .filter(|sub| sub.meta.has_traffic || sub.meta.expire > 0)
            .cloned()
        {
            ui.add_space(12.0);
            caption(ui, "подписка");
            card_frame()
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    subscription_summary(ui, &subscription);
                });
        }

        if let Some(process) = self
            .zapret_process
            .clone()
            .filter(|_| !self.zapret_warning_dismissed)
        {
            ui.add_space(12.0);
            let mut dismiss = false;
            egui::Frame::new()
                .fill(alpha(AMBER, 12))
                .stroke(Stroke::new(1.0_f32, alpha(AMBER, 48)))
                .corner_radius(12)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.add(lucide_image(Icon::TriangleAlert, 15.0, AMBER));
                        ui.vertical(|ui| {
                            ui.label(text_semi(format!("Запущен {process}"), 12.5, TEXT));
                            ui.label(text("zapret может мешать VPN-соединению", 11.0, MUTED));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            dismiss = icon_button(ui, Icon::X, "Скрыть", 24.0).clicked();
                        });
                    });
                });
            if dismiss {
                self.zapret_warning_dismissed = true;
                self.zapret_process = None;
            }
        }
    }

    fn map_card(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        rect: Rect,
    ) -> Option<(String, Pos2, Rect)> {
        self.tick_map_camera(ctx);
        let response = ui.interact(rect, ui.id().with("map_canvas"), Sense::click_and_drag());
        let painter = ui.painter().with_clip_rect(rect.shrink(1.0));
        // Land and glow follow the rounded corners of the card.
        let bands = map::rounded_clip_bands(rect.shrink(1.0), 15.0, 10);
        let connected = self.process.is_some();
        ui.painter().rect_filled(rect, 16.0, alpha(CARD, 214));
        for band in &bands {
            paint_radial_glow(
                &ui.painter().with_clip_rect(*band),
                rect.center() + vec2(rect.width() * 0.08, -rect.height() * 0.1),
                rect.width() * 0.42,
                alpha(if connected { GREEN } else { ACCENT }, 24),
            );
        }

        if !self.map.initialized {
            // Start centred on Europe, where most servers usually are.
            self.map.initialized = true;
            let world = map_world_rect(rect);
            let europe = geo_to_pos(world, 30.0, 0.0);
            self.map.pan = clamp_map_pan(vec2(rect.center().x - europe.x, 0.0), rect, 1.0);
        }
        if self.map.camera_animation.is_none() {
            // Keep the camera valid while the card is resized (sidebar, window).
            self.map.zoom = self.map.zoom.max(min_map_zoom(rect));
            self.map.pan = clamp_map_pan(self.map.pan, rect, self.map.zoom);
        }
        if response.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            self.map.camera_animation = None;
            self.map.pan += response.drag_delta();
            self.map.pan = clamp_map_pan(self.map.pan, rect, self.map.zoom);
        } else if response.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
            let scroll = ctx.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.01 {
                self.map.camera_animation = None;
                let pointer = response.hover_pos().unwrap_or(rect.center());
                zoom_map_at(
                    &mut self.map.zoom,
                    &mut self.map.pan,
                    rect,
                    pointer,
                    (scroll * 0.0025).exp(),
                );
            }
        }
        if let Some(country) = self.map.focus_country.take() {
            if let Some((longitude, latitude)) = country_coordinates(&country) {
                let world = map_world_rect(rect);
                let target_zoom = self.map.zoom.max(2.15);
                let base = geo_to_pos(world, longitude, latitude);
                let target_pan =
                    rect.center() - world.center() - (base - world.center()) * target_zoom;
                self.animate_map_camera(target_zoom, clamp_map_pan(target_pan, rect, target_zoom));
            }
        }

        let camera = MapCamera {
            world: map_world_rect(rect),
            zoom: self.map.zoom,
            pan: self.map.pan,
        };
        map::paint_world(ui.painter(), &self.world_map, &camera, &bands);
        map::paint_route_animation(&painter, &camera, &self.route_animation);
        let clicked = self.paint_markers(ui, ctx, &painter, &camera, &response);
        if let Some(index) = clicked {
            let country = self.nodes[index].country.clone();
            let region = (self.map.zoom >= 2.3)
                .then(|| self.server_locations.get(&self.nodes[index].host))
                .flatten()
                .map(|location| location.region.clone());
            let server_count = self
                .visible_indices()
                .into_iter()
                .filter(|index| {
                    let node = &self.nodes[*index];
                    node.country == country
                        && region.as_ref().is_none_or(|region| {
                            self.server_locations
                                .get(&node.host)
                                .is_some_and(|location| &location.region == region)
                        })
                })
                .count();
            if server_count == 1 {
                self.select_node(index);
                self.map.picker_country = None;
                self.map.picker_region = None;
            } else {
                self.map.picker_country = Some(country);
                self.map.picker_region = region;
                self.map.picker_opened_at = Some(Instant::now());
            }
        } else if response.clicked() {
            self.map.picker_country = None;
            self.map.picker_region = None;
        }

        ui.painter().rect_stroke(
            rect,
            16.0,
            Stroke::new(1.0_f32, BORDER),
            egui::StrokeKind::Inside,
        );

        // title chip
        let locations: HashSet<&str> = self
            .nodes
            .iter()
            .map(|node| node.country.as_str())
            .collect();
        let chip = Rect::from_min_size(rect.left_top() + vec2(14.0, 14.0), vec2(214.0, 46.0));
        paint_surface(&painter, chip, 13.0);
        paint_icon(
            ui,
            Icon::Earth,
            Rect::from_center_size(pos2(chip.left() + 22.0, chip.center().y), Vec2::splat(18.0)),
            ACCENT,
        );
        painter.text(
            pos2(chip.left() + 40.0, chip.top() + 15.0),
            Align2::LEFT_CENTER,
            "карта серверов",
            semi(13.0),
            TEXT,
        );
        painter.text(
            pos2(chip.left() + 40.0, chip.top() + 32.0),
            Align2::LEFT_CENTER,
            format!(
                "{} · {} {}",
                server_count_label(self.nodes.len()),
                locations.len(),
                universe::subscription::plural(locations.len() as i64, "страна", "страны", "стран")
            ),
            regular(11.0),
            MUTED,
        );

        // zoom controls
        let controls = Rect::from_min_size(
            pos2(rect.right() - 14.0 - 40.0, rect.top() + 14.0),
            vec2(40.0, 118.0),
        );
        paint_surface(&painter, controls, 13.0);
        let mut zoom_request = None;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(controls.shrink(4.0))
                .layout(Layout::top_down(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if icon_button(ui, Icon::ZoomIn, "Приблизить", 32.0).clicked() {
                    zoom_request = Some(1.3);
                }
                if icon_button(ui, Icon::ZoomOut, "Отдалить", 32.0).clicked() {
                    zoom_request = Some(0.77);
                }
                if icon_button(ui, Icon::LocateFixed, "Весь мир", 32.0).clicked() {
                    zoom_request = Some(0.0);
                }
            },
        );
        match zoom_request {
            Some(0.0) => self.animate_map_camera(min_map_zoom(rect), Vec2::ZERO),
            Some(factor) => {
                let (mut zoom, mut pan) = self
                    .map
                    .camera_animation
                    .as_ref()
                    .map(|animation| (animation.to_zoom, animation.to_pan))
                    .unwrap_or((self.map.zoom, self.map.pan));
                zoom_map_at(&mut zoom, &mut pan, rect, rect.center(), factor);
                self.animate_map_camera(zoom, pan);
            }
            None => {}
        }

        // connection transition chip
        if matches!(
            self.route_animation.phase,
            RoutePhase::Connecting | RoutePhase::Failed | RoutePhase::Disconnecting
        ) {
            let (message, color) = match self.route_animation.phase {
                RoutePhase::Connecting if self.settings.multihop_enabled => {
                    ("соединяю два узла", ACCENT)
                }
                RoutePhase::Connecting => ("прокладываю защищённый маршрут", ACCENT),
                RoutePhase::Failed => ("маршрут прерван", RED),
                _ => ("завершаю соединение", MUTED),
            };
            let galley = painter.layout_no_wrap(message.to_owned(), regular(12.5), TEXT);
            let chip = Rect::from_center_size(
                pos2(rect.center().x, rect.top() + 37.0),
                vec2(galley.size().x + 48.0, 34.0),
            );
            paint_surface(&painter, chip, 17.0);
            let pulse = 3.0
                + (self.route_animation.started.elapsed().as_secs_f32() * 4.0)
                    .sin()
                    .abs()
                    * 2.0;
            painter.circle_filled(pos2(chip.left() + 18.0, chip.center().y), pulse, color);
            painter.galley(
                pos2(chip.left() + 32.0, chip.center().y - galley.size().y / 2.0),
                galley,
                TEXT,
            );
        }

        // multihop picking banner
        if let Some(stage) = self.multihop_pick {
            let banner = Rect::from_min_size(rect.left_top() + vec2(14.0, 70.0), vec2(270.0, 52.0));
            paint_surface(&painter, banner, 13.0);
            let pulse = 8.0 + (ctx.input(|input| input.time) as f32 * 3.4).sin().abs() * 3.0;
            let marker = pos2(banner.left() + 24.0, banner.center().y);
            painter.circle_filled(marker, pulse, alpha(ACCENT, 26));
            painter.circle_filled(marker, 5.0, ACCENT);
            painter.text(
                pos2(banner.left() + 44.0, banner.top() + 16.0),
                Align2::LEFT_CENTER,
                match stage {
                    MultiHopPick::Entry => "multihop · шаг 1 из 2",
                    MultiHopPick::Exit => "multihop · шаг 2 из 2",
                },
                regular(11.0),
                ACCENT,
            );
            painter.text(
                pos2(banner.left() + 44.0, banner.top() + 35.0),
                Align2::LEFT_CENTER,
                match stage {
                    MultiHopPick::Entry => "выберите входной сервер",
                    MultiHopPick::Exit => "выберите выходной сервер",
                },
                semi(13.0),
                TEXT,
            );
            let cancel = Rect::from_center_size(
                pos2(banner.right() - 22.0, banner.center().y),
                Vec2::splat(28.0),
            );
            if ui
                .put(
                    cancel,
                    egui::Button::image(lucide_image(Icon::X, 14.0, MUTED))
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE),
                )
                .on_hover_text("Отменить выбор")
                .clicked()
            {
                self.multihop_pick = None;
            }
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        // legend & warnings
        let legend_y = rect.bottom() - 22.0;
        let mut x = rect.left() + 20.0;
        for (color, label) in [(ACCENT, "доступно"), (GREEN, "подключено")] {
            painter.circle_filled(pos2(x, legend_y), 4.0, color);
            let drawn = painter.text(
                pos2(x + 10.0, legend_y),
                Align2::LEFT_CENTER,
                label,
                regular(11.5),
                MUTED,
            );
            x = drawn.right() + 18.0;
        }
        let now = now_unix();
        if let Some(warning) = self
            .settings
            .subscriptions
            .iter()
            .filter(|sub| sub.enabled)
            .find_map(|sub| {
                sub.warning(now)
                    .map(|text| format!("{} · {text}", sub.display_name()))
            })
        {
            let galley = painter.layout_no_wrap(warning, regular(12.0), TEXT);
            let chip = Rect::from_center_size(
                pos2(rect.center().x, rect.bottom() - 62.0),
                vec2(galley.size().x + 46.0, 32.0),
            );
            painter.rect_filled(chip, 16.0, Color32::from_rgb(38, 33, 20));
            painter.rect_stroke(
                chip,
                16.0,
                Stroke::new(1.0_f32, alpha(AMBER, 80)),
                egui::StrokeKind::Inside,
            );
            paint_icon(
                ui,
                Icon::TriangleAlert,
                Rect::from_center_size(
                    pos2(chip.left() + 18.0, chip.center().y),
                    Vec2::splat(14.0),
                ),
                AMBER,
            );
            painter.galley(
                pos2(chip.left() + 32.0, chip.center().y - galley.size().y / 2.0),
                galley,
                TEXT,
            );
        }

        if self.nodes.is_empty() {
            let card = Rect::from_center_size(rect.center(), vec2(330.0, 150.0));
            paint_surface(&painter, card, 16.0);
            painter.text(
                pos2(card.center().x, card.top() + 38.0),
                Align2::CENTER_CENTER,
                "серверов пока нет",
                semi(16.0),
                TEXT,
            );
            painter.text(
                pos2(card.center().x, card.top() + 62.0),
                Align2::CENTER_CENTER,
                "добавьте подписку или ключи",
                regular(12.5),
                MUTED,
            );
            let button_rect = Rect::from_center_size(
                pos2(card.center().x, card.bottom() - 36.0),
                vec2(210.0, 38.0),
            );
            ui.scope_builder(UiBuilder::new().max_rect(button_rect), |ui| {
                if button(
                    ui,
                    ButtonKind::Primary,
                    Some(Icon::Plus),
                    "добавить подписку",
                    button_rect.size(),
                    true,
                )
                .clicked()
                {
                    self.open_import(None, String::new());
                }
            });
        }

        let picker = self.map.picker_country.clone().and_then(|country| {
            let regional = self.map.picker_region.as_ref().and_then(|region| {
                self.nodes
                    .iter()
                    .filter(|node| node.country == country)
                    .filter_map(|node| self.server_locations.get(&node.host))
                    .find(|location| &location.region == region)
                    .map(|location| (location.longitude, location.latitude))
            });
            let fallback = self
                .nodes
                .iter()
                .find(|node| node.country == country)
                .and_then(|node| self.node_point(node))
                .map(|point| (point.longitude, point.latitude));
            regional
                .or_else(|| country_coordinates(&country))
                .or(fallback)
                .map(|(longitude, latitude)| (country, camera.project(longitude, latitude), rect))
        });
        picker
    }

    fn animate_map_camera(&mut self, target_zoom: f32, target_pan: Vec2) {
        let target_zoom = target_zoom.clamp(0.2, 10.0);
        if (self.map.zoom - target_zoom).abs() < 0.001
            && (self.map.pan - target_pan).length_sq() < 0.01
        {
            self.map.zoom = target_zoom;
            self.map.pan = target_pan;
            self.map.camera_animation = None;
            return;
        }
        self.map.camera_animation = Some(MapCameraAnimation::new(
            self.map.zoom,
            self.map.pan,
            target_zoom,
            target_pan,
        ));
        self.ctx.request_repaint();
    }

    fn tick_map_camera(&mut self, ctx: &egui::Context) {
        let Some(animation) = self.map.camera_animation.as_ref() else {
            return;
        };
        let (zoom, pan, complete) = animation.sample();
        self.map.zoom = zoom;
        self.map.pan = pan;
        if complete {
            self.map.camera_animation = None;
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn paint_markers(
        &self,
        ui: &egui::Ui,
        ctx: &egui::Context,
        painter: &egui::Painter,
        camera: &MapCamera,
        response: &egui::Response,
    ) -> Option<usize> {
        struct Group {
            label: String,
            indices: Vec<usize>,
            anchor: (f32, f32),
            sum: (f32, f32),
            regional: bool,
        }
        let regional_view = self.map.zoom >= 2.3;
        let spread = ((self.map.zoom - 1.8) / 0.9).clamp(0.0, 1.0);
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for index in self.visible_indices() {
            let node = &self.nodes[index];
            let location = self.server_locations.get(&node.host);
            let Some(anchor) = country_coordinates(&node.country)
                .or_else(|| location.map(|location| (location.longitude, location.latitude)))
            else {
                continue;
            };
            let regional = regional_view.then_some(location).flatten();
            let target = regional
                .map(|location| (location.longitude, location.latitude))
                .unwrap_or(anchor);
            let key = match regional {
                Some(location) => format!("{}\0{}", node.country, location.region.to_lowercase()),
                None => node.country.clone(),
            };
            let group = groups.entry(key).or_insert_with(|| Group {
                label: regional
                    .map(|location| location.region.clone())
                    .unwrap_or_else(|| node.country.clone()),
                indices: Vec::new(),
                anchor,
                sum: (0.0, 0.0),
                regional: regional.is_some(),
            });
            group.indices.push(index);
            group.sum.0 += target.0;
            group.sum.1 += target.1;
        }

        let pointer = response.hover_pos();
        let connected_index = self
            .connected_raw
            .as_deref()
            .and_then(|raw| self.nodes.iter().position(|node| node.raw == raw));
        let mut hovered: Option<(String, usize, usize, Pos2, f32)> = None;
        for group in groups.values() {
            let count = group.indices.len();
            let average = (group.sum.0 / count as f32, group.sum.1 / count as f32);
            let (longitude, latitude) = if group.regional {
                (
                    group.anchor.0 + (average.0 - group.anchor.0) * spread,
                    group.anchor.1 + (average.1 - group.anchor.1) * spread,
                )
            } else {
                group.anchor
            };
            let mut point = camera.project(longitude, latitude);
            let selected = self
                .selected
                .is_some_and(|selected| group.indices.contains(&selected));
            let connected = connected_index.is_some_and(|index| group.indices.contains(&index));
            let hop_entry = self.settings.multihop_enabled
                && group
                    .indices
                    .iter()
                    .any(|index| self.nodes[*index].raw == self.settings.multihop_entry);
            let hop_exit = self.settings.multihop_enabled
                && group
                    .indices
                    .iter()
                    .any(|index| self.nodes[*index].raw == self.settings.multihop_exit);
            if selected {
                let t = (self.selection_changed_at.elapsed().as_secs_f32() / 0.66).min(1.0);
                if t < 1.0 {
                    point.y -= 17.0 * (1.0 - t) * (std::f32::consts::PI * t).sin();
                    ctx.request_repaint_after(Duration::from_millis(16));
                }
            }
            let radius = if count >= 100 {
                10.0
            } else if count >= 10 {
                9.0
            } else if count > 1 {
                8.0
            } else {
                5.5
            };
            let is_hovered = pointer.is_some_and(|cursor| cursor.distance(point) <= radius + 7.0);
            let color = if connected { GREEN } else { ACCENT };
            if connected {
                let phase = self
                    .connected_since
                    .map(|started| started.elapsed().as_secs_f32())
                    .unwrap_or_default();
                let pulse = 8.0 + (phase * 2.4).sin().abs() * 5.0;
                painter.circle_filled(point, radius + pulse, alpha(GREEN, 18));
                painter.circle_stroke(
                    point,
                    radius + pulse,
                    Stroke::new(1.0_f32, alpha(GREEN, 72)),
                );
            }
            painter.circle_filled(
                point,
                radius + if selected || is_hovered { 6.0 } else { 3.5 },
                alpha(color, if selected || is_hovered { 46 } else { 26 }),
            );
            painter.circle_filled(
                point,
                radius,
                if selected || is_hovered {
                    mix(color, Color32::WHITE, 0.25)
                } else {
                    color
                },
            );
            if count > 1 {
                painter.text(
                    point,
                    Align2::CENTER_CENTER,
                    compact_count(count),
                    semi(9.0),
                    ON_ACCENT,
                );
            } else {
                painter.circle_filled(point, 2.0, ON_ACCENT);
            }
            if selected {
                painter.circle_stroke(
                    point,
                    radius + 6.5,
                    Stroke::new(1.2_f32, alpha(ACCENT_HOVER, 170)),
                );
            }
            if hop_entry {
                painter.circle_stroke(point, radius + 9.0, Stroke::new(1.5_f32, ACCENT_HOVER));
                paint_hop_badge(
                    painter,
                    point + vec2(-radius - 7.0, -radius - 8.0),
                    "1",
                    ACCENT,
                );
            }
            if hop_exit {
                painter.circle_stroke(point, radius + 13.0, Stroke::new(1.5_f32, GREEN));
                paint_hop_badge(
                    painter,
                    point + vec2(radius + 7.0, -radius - 8.0),
                    "2",
                    GREEN,
                );
            }
            if is_hovered {
                let distance = pointer
                    .map(|cursor| cursor.distance(point))
                    .unwrap_or(f32::MAX);
                if hovered
                    .as_ref()
                    .is_none_or(|(_, _, _, _, best)| distance < *best)
                {
                    hovered = Some((
                        group.label.clone(),
                        group.indices[0],
                        count,
                        point,
                        distance,
                    ));
                }
            }
        }

        if self.map.picker_country.is_none() {
            if let Some((label, index, count, point, _)) = hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                let galley = painter.layout_no_wrap(
                    format!("{label}  ·  {}", server_count_label(count)),
                    regular(12.0),
                    TEXT,
                );
                let tooltip = Rect::from_center_size(
                    point + vec2(0.0, -34.0),
                    galley.size() + vec2(24.0, 14.0),
                );
                paint_surface(painter, tooltip, 10.0);
                painter.galley(tooltip.center() - galley.size() / 2.0, galley, TEXT);
                if response.clicked() {
                    return Some(index);
                }
            }
        }
        None
    }

    fn server_picker_overlay(
        &mut self,
        ctx: &egui::Context,
        country: &str,
        anchor: Pos2,
        canvas: Rect,
    ) {
        let indices: Vec<usize> = self
            .visible_indices()
            .into_iter()
            .filter(|index| {
                let node = &self.nodes[*index];
                node.country == country
                    && self.map.picker_region.as_ref().is_none_or(|region| {
                        self.server_locations
                            .get(&node.host)
                            .is_some_and(|location| &location.region == region)
                    })
            })
            .collect();
        if indices.is_empty() {
            self.map.picker_country = None;
            self.map.picker_region = None;
            return;
        }
        let popup_width = 360.0_f32.min((canvas.width() - 32.0).max(280.0));
        let estimated_height = (84.0 + indices.len() as f32 * 50.0).min(360.0);
        let x = if anchor.x < canvas.center().x {
            anchor.x + 18.0
        } else {
            anchor.x - popup_width - 18.0
        }
        .clamp(
            canvas.left() + 12.0,
            (canvas.right() - popup_width - 12.0).max(canvas.left() + 12.0),
        );
        let y = (anchor.y - 24.0).clamp(
            canvas.top() + 12.0,
            (canvas.bottom() - estimated_height - 12.0).max(canvas.top() + 12.0),
        );
        let reveal = ease_out_cubic(
            self.map
                .picker_opened_at
                .map(|opened| opened.elapsed().as_secs_f32() / 0.2)
                .unwrap_or(1.0),
        );
        if reveal < 1.0 {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let nodes: Vec<(usize, VpnNode)> = indices
            .iter()
            .filter_map(|index| self.nodes.get(*index).cloned().map(|node| (*index, node)))
            .collect();
        let flag = nodes
            .first()
            .map(|(_, node)| node.flag.clone())
            .unwrap_or_default();
        let title = self
            .map
            .picker_region
            .clone()
            .unwrap_or_else(|| country.to_owned());
        let mut choice = None;
        let mut close = false;
        egui::Area::new(egui::Id::new("map_server_picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(x, y + (1.0 - reveal) * 10.0))
            .show(ctx, |ui| {
                ui.set_opacity(0.3 + reveal * 0.7);
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0_f32, BORDER_STRONG))
                    .corner_radius(16)
                    .inner_margin(egui::Margin::same(12))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 14],
                        blur: 34,
                        spread: 0,
                        color: Color32::from_black_alpha(130),
                    })
                    .show(ui, |ui| {
                        ui.set_width(popup_width - 24.0);
                        ui.horizontal(|ui| {
                            flag_tile(ui, &flag, vec2(40.0, 32.0), None);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(text_semi(&title, 15.0, TEXT));
                                ui.label(text(server_count_label(nodes.len()), 11.5, MUTED));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                close = icon_button(ui, Icon::X, "Закрыть", 28.0).clicked();
                            });
                        });
                        ui.add_space(8.0);
                        egui::ScrollArea::vertical()
                            .max_height(270.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (index, node) in &nodes {
                                    let selected = self.selected == Some(*index);
                                    let (latency, color) = latency_style(
                                        self.latencies.get(&node.raw),
                                        self.pending_latency.contains(&node.raw),
                                        node.protocol.is_udp(),
                                    );
                                    let (rect, response) = ui.allocate_exact_size(
                                        vec2(ui.available_width(), 46.0),
                                        Sense::click(),
                                    );
                                    let hover = ctx.animate_bool(
                                        response.id.with("hover"),
                                        response.hovered(),
                                    );
                                    let painter = ui.painter();
                                    painter.rect_filled(
                                        rect,
                                        10.0,
                                        if selected {
                                            ELEVATED
                                        } else {
                                            alpha(CARD_HOVER, (255.0 * hover) as u8)
                                        },
                                    );
                                    if selected {
                                        painter.rect_stroke(
                                            rect,
                                            10.0,
                                            Stroke::new(1.0_f32, alpha(ACCENT, 90)),
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                    paint_icon(
                                        ui,
                                        if selected {
                                            Icon::CircleCheck
                                        } else {
                                            Icon::Server
                                        },
                                        Rect::from_center_size(
                                            pos2(rect.left() + 18.0, rect.center().y),
                                            Vec2::splat(15.0),
                                        ),
                                        if selected { ACCENT } else { MUTED },
                                    );
                                    let right = paint_painter_badge(
                                        painter,
                                        rect.right() - 10.0,
                                        rect.center().y,
                                        node.protocol.short_label(),
                                        ACCENT,
                                    );
                                    painter.text(
                                        pos2(right - 10.0, rect.center().y),
                                        Align2::RIGHT_CENTER,
                                        &latency,
                                        semi(11.5),
                                        color,
                                    );
                                    let max_width = right - 70.0 - rect.left() - 36.0;
                                    paint_text_truncated(
                                        painter,
                                        pos2(rect.left() + 36.0, rect.center().y - 8.0),
                                        Align2::LEFT_CENTER,
                                        node.title(),
                                        semi(13.0),
                                        TEXT,
                                        max_width,
                                    );
                                    paint_text_truncated(
                                        painter,
                                        pos2(rect.left() + 36.0, rect.center().y + 9.0),
                                        Align2::LEFT_CENTER,
                                        &node.endpoint(),
                                        mono(10.5),
                                        MUTED,
                                        max_width,
                                    );
                                    if response
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .clicked()
                                    {
                                        choice = Some(*index);
                                    }
                                }
                            });
                    });
            });
        if close {
            self.map.picker_country = None;
            self.map.picker_region = None;
        }
        if let Some(index) = choice {
            self.select_node(index);
            self.map.picker_country = None;
            self.map.picker_region = None;
        }
    }

    // ── servers ──────────────────────────────────────────────────────────

    fn page_title(ui: &mut egui::Ui, title: &str, subtitle: &str, reserve: f32) {
        let needed = ui.fonts(|fonts| {
            fonts
                .layout_no_wrap(title.to_owned(), theme::heavy(26.0), TEXT)
                .size()
                .x
                + fonts
                    .layout_no_wrap(subtitle.to_owned(), regular(14.0), TEXT)
                    .size()
                    .x
        }) + 24.0;
        let show_subtitle = ui.available_width() - reserve >= needed;
        ui.horizontal(|ui| {
            ui.label(text_heavy(title, 26.0, TEXT));
            if show_subtitle {
                ui.add_space(2.0);
                ui.label(text(subtitle, 14.0, MUTED));
            }
        });
    }

    fn servers_page(&mut self, ui: &mut egui::Ui) {
        if !self.auto_pinged && !self.nodes.is_empty() {
            // Measure every server once per session so the list is sortable by ping.
            self.auto_pinged = true;
            let untested = self
                .nodes
                .iter()
                .filter(|node| !self.latencies.contains_key(&node.raw))
                .cloned()
                .collect();
            self.begin_latency_test(untested);
        }
        let mut indices = self.visible_indices();
        ui.horizontal(|ui| {
            Self::page_title(ui, "Серверы", &server_count_label(self.nodes.len()), 290.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = "проверить пинг";
                if button(
                    ui,
                    ButtonKind::Secondary,
                    Some(Icon::Gauge),
                    label,
                    vec2(theme::button_width(ui, label, true, 34.0), 34.0),
                    !indices.is_empty(),
                )
                .on_hover_text("TCP-задержка до каждого сервера в списке")
                .clicked()
                {
                    for index in &indices {
                        self.latencies.remove(&self.nodes[*index].raw);
                    }
                    let nodes = indices
                        .iter()
                        .map(|index| self.nodes[*index].clone())
                        .collect();
                    self.begin_latency_test(nodes);
                }
                let refreshing = !self.refreshing.is_empty();
                let label = if refreshing {
                    "обновляю…"
                } else {
                    "обновить"
                };
                if button(
                    ui,
                    ButtonKind::Secondary,
                    Some(Icon::RefreshCw),
                    label,
                    vec2(theme::button_width(ui, label, true, 34.0), 34.0),
                    !refreshing,
                )
                .clicked()
                {
                    self.refresh_all(true);
                }
            });
        });
        ui.add_space(14.0);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let width = (ui.available_width() - 530.0).clamp(200.0, 340.0);
            search_field(ui, &mut self.search, "поиск серверов", Icon::Search, width);
            ui.add_space(6.0);
            let mut protocol = self.settings.protocol_filter.clone();
            if segmented(
                ui,
                "protocol_filter",
                &mut protocol,
                &[
                    ("all".to_owned(), "все"),
                    ("vless".to_owned(), "VLESS"),
                    ("vmess".to_owned(), "VMess"),
                    ("trojan".to_owned(), "Trojan"),
                    ("ss".to_owned(), "SS"),
                    ("hysteria2".to_owned(), "HY2"),
                ],
                330.0,
                34.0,
            ) {
                self.settings.protocol_filter = protocol;
                self.persist_settings();
            }
            ui.add_space(6.0);
            let mut sort = self.settings.server_sort.clone();
            if segmented(
                ui,
                "server_sort",
                &mut sort,
                &[
                    ("provider".to_owned(), "порядок"),
                    ("ping".to_owned(), "пинг"),
                    ("name".to_owned(), "имя"),
                ],
                180.0,
                34.0,
            ) {
                self.settings.server_sort = sort;
                self.persist_settings();
            }
        });

        if let Some(stage) = self.multihop_pick {
            ui.add_space(10.0);
            egui::Frame::new()
                .fill(alpha(ACCENT, 16))
                .stroke(Stroke::new(1.0_f32, alpha(ACCENT, 70)))
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.add(lucide_image(Icon::Route, 16.0, ACCENT));
                        ui.label(text_semi(
                            match stage {
                                MultiHopPick::Entry => "Выберите входной сервер MultiHop",
                                MultiHopPick::Exit => "Выберите выходной сервер MultiHop",
                            },
                            13.0,
                            TEXT,
                        ));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, Icon::X, "Отменить", 26.0).clicked() {
                                self.multihop_pick = None;
                            }
                        });
                    });
                });
        }

        ui.add_space(12.0);
        if self.settings.subscriptions.is_empty() {
            self.empty_subscriptions(ui);
            return;
        }

        match self.settings.server_sort.as_str() {
            "ping" => indices.sort_by_key(|index| {
                self.latencies
                    .get(&self.nodes[*index].raw)
                    .copied()
                    .flatten()
                    .unwrap_or(u128::MAX)
            }),
            "name" => indices.sort_by_key(|index| self.nodes[*index].title().to_lowercase()),
            _ => {}
        }
        let groups: Vec<(String, String, Vec<usize>)> = self
            .settings
            .subscriptions
            .iter()
            .filter(|sub| sub.enabled)
            .map(|sub| {
                (
                    sub.id.clone(),
                    sub.display_name(),
                    indices
                        .iter()
                        .copied()
                        .filter(|index| self.nodes[*index].source == sub.id)
                        .collect::<Vec<_>>(),
                )
            })
            .filter(|(_, _, items)| !items.is_empty())
            .collect();

        let mut choose = None;
        let mut connect = None;
        let mut toggle_group = None;
        egui::ScrollArea::vertical()
            .id_salt("server_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if groups.is_empty() {
                    ui.add_space(50.0);
                    ui.vertical_centered(|ui| {
                        ui.add(lucide_image(Icon::Search, 28.0, FAINT));
                        ui.add_space(6.0);
                        ui.label(text_semi("Ничего не найдено", 15.0, TEXT));
                        ui.label(text(
                            if self.nodes.is_empty() {
                                "подписки ещё загружаются или пусты"
                            } else {
                                "измените поиск или фильтр протокола"
                            },
                            12.5,
                            MUTED,
                        ));
                    });
                }
                let connected_raw = self.connected_raw.clone();
                for (id, name, items) in &groups {
                    let collapsed = self.collapsed_groups.contains(id);
                    let header = ui
                        .horizontal(|ui| {
                            ui.add(lucide_image(
                                if collapsed {
                                    Icon::ChevronRight
                                } else {
                                    Icon::ChevronDown
                                },
                                14.0,
                                MUTED,
                            ));
                            ui.label(text_semi(name, 13.5, TEXT_DIM));
                            ui.label(text(server_count_label(items.len()), 12.0, FAINT));
                        })
                        .response
                        .interact(Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if header.clicked() {
                        toggle_group = Some(id.clone());
                    }
                    ui.add_space(4.0);
                    if collapsed {
                        ui.add_space(8.0);
                        continue;
                    }
                    for index in items {
                        let node = &self.nodes[*index];
                        let selected = self.selected == Some(*index);
                        let connected = connected_raw.as_deref() == Some(node.raw.as_str());
                        let response = self.server_row(ui, node, selected, connected);
                        if response.double_clicked() {
                            connect = Some(*index);
                        } else if response.clicked() {
                            choose = Some(*index);
                        }
                    }
                    ui.add_space(12.0);
                }
            });
        if let Some(id) = toggle_group {
            if !self.collapsed_groups.remove(&id) {
                self.collapsed_groups.insert(id);
            }
        }
        if let Some(index) = choose {
            self.select_node(index);
        }
        if let Some(index) = connect {
            self.select_node(index);
            if self.connected_raw.as_deref() != Some(self.nodes[index].raw.as_str()) {
                self.connect(true);
            }
            self.page = Page::Home;
        }
    }

    fn server_row(
        &self,
        ui: &mut egui::Ui,
        node: &VpnNode,
        selected: bool,
        connected: bool,
    ) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click());
        let hover = ui
            .ctx()
            .animate_bool(response.id.with("hover"), response.hovered());
        let painter = ui.painter();
        let fill = if selected {
            ELEVATED
        } else {
            alpha(CARD_HOVER, (255.0 * hover) as u8)
        };
        painter.rect_filled(rect, 12.0, fill);
        if selected {
            painter.rect_stroke(
                rect,
                12.0,
                Stroke::new(1.0_f32, BORDER_STRONG),
                egui::StrokeKind::Inside,
            );
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(rect.left(), rect.top() + 14.0),
                    pos2(rect.left() + 3.0, rect.bottom() - 14.0),
                ),
                2.0,
                ACCENT,
            );
        }
        let flag =
            Rect::from_center_size(pos2(rect.left() + 32.0, rect.center().y), vec2(40.0, 32.0));
        paint_flag(ui, flag, &node.flag, connected.then_some(alpha(GREEN, 140)));

        let (latency, latency_color) = latency_style(
            self.latencies.get(&node.raw),
            self.pending_latency.contains(&node.raw),
            node.protocol.is_udp(),
        );
        let mut right = rect.right() - 16.0;
        let latency_rect = painter.text(
            pos2(right, rect.center().y),
            Align2::RIGHT_CENTER,
            &latency,
            semi(12.5),
            latency_color,
        );
        right = latency_rect.left().min(right - 60.0) - 14.0;
        right = paint_painter_badge(
            painter,
            right,
            rect.center().y,
            node.protocol.short_label(),
            ACCENT,
        ) - 8.0;
        let transport = node.transport.to_lowercase();
        let transport_rect = painter.text(
            pos2(right, rect.center().y),
            Align2::RIGHT_CENTER,
            &transport,
            regular(11.5),
            FAINT,
        );
        right = transport_rect.left() - 12.0;
        if connected {
            let drawn = painter.text(
                pos2(right, rect.center().y),
                Align2::RIGHT_CENTER,
                "подключено",
                semi(11.5),
                GREEN,
            );
            right = drawn.left() - 12.0;
        }
        let text_left = flag.right() + 14.0;
        let max_width = right - text_left;
        paint_text_truncated(
            painter,
            pos2(text_left, rect.center().y - 9.0),
            Align2::LEFT_CENTER,
            node.title(),
            semi(13.5),
            TEXT,
            max_width,
        );
        // Until geolocation finishes the "country" is just the server name.
        let subtitle = if node.country == node.title() || node.country == node.name {
            node.endpoint()
        } else {
            format!("{}  ·  {}", short_country(&node.country), node.endpoint())
        };
        paint_text_truncated(
            painter,
            pos2(text_left, rect.center().y + 10.0),
            Align2::LEFT_CENTER,
            &subtitle,
            regular(11.5),
            MUTED,
            max_width,
        );
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Клик — выбрать, двойной клик — подключиться")
    }

    // ── subscriptions ────────────────────────────────────────────────────

    pub(crate) fn open_import(&mut self, editing: Option<String>, input: String) {
        let (name, input, auto_update) =
            match editing.as_deref().and_then(|id| self.subscription(id)) {
                Some(sub) => (
                    sub.name.clone(),
                    if sub.is_remote() {
                        sub.url.clone()
                    } else {
                        sub.keys.clone()
                    },
                    sub.auto_update,
                ),
                None => (String::new(), input, true),
            };
        self.import = Some(ImportDialog {
            editing,
            name,
            input,
            auto_update,
            opened_at: Instant::now(),
        });
    }

    fn subscriptions_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let count = self.settings.subscriptions.len();
            Self::page_title(
                ui,
                "Подписки",
                &format!(
                    "{count} {}",
                    universe::subscription::plural(count as i64, "профиль", "профиля", "профилей")
                ),
                270.0,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = "добавить";
                if button(
                    ui,
                    ButtonKind::Primary,
                    Some(Icon::Plus),
                    label,
                    vec2(theme::button_width(ui, label, true, 34.0), 34.0),
                    true,
                )
                .clicked()
                {
                    self.open_import(None, String::new());
                }
                if count > 0 {
                    let refreshing = !self.refreshing.is_empty();
                    let label = if refreshing {
                        "обновляю…"
                    } else {
                        "обновить все"
                    };
                    if button(
                        ui,
                        ButtonKind::Secondary,
                        Some(Icon::RefreshCw),
                        label,
                        vec2(theme::button_width(ui, label, true, 34.0), 34.0),
                        !refreshing,
                    )
                    .clicked()
                    {
                        self.refresh_all(true);
                    }
                }
            });
        });
        ui.add_space(16.0);
        if self.settings.subscriptions.is_empty() {
            self.empty_subscriptions(ui);
            return;
        }
        let ids: Vec<String> = self
            .settings
            .subscriptions
            .iter()
            .map(|sub| sub.id.clone())
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("subscriptions")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let columns = if ui.available_width() > 840.0 { 2 } else { 1 };
                ui.columns(columns, |columns| {
                    for (position, id) in ids.iter().enumerate() {
                        let column = &mut columns[position % columns.len()];
                        self.subscription_card(column, id);
                        column.add_space(14.0);
                    }
                });
            });
    }

    fn empty_subscriptions(&mut self, ui: &mut egui::Ui) {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width(520.0);
            card_frame()
                .inner_margin(egui::Margin::same(30))
                .corner_radius(20)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
                        ui.painter()
                            .circle_filled(rect.center(), 32.0, alpha(ACCENT, 26));
                        ui.painter().circle_stroke(
                            rect.center(),
                            32.0,
                            Stroke::new(1.0_f32, alpha(ACCENT, 70)),
                        );
                        paint_icon(
                            ui,
                            Icon::Orbit,
                            Rect::from_center_size(rect.center(), Vec2::splat(28.0)),
                            ACCENT,
                        );
                        ui.add_space(14.0);
                        ui.label(text_heavy("Добавьте свою подписку", 21.0, TEXT));
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(text(
                                "Вставьте ссылку на подписку от вашего VPN-сервиса или ключи \
                                 vless://, vmess://, trojan://, ss://, hysteria2://. \
                                 universe покажет срок действия, остаток трафика и описание.",
                                13.0,
                                MUTED,
                            ))
                            .wrap(),
                        );
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            let total = 190.0 + 190.0 + 10.0;
                            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                            if button(
                                ui,
                                ButtonKind::Primary,
                                Some(Icon::Plus),
                                "добавить подписку",
                                vec2(190.0, 40.0),
                                true,
                            )
                            .clicked()
                            {
                                self.open_import(None, String::new());
                            }
                            ui.add_space(2.0);
                            if button(
                                ui,
                                ButtonKind::Secondary,
                                Some(Icon::ClipboardPaste),
                                "из буфера",
                                vec2(190.0, 40.0),
                                true,
                            )
                            .clicked()
                            {
                                let clipboard = read_clipboard().unwrap_or_default();
                                self.open_import(None, clipboard.trim().to_owned());
                            }
                        });
                        ui.add_space(16.0);
                        if ui
                            .add(
                                egui::Label::new(text(
                                    "или попробовать бесплатные публичные серверы",
                                    12.0,
                                    ACCENT,
                                ))
                                .sense(Sense::click()),
                            )
                            .on_hover_text(
                                "Серверы неизвестных владельцев из открытого списка 7ec.ru — \
                                 без гарантий и не для важных данных",
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.add_public_feed();
                        }
                    });
                });
        });
    }

    fn subscription_card(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(subscription) = self.subscription(id).cloned() else {
            return;
        };
        let refreshing = self.refreshing.contains(id);
        let confirming = self
            .delete_confirm
            .as_ref()
            .is_some_and(|(pending, at)| pending == id && at.elapsed() < Duration::from_secs(4));
        let mut enabled = subscription.enabled;
        let mut action_refresh = false;
        let mut action_edit = false;
        let mut action_delete = false;
        let mut open_url = None;
        card_frame()
            .inner_margin(egui::Margin::same(16))
            .corner_radius(16)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let row = vec2(ui.available_width(), 44.0);
                ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                    toggle(ui, &mut enabled).on_hover_text("Показывать серверы этой подписки");
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        let (tile, _) = ui.allocate_exact_size(Vec2::splat(42.0), Sense::hover());
                        ui.painter().rect_filled(tile, 12.0, alpha(ACCENT, 24));
                        ui.painter().rect_stroke(
                            tile,
                            12.0,
                            Stroke::new(1.0_f32, alpha(ACCENT, 50)),
                            egui::StrokeKind::Inside,
                        );
                        paint_icon(
                            ui,
                            if subscription.is_remote() {
                                Icon::Link
                            } else {
                                Icon::KeyRound
                            },
                            Rect::from_center_size(tile.center(), Vec2::splat(19.0)),
                            ACCENT,
                        );
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.add(
                                egui::Label::new(text_semi(
                                    subscription.display_name(),
                                    16.0,
                                    TEXT,
                                ))
                                .truncate(),
                            );
                            let subtitle = if subscription.is_remote() {
                                url::Url::parse(&subscription.url)
                                    .ok()
                                    .and_then(|url| url.host_str().map(str::to_owned))
                                    .unwrap_or_else(|| "ссылка на подписку".to_owned())
                            } else {
                                "ключи добавлены вручную".to_owned()
                            };
                            ui.add(egui::Label::new(text(subtitle, 12.0, MUTED)).truncate());
                        });
                    });
                });

                if !subscription.meta.announce.trim().is_empty() {
                    ui.add_space(10.0);
                    egui::Frame::new()
                        .fill(FIELD)
                        .corner_radius(10)
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_top(|ui| {
                                ui.add(lucide_image(Icon::Info, 14.0, ACCENT));
                                ui.add(
                                    egui::Label::new(text(
                                        subscription.meta.announce.trim(),
                                        12.5,
                                        TEXT_DIM,
                                    ))
                                    .wrap(),
                                );
                            });
                        });
                }
                if !subscription.meta.notes.is_empty() {
                    ui.add_space(6.0);
                }
                for note in subscription.meta.notes.iter().take(4) {
                    ui.spacing_mut().interact_size.y = 18.0;
                    ui.horizontal(|ui| {
                        ui.add(lucide_image(Icon::Sparkles, 12.0, FAINT));
                        ui.add(egui::Label::new(text(note, 12.0, MUTED)).truncate());
                    });
                }

                ui.add_space(10.0);
                subscription_details(ui, &subscription);
                if refreshing {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0).color(ACCENT));
                        ui.label(text("обновление…", 12.0, ACCENT));
                    });
                } else if !subscription.last_error.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal_top(|ui| {
                        ui.add(lucide_image(Icon::CircleAlert, 14.0, RED));
                        ui.add(egui::Label::new(text(&subscription.last_error, 12.0, RED)).wrap());
                    });
                }

                ui.add_space(12.0);
                let compact = ui.available_width() < 380.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let sized = |label: &'static str, width: f32| {
                        if compact {
                            ("", 34.0)
                        } else {
                            (label, width)
                        }
                    };
                    if subscription.is_remote() {
                        let (label, width) = sized("обновить", 110.0);
                        action_refresh = button(
                            ui,
                            ButtonKind::Secondary,
                            Some(Icon::RefreshCw),
                            label,
                            vec2(width, 32.0),
                            !refreshing,
                        )
                        .on_hover_text("Обновить подписку")
                        .clicked();
                    }
                    let (label, width) = sized("изменить", 112.0);
                    action_edit = button(
                        ui,
                        ButtonKind::Secondary,
                        Some(Icon::Pencil),
                        label,
                        vec2(width, 32.0),
                        true,
                    )
                    .on_hover_text("Изменить ссылку, ключи или название")
                    .clicked();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if confirming { "удалить?" } else { "" };
                        let width = if confirming { 104.0 } else { 34.0 };
                        action_delete = button(
                            ui,
                            ButtonKind::Danger,
                            Some(Icon::Trash),
                            label,
                            vec2(width, 32.0),
                            true,
                        )
                        .on_hover_text("Удалить подписку")
                        .clicked();
                        if subscription.is_remote()
                            && icon_button(ui, Icon::Copy, "Скопировать ссылку", 32.0).clicked()
                        {
                            ui.ctx().copy_text(subscription.url.clone());
                        }
                        let site = [
                            (&subscription.meta.web_page_url, "Сайт сервиса"),
                            (&subscription.meta.support_url, "Поддержка"),
                        ]
                        .into_iter()
                        .find(|(url, _)| {
                            let lower = url.to_ascii_lowercase();
                            lower.starts_with("https://") || lower.starts_with("http://")
                        });
                        if let Some((url, label)) = site {
                            if icon_button(ui, Icon::ExternalLink, &format!("{label}: {url}"), 32.0)
                                .clicked()
                            {
                                open_url = Some(url.clone());
                            }
                        }
                    });
                });
            });

        if enabled != subscription.enabled {
            if let Some(sub) = self
                .settings
                .subscriptions
                .iter_mut()
                .find(|sub| sub.id == id)
            {
                sub.enabled = enabled;
            }
            self.rebuild_nodes();
            self.persist_settings();
        }
        if action_refresh {
            self.refresh_subscription(id);
        }
        if action_edit {
            self.open_import(Some(id.to_owned()), String::new());
        }
        if let Some(url) = open_url {
            self.open_path(&url);
        }
        if action_delete {
            if confirming {
                self.delete_confirm = None;
                self.remove_subscription(id);
            } else {
                self.delete_confirm = Some((id.to_owned(), Instant::now()));
            }
        }
        if confirming {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
    }

    pub(crate) fn handle_global_paste(&mut self, ctx: &egui::Context) {
        if self.import.is_some() || ctx.memory(|memory| memory.focused().is_some()) {
            return;
        }
        let pasted = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Paste(value) => Some(value.clone()),
                _ => None,
            })
        });
        if let Some(value) = pasted {
            if matches!(
                classify_import(&value),
                ImportInput::Url(_) | ImportInput::Keys { .. }
            ) {
                self.open_import(None, value.trim().to_owned());
            }
        }
    }

    pub(crate) fn import_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.import.take() else {
            return;
        };
        let screen = ctx.screen_rect();
        let reveal = ease_out_cubic(dialog.opened_at.elapsed().as_secs_f32() / 0.18);
        if reveal < 1.0 {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let mut close = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let mut submit = false;
        let radius = self.window_radius();
        egui::Area::new(egui::Id::new("import_backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let response = ui.allocate_rect(screen, Sense::click());
                ui.painter().rect_filled(
                    screen,
                    radius,
                    Color32::from_black_alpha((150.0 * reveal) as u8),
                );
                if response.clicked() {
                    close = true;
                }
            });

        let classification = classify_import(&dialog.input);
        let duplicate = match &classification {
            ImportInput::Url(url) => self.settings.subscriptions.iter().any(|sub| {
                Some(&sub.id) != dialog.editing.as_ref() && sub.url.trim() == url.trim()
            }),
            _ => false,
        };
        let valid = !duplicate
            && matches!(
                classification,
                ImportInput::Url(_) | ImportInput::Keys { .. }
            );
        egui::Area::new(egui::Id::new("import_dialog"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, (1.0 - reveal) * 14.0))
            .show(ctx, |ui| {
                ui.set_opacity(reveal);
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0_f32, BORDER_STRONG))
                    .corner_radius(20)
                    .inner_margin(egui::Margin::same(22))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 20],
                        blur: 48,
                        spread: 0,
                        color: Color32::from_black_alpha(150),
                    })
                    .show(ui, |ui| {
                        ui.set_width(500.0);
                        ui.horizontal(|ui| {
                            ui.label(text_heavy(
                                if dialog.editing.is_some() {
                                    "Изменить подписку"
                                } else {
                                    "Новая подписка"
                                },
                                20.0,
                                TEXT,
                            ));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon_button(ui, Icon::X, "Закрыть", 30.0).clicked() {
                                    close = true;
                                }
                            });
                        });
                        ui.add(
                            egui::Label::new(text(
                                "Ссылка https:// на подписку или ключи — по одному на строке. \
                                 Срок действия, трафик и описание подтянутся автоматически.",
                                12.5,
                                MUTED,
                            ))
                            .wrap(),
                        );
                        ui.add_space(14.0);
                        caption(ui, "ссылка или ключи");
                        inset_frame()
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                let response = ui.add(
                                    egui::TextEdit::multiline(&mut dialog.input)
                                        .font(mono(12.0))
                                        .hint_text(text(
                                            "https://sub.example.com/…\nvless://…",
                                            12.0,
                                            FAINT,
                                        ))
                                        .desired_rows(5)
                                        .desired_width(f32::INFINITY)
                                        .frame(false),
                                );
                                if dialog.opened_at.elapsed() < Duration::from_millis(60) {
                                    response.request_focus();
                                }
                            });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let (icon, message, color) = match &classification {
                                _ if duplicate => (
                                    Icon::CircleAlert,
                                    "эта подписка уже добавлена".to_owned(),
                                    AMBER,
                                ),
                                ImportInput::Empty => {
                                    (Icon::Info, "вставьте ссылку или ключи".to_owned(), MUTED)
                                }
                                ImportInput::Url(url) => (
                                    Icon::Link,
                                    format!(
                                        "ссылка на подписку · {}",
                                        url::Url::parse(url)
                                            .ok()
                                            .and_then(|url| url.host_str().map(str::to_owned))
                                            .unwrap_or_default()
                                    ),
                                    ACCENT,
                                ),
                                ImportInput::Keys { count, summary } => (
                                    Icon::KeyRound,
                                    format!(
                                        "{count} {} · {summary}",
                                        universe::subscription::plural(
                                            *count as i64,
                                            "ключ",
                                            "ключа",
                                            "ключей"
                                        )
                                    ),
                                    GREEN,
                                ),
                                ImportInput::Unknown => (
                                    Icon::CircleAlert,
                                    "не похоже на ссылку или поддерживаемый ключ".to_owned(),
                                    AMBER,
                                ),
                            };
                            ui.add(lucide_image(icon, 14.0, color));
                            ui.add(egui::Label::new(text(message, 12.0, color)).truncate());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if button(
                                    ui,
                                    ButtonKind::Ghost,
                                    Some(Icon::ClipboardPaste),
                                    "вставить",
                                    vec2(104.0, 28.0),
                                    true,
                                )
                                .clicked()
                                {
                                    if let Some(value) = read_clipboard() {
                                        dialog.input = value.trim().to_owned();
                                    }
                                }
                            });
                        });
                        ui.add_space(12.0);
                        caption(ui, "название");
                        inset_frame().show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut dialog.name)
                                    .font(regular(13.5))
                                    .hint_text(text(
                                        "необязательно — возьмём из подписки",
                                        13.0,
                                        FAINT,
                                    ))
                                    .desired_width(f32::INFINITY)
                                    .frame(false),
                            );
                        });
                        if matches!(classification, ImportInput::Url(_)) {
                            ui.add_space(10.0);
                            setting_row(
                                ui,
                                "Автообновление",
                                "обновлять список серверов и данные подписки по расписанию",
                                &mut dialog.auto_update,
                            );
                        }
                        ui.add_space(18.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let label = if dialog.editing.is_some() {
                                "сохранить"
                            } else {
                                "добавить"
                            };
                            if button(
                                ui,
                                ButtonKind::Primary,
                                Some(Icon::Check),
                                label,
                                vec2(150.0, 40.0),
                                valid,
                            )
                            .clicked()
                            {
                                submit = true;
                            }
                            if button(
                                ui,
                                ButtonKind::Secondary,
                                None,
                                "отмена",
                                vec2(110.0, 40.0),
                                true,
                            )
                            .clicked()
                            {
                                close = true;
                            }
                        });
                    });
            });
        if valid
            && ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Enter))
        {
            submit = true;
        }
        if submit && valid {
            self.apply_import(dialog, classification);
        } else if !close {
            self.import = Some(dialog);
        }
    }

    fn apply_import(&mut self, dialog: ImportDialog, classification: ImportInput) {
        self.page = Page::Subscriptions;
        let Some(id) = dialog.editing else {
            match classification {
                ImportInput::Url(url) => {
                    let mut subscription = Subscription::remote(&dialog.name, &url);
                    subscription.auto_update = dialog.auto_update;
                    self.add_subscription(subscription);
                }
                ImportInput::Keys { .. } => {
                    self.add_subscription(Subscription::manual(&dialog.name, &dialog.input));
                }
                _ => {}
            }
            return;
        };
        let Some(subscription) = self
            .settings
            .subscriptions
            .iter_mut()
            .find(|sub| sub.id == id)
        else {
            return;
        };
        subscription.name = dialog.name.trim().to_owned();
        let mut refresh = false;
        match classification {
            ImportInput::Url(url) => {
                if subscription.url != url {
                    subscription.url = url;
                    subscription.meta = Default::default();
                    subscription.updated_at = 0;
                    refresh = true;
                }
                subscription.keys.clear();
                subscription.auto_update = dialog.auto_update;
            }
            ImportInput::Keys { .. } => {
                subscription.url.clear();
                subscription.keys = dialog.input.trim().to_owned();
                subscription.meta = Default::default();
                subscription.last_error.clear();
                subscription.updated_at = now_unix();
                subscription.auto_update = false;
            }
            _ => {}
        }
        if !self
            .settings
            .subscriptions
            .iter()
            .any(|sub| sub.id == id && sub.is_remote())
        {
            self.sub_bodies.remove(&id);
            let _ = fs::remove_file(self.paths.subscription_cache(&id));
        }
        self.rebuild_nodes();
        self.persist_settings();
        if refresh {
            self.refresh_subscription(&id);
        }
        self.set_notice("Подписка сохранена");
    }

    // ── settings ─────────────────────────────────────────────────────────

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        Self::page_title(
            ui,
            "Настройки",
            "применяются при следующем подключении",
            0.0,
        );
        ui.add_space(16.0);
        let mut draft = self.settings.clone();
        let mut install_core = false;
        let mut open_folder = false;
        let server_choices: Vec<(String, String)> = self
            .nodes
            .iter()
            .map(|node| {
                (
                    node.raw.clone(),
                    format!("{} · {}", node.title(), node.protocol.short_label()),
                )
            })
            .collect();
        let selected_raw = self
            .selected
            .and_then(|index| self.nodes.get(index))
            .or_else(|| self.nodes.first())
            .map(|node| node.raw.clone())
            .unwrap_or_default();
        let machine_hwid = self.device.hwid.clone();
        egui::ScrollArea::vertical()
            .id_salt("settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let columns = if ui.available_width() > 840.0 { 2 } else { 1 };
                ui.columns(columns, |columns| {
                    let (left, right) = if columns.len() > 1 {
                        let (left, right) = columns.split_at_mut(1);
                        (&mut left[0], Some(&mut right[0]))
                    } else {
                        (&mut columns[0], None)
                    };
                    titled_card(left, "соединение", |ui| {
                        setting_row(
                            ui,
                            "Kill Switch",
                            "строгая маршрутизация и автопереподключение при обрыве",
                            &mut draft.kill_switch,
                        );
                        setting_row(
                            ui,
                            "Подключаться при запуске",
                            "к последнему выбранному серверу",
                            &mut draft.auto_connect,
                        );
                        setting_row(
                            ui,
                            "Защищённый DNS",
                            "DNS-over-HTTPS (Cloudflare) внутри туннеля",
                            &mut draft.secure_dns,
                        );
                        setting_row(
                            ui,
                            "Локальная сеть",
                            "доступ к принтерам, NAS и устройствам дома",
                            &mut draft.allow_lan,
                        );
                    });
                    left.add_space(14.0);
                    titled_card(left, "приложение", |ui| {
                        setting_row(
                            ui,
                            "Запускать свёрнутым",
                            "сразу в системный трей",
                            &mut draft.start_minimized,
                        );
                        setting_row(
                            ui,
                            "Обновлять подписки",
                            "при запуске и по расписанию в фоне",
                            &mut draft.auto_refresh,
                        );
                        ui.add_enabled_ui(draft.auto_refresh, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(text_semi("Интервал", 13.5, TEXT));
                                    ui.label(text("если провайдер не задал свой", 11.5, MUTED));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    segmented(
                                        ui,
                                        "update_interval",
                                        &mut draft.update_interval_hours,
                                        &[(6, "6 ч"), (12, "12 ч"), (24, "24 ч")],
                                        168.0,
                                        28.0,
                                    );
                                });
                            });
                        });
                        ui.add_space(8.0);
                        ui.label(text_semi("User-Agent подписок", 13.5, TEXT));
                        ui.label(text(
                            "некоторые панели отдают ключи только известным клиентам",
                            11.5,
                            MUTED,
                        ));
                        ui.add_space(4.0);
                        inset_frame().show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut draft.user_agent)
                                    .font(mono(12.0))
                                    .hint_text(DEFAULT_USER_AGENT)
                                    .desired_width(f32::INFINITY)
                                    .frame(false),
                            );
                        });
                        ui.add_space(12.0);
                        setting_row(
                            ui,
                            "Передавать HWID",
                            "идентификатор устройства для сервисов с лимитом устройств — как в Happ и Incy",
                            &mut draft.send_hwid,
                        );
                        hwid_editor(ui, &mut draft, &machine_hwid);
                    });
                    left.add_space(14.0);
                    let core_ready = self.core_path.is_some();
                    let installing = self.installing_core;
                    let data_folder = self.paths.root.display().to_string();
                    titled_card(left, "о программе", |ui| {
                        info_row(ui, ACCENT, "версия", |ui| {
                            info_value(
                                ui,
                                format!("{APP_NAME} {}", env!("CARGO_PKG_VERSION")),
                                TEXT,
                            )
                        });
                        info_row(
                            ui,
                            if core_ready { GREEN } else { AMBER },
                            "VPN-ядро",
                            |ui| {
                                info_value(
                                    ui,
                                    if core_ready {
                                        format!("sing-box {}", universe::singbox::CORE_VERSION)
                                    } else if installing {
                                        "скачивается…".to_owned()
                                    } else {
                                        "не установлено".to_owned()
                                    },
                                    if core_ready { TEXT } else { AMBER },
                                )
                            },
                        );
                        info_row(ui, FAINT, "данные", |ui| {
                            ui.add(egui::Label::new(text(&data_folder, 11.5, MUTED)).truncate());
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            open_folder = button(
                                ui,
                                ButtonKind::Secondary,
                                Some(Icon::FolderOpen),
                                "папка данных",
                                vec2(150.0, 32.0),
                                true,
                            )
                            .clicked();
                            install_core = button(
                                ui,
                                ButtonKind::Secondary,
                                Some(Icon::Download),
                                if core_ready {
                                    "переустановить ядро"
                                } else {
                                    "установить ядро"
                                },
                                vec2(190.0, 32.0),
                                !installing,
                            )
                            .clicked();
                        });
                    });

                    let target = match right {
                        Some(right) => right,
                        None => {
                            left.add_space(14.0);
                            left
                        }
                    };
                    titled_card(
                        target,
                        "раздельное туннелирование",
                        |ui| {
                            setting_row(
                                ui,
                                "Раздельное туннелирование",
                                "свои правила для приложений и сайтов",
                                &mut draft.split_tunnel,
                            );
                            ui.add_enabled_ui(draft.split_tunnel, |ui| {
                                ui.add_space(4.0);
                                let width = ui.available_width();
                                segmented(
                                    ui,
                                    "split_mode",
                                    &mut draft.split_mode,
                                    &[
                                        ("bypass".to_owned(), "в обход VPN"),
                                        ("only".to_owned(), "только через VPN"),
                                    ],
                                    width,
                                    32.0,
                                );
                                ui.add_space(10.0);
                                rules_editor(
                                    ui,
                                    "приложения",
                                    "имя процесса или полный путь — по одному на строке",
                                    "chrome.exe\nTelegram.exe\nC:\\Games\\game.exe",
                                    &mut draft.split_apps,
                                );
                                ui.add_space(8.0);
                                rules_editor(
                                    ui,
                                    "сайты",
                                    "домены без масок — по одному на строке",
                                    "youtube.com\ngithub.com",
                                    &mut draft.split_domains,
                                );
                            });
                        },
                    );
                    target.add_space(14.0);
                    titled_card(
                        target,
                        "routeflow · мульти-маршрутизация",
                        |ui| {
                            setting_row(
                                ui,
                                "RouteFlow",
                                "назначает приложениям и сайтам разные VPN-серверы",
                                &mut draft.route_flow_enabled,
                            );
                            ui.add_enabled_ui(draft.route_flow_enabled, |ui| {
                                let mut remove = None;
                                for (index, rule) in draft.route_flow.iter_mut().enumerate() {
                                    ui.add_space(8.0);
                                    egui::Frame::new()
                                        .fill(FIELD)
                                        .stroke(Stroke::new(1.0_f32, BORDER))
                                        .corner_radius(12)
                                        .inner_margin(egui::Margin::same(10))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                ui.add(lucide_image(Icon::Route, 15.0, ACCENT));
                                                ui.label(text_semi(
                                                    format!("маршрут {:02}", index + 1),
                                                    13.0,
                                                    TEXT,
                                                ));
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        if icon_button(
                                                            ui,
                                                            Icon::Trash,
                                                            "Удалить",
                                                            26.0,
                                                        )
                                                        .clicked()
                                                        {
                                                            remove = Some(index);
                                                        }
                                                        toggle(ui, &mut rule.enabled);
                                                    },
                                                );
                                            });
                                            ui.add_space(6.0);
                                            let selected_label = server_choices
                                                .iter()
                                                .find(|(raw, _)| raw == &rule.server)
                                                .map(|(_, label)| label.as_str())
                                                .unwrap_or("выберите сервер");
                                            egui::ComboBox::from_id_salt((
                                                "routeflow_server",
                                                index,
                                            ))
                                            .width(ui.available_width())
                                            .selected_text(selected_label)
                                            .show_ui(
                                                ui,
                                                |ui| {
                                                    for (raw, label) in &server_choices {
                                                        ui.selectable_value(
                                                            &mut rule.server,
                                                            raw.clone(),
                                                            label,
                                                        );
                                                    }
                                                },
                                            );
                                            ui.add_space(6.0);
                                            rules_editor(
                                                ui,
                                                "приложения",
                                                "процессы или пути",
                                                "chrome.exe",
                                                &mut rule.apps,
                                            );
                                            ui.add_space(6.0);
                                            rules_editor(
                                                ui,
                                                "сайты",
                                                "домены",
                                                "youtube.com",
                                                &mut rule.domains,
                                            );
                                        });
                                }
                                if let Some(index) = remove {
                                    draft.route_flow.remove(index);
                                }
                                ui.add_space(8.0);
                                if button(
                                    ui,
                                    ButtonKind::Secondary,
                                    Some(Icon::Plus),
                                    "добавить правило",
                                    vec2(180.0, 32.0),
                                    !server_choices.is_empty(),
                                )
                                .clicked()
                                {
                                    draft.route_flow.push(RouteFlowSetting {
                                        enabled: true,
                                        server: selected_raw.clone(),
                                        apps: String::new(),
                                        domains: String::new(),
                                    });
                                }
                            });
                        },
                    );
                });
            });

        if open_folder {
            let folder = self.paths.root.display().to_string();
            self.open_path(&folder);
        }
        if install_core {
            self.begin_core_install();
        }
        if draft != self.settings {
            if !draft.kill_switch {
                self.reconnect_at = None;
            }
            let device_changed =
                draft.send_hwid != self.settings.send_hwid || draft.hwid != self.settings.hwid;
            self.settings = draft;
            self.persist_settings();
            if device_changed {
                // Panels with device limits answer differently with a new HWID.
                self.refresh_all(false);
            }
            if self.process.is_some() {
                self.notice = "Настройки сохранены · переподключитесь, чтобы применить".to_owned();
                self.notice_error = false;
            }
        }
    }

    // ── journal ──────────────────────────────────────────────────────────

    fn journal_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            Self::page_title(ui, "Журнал", "события приложения и вывод sing-box", 370.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(
                    ui,
                    ButtonKind::Secondary,
                    Some(Icon::FolderOpen),
                    "папка",
                    vec2(100.0, 34.0),
                    true,
                )
                .clicked()
                {
                    let folder = self.paths.root.display().to_string();
                    self.open_path(&folder);
                }
                if button(
                    ui,
                    ButtonKind::Secondary,
                    Some(Icon::Trash),
                    "очистить",
                    vec2(116.0, 34.0),
                    true,
                )
                .clicked()
                {
                    if self.journal_tab == "core" {
                        self.core_logs.clear();
                    } else {
                        self.journal.clear();
                    }
                }
                if button(
                    ui,
                    ButtonKind::Secondary,
                    Some(Icon::Copy),
                    "копировать",
                    vec2(130.0, 34.0),
                    true,
                )
                .clicked()
                {
                    let content = if self.journal_tab == "core" {
                        self.core_logs
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        self.journal
                            .iter()
                            .map(|line| format!("{} {}", line.time, line.text))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    ui.ctx().copy_text(content);
                    self.set_notice("Журнал скопирован");
                }
            });
        });
        ui.add_space(14.0);
        segmented(
            ui,
            "journal_tab",
            &mut self.journal_tab,
            &[
                ("events".to_owned(), "события"),
                ("core".to_owned(), "sing-box"),
            ],
            240.0,
            32.0,
        );
        ui.add_space(12.0);
        egui::Frame::new()
            .fill(FIELD)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(14)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(ui.available_height());
                egui::ScrollArea::vertical()
                    .id_salt(("journal", self.journal_tab.clone()))
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.journal_tab == "core" {
                            if self.core_logs.is_empty() {
                                ui.label(text(
                                    "вывод sing-box появится после подключения",
                                    12.5,
                                    FAINT,
                                ));
                            }
                            for line in &self.core_logs {
                                let lower = line.to_lowercase();
                                let color = if lower.contains("error") || lower.contains("fatal") {
                                    RED
                                } else if lower.contains("warn") {
                                    AMBER
                                } else {
                                    MUTED
                                };
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(line).font(mono(11.5)).color(color),
                                    )
                                    .wrap(),
                                );
                            }
                        } else {
                            for line in &self.journal {
                                journal_line(ui, line);
                            }
                        }
                    });
            });
    }

    // ── toast ────────────────────────────────────────────────────────────

    pub(crate) fn toast(&self, ctx: &egui::Context) {
        if (self.page == Page::Home && !self.welcome)
            || (self.welcome && !self.notice_error)
            || self.notice.is_empty()
            || self.import.is_some()
        {
            return;
        }
        let age = self.notice_at.elapsed().as_secs_f32();
        if age > 4.2 {
            return;
        }
        let fade = (age / 0.18).min(1.0) * ((4.2 - age) / 0.4).min(1.0);
        ctx.request_repaint_after(Duration::from_millis(32));
        let color = if self.notice_error { RED } else { ACCENT };
        egui::Area::new(egui::Id::new("toast"))
            .order(egui::Order::Foreground)
            .anchor(
                Align2::CENTER_BOTTOM,
                vec2(
                    if self.welcome {
                        0.0
                    } else {
                        self.sidebar_width(ctx) / 2.0
                    },
                    -24.0 + (1.0 - fade) * 10.0,
                ),
            )
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                egui::Frame::new()
                    .fill(SIDEBAR)
                    .stroke(Stroke::new(1.0_f32, BORDER_STRONG))
                    .corner_radius(14)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 10],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_black_alpha(120),
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(520.0);
                        ui.horizontal(|ui| {
                            ui.add(lucide_image(
                                if self.notice_error {
                                    Icon::CircleAlert
                                } else {
                                    Icon::Check
                                },
                                15.0,
                                color,
                            ));
                            ui.add(egui::Label::new(text(&self.notice, 13.0, TEXT)).wrap());
                        });
                    });
            });
    }
}

fn nav_caption(ui: &mut egui::Ui, label: &str, compact: bool) {
    if compact {
        // A short divider keeps the groups apart without text.
        ui.add_space(8.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().line_segment(
            [
                rect.left_center() + vec2(8.0, 0.0),
                rect.right_center() - vec2(8.0, 0.0),
            ],
            Stroke::new(1.0_f32, BORDER),
        );
        ui.add_space(8.0);
        return;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(text(label, 11.5, FAINT));
    });
    ui.add_space(2.0);
}

fn paint_hop_number(ui: &mut egui::Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 10.0, alpha(ACCENT, 30));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        semi(11.0),
        ACCENT,
    );
}

fn paint_sparkline(ui: &mut egui::Ui, history: &VecDeque<(u64, u64)>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let painter = ui.painter();
    let peak = history
        .iter()
        .map(|(up, down)| (*up).max(*down))
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let step = rect.width() / 59.0;
    let offset = 60 - history.len();
    let points = |pick: fn(&(u64, u64)) -> u64| -> Vec<Pos2> {
        history
            .iter()
            .enumerate()
            .map(|(index, sample)| {
                pos2(
                    rect.left() + (offset + index) as f32 * step,
                    rect.bottom() - 2.0 - (pick(sample) as f32 / peak) * (rect.height() - 6.0),
                )
            })
            .collect()
    };
    let down = points(|sample| sample.1);
    let up = points(|sample| sample.0);
    let mut mesh = egui::Mesh::default();
    for (index, point) in down.iter().enumerate() {
        mesh.colored_vertex(*point, alpha(ACCENT, 46));
        mesh.colored_vertex(pos2(point.x, rect.bottom()), alpha(ACCENT, 0));
        if index > 0 {
            let base = (index as u32 - 1) * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 2, base + 3);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.add(egui::Shape::line(down, Stroke::new(1.6_f32, ACCENT)));
    painter.add(egui::Shape::line(
        up,
        Stroke::new(1.0_f32, alpha(GREEN, 170)),
    ));
}

fn journal_line(ui: &mut egui::Ui, line: &JournalLine) {
    let (prefix, color) = match line.kind {
        LineKind::Success => ("[+]", ACCENT),
        LineKind::Error => ("[!]", RED),
        LineKind::Info => ("[·]", MUTED),
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!("{} {prefix} {}", line.time, line.text))
                .font(mono(11.5))
                .color(color),
        )
        .wrap(),
    );
}

/// Shows the HWID with copy and reset actions.
fn hwid_editor(ui: &mut egui::Ui, draft: &mut Settings, machine_hwid: &str) {
    ui.add_enabled_ui(draft.send_hwid, |ui| {
        let custom = !draft.hwid.trim().is_empty();
        let shown = if custom {
            draft.hwid.trim().to_owned()
        } else {
            machine_hwid.to_owned()
        };
        inset_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if button(
                        ui,
                        ButtonKind::Ghost,
                        Some(Icon::RefreshCw),
                        "сбросить",
                        vec2(104.0, 28.0),
                        true,
                    )
                    .on_hover_text("Новый случайный HWID — сервис посчитает это новым устройством")
                    .clicked()
                    {
                        draft.hwid = universe::device::random_hwid();
                    }
                    if icon_button(ui, Icon::Copy, "Скопировать HWID", 28.0).clicked() {
                        ui.ctx().copy_text(shown.clone());
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&shown).font(mono(12.0)).color(TEXT_DIM),
                            )
                            .truncate(),
                        );
                    });
                });
            });
        });
        ui.add_space(2.0);
        if custom && !machine_hwid.is_empty() {
            if ui
                .add(
                    egui::Label::new(text(
                        "случайный HWID · вернуть HWID компьютера",
                        11.5,
                        ACCENT,
                    ))
                    .sense(Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                draft.hwid.clear();
            }
        } else {
            ui.add(
                egui::Label::new(text(
                    "постоянный, вычисляется из идентификатора Windows; сам он не передаётся",
                    11.5,
                    MUTED,
                ))
                .wrap(),
            );
        }
    });
}

fn setting_row(ui: &mut egui::Ui, title: &str, description: &str, value: &mut bool) {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 52.0).max(120.0);
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(text_semi(title, 13.5, TEXT));
            ui.add(egui::Label::new(text(description, 11.5, MUTED)).wrap());
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            toggle(ui, value);
        });
    });
    ui.add_space(8.0);
}

fn rules_editor(ui: &mut egui::Ui, title: &str, description: &str, hint: &str, value: &mut String) {
    ui.label(text_semi(title, 12.5, TEXT_DIM));
    ui.label(text(description, 11.0, MUTED));
    ui.add_space(3.0);
    inset_frame().show(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(value)
                .font(mono(12.0))
                .hint_text(text(hint, 12.0, FAINT))
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .frame(false),
        );
    });
}

/// Days left and traffic, used on the home page.
fn subscription_summary(ui: &mut egui::Ui, subscription: &Subscription) {
    let now = now_unix();
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(text_semi(subscription.display_name(), 13.5, TEXT)).truncate());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (label, color) = remaining_label(subscription, now);
            ui.label(text_semi(label, 12.5, color));
        });
    });
    if subscription.meta.has_traffic {
        ui.add_space(6.0);
        if let Some(ratio) = subscription.traffic_ratio() {
            progress_bar(ui, ratio, traffic_color(ratio), 6.0);
            ui.add_space(4.0);
        }
        ui.label(text(traffic_label(subscription), 11.5, MUTED));
    }
}

/// Full information block of a subscription card.
fn subscription_details(ui: &mut egui::Ui, subscription: &Subscription) {
    let now = now_unix();
    let servers = if subscription.skipped > 0 {
        format!(
            "{} · пропущено {}",
            subscription.node_count, subscription.skipped
        )
    } else {
        subscription.node_count.to_string()
    };
    info_row(ui, ACCENT, "серверы", |ui| {
        info_value(ui, servers, TEXT)
    });
    let (remaining, color) = remaining_label(subscription, now);
    let remaining = if subscription.meta.expire > 0 {
        format!("{remaining} · до {}", format_date(subscription.meta.expire))
    } else {
        remaining
    };
    info_row(ui, color, "срок", |ui| info_value(ui, remaining, color));
    if subscription.meta.has_traffic {
        let ratio = subscription.traffic_ratio();
        info_row(
            ui,
            ratio.map(traffic_color).unwrap_or(ACCENT),
            "трафик",
            |ui| info_value(ui, traffic_label(subscription), TEXT),
        );
        if let Some(ratio) = ratio {
            ui.add_space(2.0);
            progress_bar(ui, ratio, traffic_color(ratio), 6.0);
            ui.add_space(4.0);
        }
    }
    let updated = if subscription.is_remote() {
        let mut value = format_ago(subscription.updated_at, now);
        if subscription.auto_update {
            value.push_str(&format!(
                " · каждые {} ч",
                subscription.update_interval_hours(DEFAULT_UPDATE_HOURS)
            ));
        }
        value
    } else {
        "вручную".to_owned()
    };
    info_row(ui, FAINT, "обновлено", |ui| {
        info_value(ui, updated, TEXT_DIM)
    });
}

fn remaining_label(subscription: &Subscription, now: u64) -> (String, Color32) {
    match subscription.seconds_left(now) {
        None => (
            if subscription.is_remote() && subscription.updated_at == 0 {
                "—".to_owned()
            } else {
                "бессрочно".to_owned()
            },
            TEXT,
        ),
        Some(seconds) if seconds <= 0 => ("истекла".to_owned(), RED),
        Some(seconds) => (
            format!("осталось {}", format_remaining(seconds)),
            if seconds < 3 * 86_400 {
                RED
            } else if seconds < 7 * 86_400 {
                AMBER
            } else {
                GREEN
            },
        ),
    }
}

fn traffic_label(subscription: &Subscription) -> String {
    let used = format_bytes(subscription.traffic_used());
    if subscription.meta.total > 0 {
        format!("{used} из {}", format_bytes(subscription.meta.total))
    } else {
        format!("{used} · безлимит")
    }
}

fn traffic_color(ratio: f32) -> Color32 {
    if ratio >= 0.9 {
        RED
    } else if ratio >= 0.7 {
        AMBER
    } else {
        ACCENT
    }
}

fn compact_count(count: usize) -> String {
    if count >= 1000 {
        format!("{}k", count / 1000)
    } else {
        count.to_string()
    }
}

pub fn short_country(country: &str) -> String {
    let normalized = country.to_lowercase();
    if normalized.contains("российская федерация") || normalized == "russian federation"
    {
        return "Россия".to_owned();
    }
    if normalized.contains("соединенные штаты")
        || normalized.contains("соединённые штаты")
        || normalized == "united states of america"
    {
        return "США".to_owned();
    }
    if normalized == "united kingdom of great britain and northern ireland" {
        return "Великобритания".to_owned();
    }
    if country.chars().count() <= 18 {
        country.to_owned()
    } else {
        country.chars().take(16).collect::<String>() + "…"
    }
}

pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let hours = minutes / 60;
    if hours > 0 {
        format!("{hours}:{:02}:{:02}", minutes % 60, seconds % 60)
    } else {
        format!("{minutes}:{:02}", seconds % 60)
    }
}
