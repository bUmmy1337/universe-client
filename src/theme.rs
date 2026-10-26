//! Visual language of universe: graphite surfaces, soft borders and a single
//! lavender accent, plus the small set of widgets every page is built from.

use base64::Engine as _;
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Rect,
    RichText, Sense, Stroke, Vec2,
};
use lucide_static_svg::Icon;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub const BG: Color32 = Color32::from_rgb(13, 13, 15);
pub const SIDEBAR: Color32 = Color32::from_rgb(17, 17, 20);
pub const CARD: Color32 = Color32::from_rgb(22, 22, 26);
pub const CARD_HOVER: Color32 = Color32::from_rgb(27, 27, 32);
pub const FIELD: Color32 = Color32::from_rgb(16, 16, 19);
pub const ELEVATED: Color32 = Color32::from_rgb(35, 35, 41);
pub const BORDER: Color32 = Color32::from_rgb(37, 37, 43);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(54, 54, 62);
pub const TEXT: Color32 = Color32::from_rgb(242, 242, 247);
pub const TEXT_DIM: Color32 = Color32::from_rgb(196, 196, 206);
pub const MUTED: Color32 = Color32::from_rgb(134, 134, 147);
pub const FAINT: Color32 = Color32::from_rgb(82, 82, 94);
pub const ACCENT: Color32 = Color32::from_rgb(165, 180, 252);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(190, 200, 255);
pub const ON_ACCENT: Color32 = Color32::from_rgb(17, 18, 34);
pub const GREEN: Color32 = Color32::from_rgb(110, 231, 183);
pub const AMBER: Color32 = Color32::from_rgb(252, 211, 77);
pub const RED: Color32 = Color32::from_rgb(252, 133, 133);

const SEMI: &str = "onest-semibold";
const HEAVY: &str = "onest-extrabold";

pub fn alpha(color: Color32, value: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), value)
}

pub fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        channel(from.r(), to.r()),
        channel(from.g(), to.g()),
        channel(from.b(), to.b()),
        channel(from.a(), to.a()),
    )
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn semi(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMI.into()))
}

pub fn heavy(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(HEAVY.into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn text(value: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(value.into()).font(regular(size)).color(color)
}

pub fn text_semi(value: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(value.into()).font(semi(size)).color(color)
}

pub fn text_heavy(value: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(value.into()).font(heavy(size)).color(color)
}

pub fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallbacks: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|name| name != "Ubuntu-Light")
        .collect();
    for (name, bytes) in [
        (
            "onest",
            &include_bytes!("../assets/fonts/Onest-Regular.ttf")[..],
        ),
        (
            SEMI,
            &include_bytes!("../assets/fonts/Onest-SemiBold.ttf")[..],
        ),
        (
            HEAVY,
            &include_bytes!("../assets/fonts/Onest-ExtraBold.ttf")[..],
        ),
        (
            "jetbrains-mono",
            &include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf")[..],
        ),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), FontData::from_static(bytes).into());
    }
    let mut emoji = Vec::new();
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\seguiemj.ttf") {
        fonts
            .font_data
            .insert("segoe-emoji".to_owned(), FontData::from_owned(bytes).into());
        emoji.push("segoe-emoji".to_owned());
    }
    let family = |primary: &str| {
        let mut list = vec![primary.to_owned()];
        if primary != "onest" {
            list.push("onest".to_owned());
        }
        list.extend(emoji.iter().cloned());
        list.extend(fallbacks.iter().cloned());
        list
    };
    fonts
        .families
        .insert(FontFamily::Proportional, family("onest"));
    fonts
        .families
        .insert(FontFamily::Name(SEMI.into()), family(SEMI));
    fonts
        .families
        .insert(FontFamily::Name(HEAVY.into()), family(HEAVY));
    fonts
        .families
        .insert(FontFamily::Monospace, family("jetbrains-mono"));
    ctx.set_fonts(fonts);
}

pub fn configure_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.window_fill = CARD;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.extreme_bg_color = FIELD;
    visuals.faint_bg_color = CARD;
    visuals.override_text_color = Some(TEXT);
    visuals.widgets.noninteractive.bg_fill = CARD;
    visuals.widgets.noninteractive.weak_bg_fill = CARD;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_DIM);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = 9.into();
    }
    visuals.widgets.inactive.bg_fill = ELEVATED;
    visuals.widgets.inactive.weak_bg_fill = ELEVATED;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_DIM);
    visuals.widgets.hovered.bg_fill = CARD_HOVER;
    visuals.widgets.hovered.weak_bg_fill = CARD_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BORDER_STRONG);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.active.bg_fill = ELEVATED;
    visuals.widgets.active.weak_bg_fill = ELEVATED;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.open.weak_bg_fill = ELEVATED;
    visuals.selection.bg_fill = alpha(ACCENT, 70);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.hyperlink_color = ACCENT;
    visuals.text_cursor.stroke = Stroke::new(1.5_f32, ACCENT);
    visuals.window_corner_radius = 14.into();
    visuals.menu_corner_radius = 12.into();
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    visuals.popup_shadow = visuals.window_shadow;
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.animation_time = 0.18;
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(12.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style
        .text_styles
        .insert(egui::TextStyle::Body, regular(13.5));
    style
        .text_styles
        .insert(egui::TextStyle::Button, regular(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, regular(11.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, mono(12.0));
    ctx.set_style(style);
}

pub fn lucide_image(icon: Icon, size: f32, color: Color32) -> egui::Image<'static> {
    let svg = icon.svg_str().replace("currentColor", "#ffffff");
    egui::Image::from_bytes(format!("bytes://lucide/{icon}.svg"), svg.into_bytes())
        .fit_to_exact_size(Vec2::splat(size))
        .tint(color)
}

pub fn paint_icon(ui: &egui::Ui, icon: Icon, rect: Rect, color: Color32) {
    lucide_image(icon, rect.width().min(rect.height()), color).paint_at(ui, rect);
}

/// Rounded surface used for every card in the app.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(alpha(CARD, 214))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(14))
}

pub fn inset_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(alpha(FIELD, 220))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(11, 8))
}

/// Small lowercase caption above a block, like "status" in the reference.
pub fn caption(ui: &mut egui::Ui, value: &str) {
    ui.add_space(2.0);
    ui.label(text(value, 12.0, MUTED));
    ui.add_space(2.0);
}

/// Card with a centered title strip, as used for settings groups.
pub fn titled_card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(alpha(CARD, 214))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(14)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let (header, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover());
            ui.painter().text(
                header.center(),
                Align2::CENTER_CENTER,
                title,
                semi(13.5),
                TEXT,
            );
            ui.painter().line_segment(
                [header.left_bottom(), header.right_bottom()],
                Stroke::new(1.0_f32, BORDER),
            );
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 14,
                    right: 14,
                    top: 8,
                    bottom: 12,
                })
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    add_contents(ui);
                });
        });
}

pub fn toggle(ui: &mut egui::Ui, value: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let on = ui.ctx().animate_bool(response.id.with("toggle"), *value);
    let hover = ui
        .ctx()
        .animate_bool(response.id.with("hover"), response.hovered());
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, mix(ELEVATED, ACCENT, on));
    painter.rect_stroke(
        rect,
        10.0,
        Stroke::new(
            1.0_f32,
            mix(mix(BORDER_STRONG, FAINT, hover * 0.6), ACCENT, on),
        ),
        egui::StrokeKind::Inside,
    );
    let knob_x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), on);
    painter.circle_filled(
        pos2(knob_x, rect.center().y),
        6.5,
        mix(Color32::from_rgb(170, 170, 182), ON_ACCENT, on),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Segmented control with a sliding selection pill.
pub fn segmented<T: PartialEq + Clone>(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    current: &mut T,
    options: &[(T, &str)],
    width: f32,
    height: f32,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, height * 0.5, FIELD);
    painter.rect_stroke(
        rect,
        height * 0.5,
        Stroke::new(1.0_f32, BORDER),
        egui::StrokeKind::Inside,
    );
    if options.is_empty() {
        return false;
    }
    let inner = rect.shrink(3.0);
    let slot = inner.width() / options.len() as f32;
    let selected = options
        .iter()
        .position(|(value, _)| value == current)
        .unwrap_or(0);
    let id = ui.id().with(("segmented", id_salt));
    let x = ui.ctx().animate_value_with_time(id, selected as f32, 0.16);
    let pill = Rect::from_min_size(
        pos2(inner.left() + slot * x, inner.top()),
        vec2(slot, inner.height()),
    );
    painter.rect_filled(pill, inner.height() * 0.5, ELEVATED);
    painter.rect_stroke(
        pill,
        inner.height() * 0.5,
        Stroke::new(1.0_f32, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    let mut changed = false;
    for (index, (value, label)) in options.iter().enumerate() {
        let cell = Rect::from_min_size(
            pos2(inner.left() + slot * index as f32, inner.top()),
            vec2(slot, inner.height()),
        );
        let response = ui
            .interact(cell, id.with(index), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let active = 1.0 - (x - index as f32).abs().min(1.0);
        let color = mix(
            if response.hovered() { TEXT_DIM } else { MUTED },
            TEXT,
            active,
        );
        painter.text(
            cell.center(),
            Align2::CENTER_CENTER,
            *label,
            if index == selected {
                semi(12.5)
            } else {
                regular(12.5)
            },
            color,
        );
        if response.clicked() && index != selected {
            *current = value.clone();
            changed = true;
        }
    }
    changed
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// Pill button painted by hand so every page shares the same proportions.
pub fn button(
    ui: &mut egui::Ui,
    kind: ButtonKind,
    icon: Option<Icon>,
    label: &str,
    size: Vec2,
    enabled: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hover = ui
        .ctx()
        .animate_bool(response.id.with("hover"), enabled && response.hovered());
    let pressed = enabled && response.is_pointer_button_down_on();
    let radius = (size.y * 0.5).min(14.0);
    let (fill, stroke, color) = match kind {
        ButtonKind::Primary => (
            mix(ACCENT, ACCENT_HOVER, hover),
            Color32::TRANSPARENT,
            ON_ACCENT,
        ),
        ButtonKind::Secondary => (
            mix(ELEVATED, CARD_HOVER, hover),
            mix(BORDER, BORDER_STRONG, hover),
            TEXT,
        ),
        ButtonKind::Ghost => (
            alpha(ELEVATED, (200.0 * hover) as u8),
            Color32::TRANSPARENT,
            mix(TEXT_DIM, TEXT, hover),
        ),
        ButtonKind::Danger => (
            alpha(RED, (18.0 + 26.0 * hover) as u8),
            alpha(RED, (60.0 + 50.0 * hover) as u8),
            RED,
        ),
    };
    let rect_drawn = if pressed { rect.shrink(1.0) } else { rect };
    let painter = ui.painter();
    let fill = if enabled {
        fill
    } else {
        alpha(fill, fill.a() / 3)
    };
    let color = if enabled { color } else { alpha(color, 110) };
    painter.rect_filled(rect_drawn, radius, fill);
    if stroke != Color32::TRANSPARENT {
        painter.rect_stroke(
            rect_drawn,
            radius,
            Stroke::new(1.0_f32, stroke),
            egui::StrokeKind::Inside,
        );
    }
    let font = if size.y >= 40.0 {
        semi(15.0)
    } else {
        semi(12.5)
    };
    let galley = painter.layout_no_wrap(label.to_owned(), font, color);
    let icon_size = if size.y >= 40.0 { 17.0 } else { 14.0 };
    let gap = if label.is_empty() { 0.0 } else { 7.0 };
    let content_width = galley.size().x + icon.map(|_| icon_size + gap).unwrap_or_default();
    let mut x = rect_drawn.center().x - content_width / 2.0;
    if let Some(icon) = icon {
        let icon_rect = Rect::from_min_size(
            pos2(x, rect_drawn.center().y - icon_size / 2.0),
            Vec2::splat(icon_size),
        );
        paint_icon(ui, icon, icon_rect, color);
        x += icon_size + gap;
    }
    painter.galley(
        pos2(x, rect_drawn.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Width a button needs for its label, icon and padding.
pub fn button_width(ui: &egui::Ui, label: &str, icon: bool, height: f32) -> f32 {
    let font = if height >= 40.0 {
        semi(15.0)
    } else {
        semi(12.5)
    };
    let width = ui.fonts(|fonts| fonts.layout_no_wrap(label.to_owned(), font, TEXT).size().x);
    width + if icon { 21.0 } else { 0.0 } + if label.is_empty() { 0.0 } else { 30.0 }
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, hint: &str, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let hover = ui
        .ctx()
        .animate_bool(response.id.with("hover"), response.hovered());
    ui.painter()
        .rect_filled(rect, 9.0, alpha(ELEVATED, (230.0 * hover) as u8));
    let icon_size = (size * 0.46).round();
    paint_icon(
        ui,
        icon,
        Rect::from_center_size(rect.center(), Vec2::splat(icon_size)),
        mix(MUTED, TEXT, hover),
    );
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if hint.is_empty() {
        response
    } else {
        response.on_hover_text(hint)
    }
}

/// Status line inside a rows card: colored dot, label on the left, value on the right.
pub fn info_row(
    ui: &mut egui::Ui,
    dot: Color32,
    label: &str,
    add_value: impl FnOnce(&mut egui::Ui),
) {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 28.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let (dot_rect, _) = ui.allocate_exact_size(vec2(12.0, 28.0), Sense::hover());
            ui.painter()
                .circle_filled(pos2(dot_rect.left() + 4.0, dot_rect.center().y), 3.2, dot);
            ui.label(text(label, 13.0, MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_value);
        },
    );
}

pub fn info_value(ui: &mut egui::Ui, value: impl Into<String>, color: Color32) {
    ui.add(egui::Label::new(text_semi(value, 13.0, color)).truncate());
}

pub fn progress_bar(ui: &mut egui::Ui, ratio: f32, color: Color32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter().rect_filled(rect, height * 0.5, ELEVATED);
    let filled = Rect::from_min_size(
        rect.min,
        vec2((rect.width() * ratio.clamp(0.0, 1.0)).max(height), height),
    );
    if ratio > 0.0 {
        ui.painter().rect_filled(filled, height * 0.5, color);
    }
}

/// Square tile with a country flag (falls back to a globe).
pub fn flag_tile(ui: &mut egui::Ui, flag: &str, size: Vec2, highlight: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    paint_flag(ui, rect, flag, highlight);
}

pub fn paint_flag(ui: &egui::Ui, rect: Rect, flag: &str, highlight: Option<Color32>) {
    let radius = (rect.height() * 0.24).round();
    ui.painter().rect_filled(rect, radius, ELEVATED);
    if let Some((code, bytes)) = flag_svg(flag) {
        let image_rect = rect.shrink2(vec2(rect.width() * 0.16, rect.height() * 0.2));
        egui::Image::from_bytes(format!("bytes://country-flags/{code}.svg"), bytes)
            .fit_to_exact_size(image_rect.size())
            .corner_radius(3)
            .paint_at(ui, image_rect);
    } else {
        paint_icon(
            ui,
            Icon::Earth,
            Rect::from_center_size(rect.center(), Vec2::splat(rect.height() * 0.5)),
            MUTED,
        );
    }
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0_f32, highlight.unwrap_or(BORDER)),
        egui::StrokeKind::Inside,
    );
}

pub fn country_code_flag(code: &str) -> Option<String> {
    let code = code.trim().to_ascii_uppercase();
    if code.len() != 2 || !code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return None;
    }
    code.bytes()
        .map(|byte| char::from_u32(0x1F1E6 + u32::from(byte - b'A')))
        .collect()
}

pub fn flag_code(flag: &str) -> Option<String> {
    let code: String = flag
        .chars()
        .filter_map(|character| {
            let value = character as u32;
            (0x1F1E6..=0x1F1FF)
                .contains(&value)
                .then(|| char::from_u32('A' as u32 + value - 0x1F1E6))
                .flatten()
        })
        .collect();
    (code.len() == 2).then_some(code)
}

fn flag_svg(flag: &str) -> Option<(String, Arc<[u8]>)> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<[u8]>>>> = OnceLock::new();

    let code = flag_code(flag)?;
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(bytes) = cache.lock().ok()?.get(&code).cloned() {
        return Some((code, bytes));
    }
    let data_uri = rs_grid_icons::flag_data_uri(&code)?;
    let payload = data_uri.strip_prefix("data:image/svg+xml;base64,")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .ok()?;
    let bytes: Arc<[u8]> = decoded.into();
    cache.lock().ok()?.insert(code.clone(), bytes.clone());
    Some((code, bytes))
}

pub fn paint_radial_glow(painter: &egui::Painter, center: egui::Pos2, radius: f32, color: Color32) {
    for layer in (1..=14).rev() {
        let t = layer as f32 / 14.0;
        painter.circle_filled(
            center,
            radius * t,
            alpha(color, ((color.a() as f32) * (1.0 - t) * 0.24) as u8),
        );
    }
}

/// Single-line text field in an inset frame with an optional leading icon.
pub fn search_field(
    ui: &mut egui::Ui,
    value: &mut String,
    hint: &str,
    icon: Icon,
    width: f32,
) -> egui::Response {
    let mut response = None;
    inset_frame()
        .corner_radius(11)
        .inner_margin(egui::Margin::symmetric(11, 6))
        .show(ui, |ui| {
            ui.set_width(width - 24.0);
            ui.horizontal(|ui| {
                ui.add(lucide_image(icon, 15.0, MUTED));
                response = Some(
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .hint_text(text(hint, 13.0, FAINT))
                            .font(regular(13.5))
                            .desired_width(ui.available_width())
                            .frame(false),
                    ),
                );
            });
        });
    response.expect("text field is always added")
}
