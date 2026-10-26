//! First-run screen: pick a server source while sing-box downloads.

use super::*;
use crate::map::ease_out_cubic;
use crate::pages::paint_painter_badge;
use crate::theme::{
    alpha, button, heavy, mix, paint_icon, paint_radial_glow, regular, semi, text, text_heavy,
    ButtonKind, ACCENT, BORDER, CARD, FAINT, GREEN, MUTED, RED, TEXT,
};
use eframe::egui::{pos2, vec2, Align2, Color32, Rect, Sense, Stroke, UiBuilder};
use lucide_static_svg::Icon;
use universe::subscription::format_bytes;

const CONTENT_WIDTH: f32 = 660.0;
const CONTENT_HEIGHT: f32 = 500.0;

impl UniverseApp {
    pub(crate) fn draw_welcome(&mut self, ctx: &egui::Context) {
        self.paint_window(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 22,
                right: 18,
                top: 0,
                bottom: 18,
            }))
            .show(ctx, |ui| {
                self.welcome_top_bar(ui, ctx);
                let free = ui.available_height();
                egui::ScrollArea::vertical()
                    .id_salt("welcome")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(((free - CONTENT_HEIGHT) / 2.0).max(0.0));
                        self.welcome_content(ui);
                    });
            });
        self.paint_window_border(ctx);
        self.handle_global_paste(ctx);
        self.import_dialog(ctx);
        self.toast(ctx);
    }

    fn welcome_top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::hover());
        let y = rect.center().y;
        let controls = self.window_controls(ui, ctx, rect.right(), y);
        let logo = Rect::from_center_size(pos2(rect.left() + 12.0, y), Vec2::splat(24.0));
        egui::Image::from_texture(&self.brand_texture)
            .fit_to_exact_size(logo.size())
            .corner_radius(6)
            .paint_at(ui, logo);
        ui.painter().text(
            pos2(logo.right() + 10.0, y),
            Align2::LEFT_CENTER,
            APP_NAME,
            heavy(15.0),
            TEXT,
        );
        let drag_rect = Rect::from_min_max(rect.min, pos2(controls.left() - 8.0, rect.bottom()));
        let drag = ui.interact(
            drag_rect,
            ui.id().with("welcome_drag_region"),
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

    fn welcome_content(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width().min(CONTENT_WIDTH);
        let left = ui.max_rect().center().x - width / 2.0;

        let (logo_row, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), 80.0), Sense::hover());
        let logo = Rect::from_center_size(logo_row.center(), Vec2::splat(76.0));
        paint_radial_glow(ui.painter(), logo.center(), 120.0, alpha(ACCENT, 60));
        egui::Image::from_texture(&self.brand_texture)
            .fit_to_exact_size(logo.size())
            .corner_radius(18)
            .paint_at(ui, logo);
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width(width);
            ui.label(text_heavy("Добро пожаловать в universe", 27.0, TEXT));
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(text(
                    "Выберите, откуда брать серверы. Это можно изменить позже в разделе «подписки».",
                    13.5,
                    MUTED,
                ))
                .wrap(),
            );
        });
        ui.add_space(18.0);

        let gap = 14.0;
        let card_width = (width - gap) / 2.0;
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 194.0), Sense::hover());
        let own = Rect::from_min_size(pos2(left, row.top()), vec2(card_width, row.height()));
        let public = own.translate(vec2(card_width + gap, 0.0));
        if welcome_choice(
            ui,
            own,
            "own",
            Icon::Link,
            "Своя подписка",
            ("рекомендуем", ACCENT),
            "Ссылка от вашего VPN-сервиса или ключи vless, vmess, trojan, ss, hysteria2. \
             Покажем срок действия, трафик и описание.",
            "добавить ссылку или ключи",
        ) {
            self.open_import(None, String::new());
        }
        if welcome_choice(
            ui,
            public,
            "public",
            Icon::Earth,
            "Публичные серверы",
            ("бесплатно", GREEN),
            "Открытый список 7ec.ru — можно начать сразу. Серверы неизвестных владельцев: \
             не используйте их для важных данных.",
            "использовать 7ec.ru",
        ) {
            self.add_public_feed();
            self.finish_welcome(Page::Home);
        }

        ui.add_space(12.0);
        let (core_row, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), 78.0), Sense::hover());
        let core_rect = Rect::from_min_size(pos2(left, core_row.top()), vec2(width, 78.0));
        self.welcome_core_card(ui, core_rect);

        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            if ui
                .add(egui::Label::new(text("настрою позже", 12.5, FAINT)).sense(Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.finish_welcome(Page::Subscriptions);
            }
        });
        ui.add_space(8.0);
    }

    fn welcome_core_card(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 16.0, alpha(CARD, 214));
        painter.rect_stroke(
            rect,
            16.0,
            Stroke::new(1.0_f32, BORDER),
            egui::StrokeKind::Inside,
        );

        let version = universe::singbox::CORE_VERSION;
        let (icon, color, title, subtitle) = if self.core_path.is_some() {
            (
                Icon::Check,
                GREEN,
                "VPN-ядро готово".to_owned(),
                format!("sing-box {version} · проверено по SHA-256"),
            )
        } else if self.installing_core {
            let progress = match self.core_progress {
                Some((downloaded, Some(total))) if downloaded > 0 => format!(
                    "{} из {} · github.com/SagerNet",
                    format_bytes(downloaded),
                    format_bytes(total)
                ),
                Some((downloaded, None)) if downloaded > 0 => {
                    format!("{} · github.com/SagerNet", format_bytes(downloaded))
                }
                _ => "подключаюсь к GitHub…".to_owned(),
            };
            (
                Icon::Download,
                ACCENT,
                format!("Скачиваю sing-box {version}"),
                progress,
            )
        } else if let Some(error) = &self.core_error {
            (
                Icon::CircleAlert,
                RED,
                "Не удалось скачать VPN-ядро".to_owned(),
                error.clone(),
            )
        } else {
            (
                Icon::Download,
                MUTED,
                format!("VPN-ядро sing-box {version}"),
                "ожидает загрузки".to_owned(),
            )
        };

        let tile = Rect::from_min_size(
            pos2(rect.left() + 18.0, rect.center().y - 21.0),
            Vec2::splat(42.0),
        );
        painter.rect_filled(tile, 12.0, alpha(color, 26));
        painter.rect_stroke(
            tile,
            12.0,
            Stroke::new(1.0_f32, alpha(color, 60)),
            egui::StrokeKind::Inside,
        );
        paint_icon(
            ui,
            icon,
            Rect::from_center_size(tile.center(), Vec2::splat(19.0)),
            color,
        );

        // right side: percent, retry or a "ready" badge
        let mut right = rect.right() - 18.0;
        if self.core_path.is_some() {
            right = paint_painter_badge(&painter, right, rect.center().y, "готово", GREEN) - 12.0;
        } else if self.installing_core {
            if let Some((downloaded, Some(total))) = self.core_progress.filter(|(_, t)| t.is_some())
            {
                let percent = (downloaded as f64 / total.max(1) as f64 * 100.0).round();
                let drawn = painter.text(
                    pos2(right, rect.center().y - 6.0),
                    Align2::RIGHT_CENTER,
                    format!("{percent:.0}%"),
                    semi(14.0),
                    ACCENT,
                );
                right = drawn.left() - 12.0;
            }
        } else {
            let label = if self.core_error.is_some() {
                "повторить"
            } else {
                "скачать"
            };
            let button_rect = Rect::from_min_size(
                pos2(right - 124.0, rect.center().y - 16.0),
                vec2(124.0, 32.0),
            );
            let clicked = ui
                .scope_builder(UiBuilder::new().max_rect(button_rect), |ui| {
                    button(
                        ui,
                        ButtonKind::Secondary,
                        Some(Icon::RefreshCw),
                        label,
                        button_rect.size(),
                        true,
                    )
                    .clicked()
                })
                .inner;
            if clicked {
                self.begin_core_install();
            }
            right = button_rect.left() - 12.0;
        }

        let text_left = tile.right() + 14.0;
        painter.text(
            pos2(text_left, rect.top() + 25.0),
            Align2::LEFT_CENTER,
            title,
            semi(14.0),
            TEXT,
        );
        let mut job = egui::text::LayoutJob::simple_singleline(
            subtitle,
            regular(12.0),
            if self.core_error.is_some() && self.core_path.is_none() && !self.installing_core {
                RED
            } else {
                MUTED
            },
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((right - text_left).max(40.0));
        let galley = painter.layout_job(job);
        painter.galley(pos2(text_left, rect.top() + 36.0), galley, MUTED);

        if self.installing_core {
            let track = Rect::from_min_max(
                pos2(text_left, rect.bottom() - 17.0),
                pos2(rect.right() - 18.0, rect.bottom() - 13.0),
            );
            painter.rect_filled(track, 2.0, alpha(ACCENT, 26));
            match self.core_progress {
                Some((downloaded, Some(total))) => {
                    let ratio = (downloaded as f32 / total.max(1) as f32).clamp(0.0, 1.0);
                    let filled =
                        Rect::from_min_size(track.min, vec2(track.width() * ratio, track.height()));
                    painter.rect_filled(filled, 2.0, ACCENT);
                }
                _ => {
                    // Unknown size: a segment sweeping across the track.
                    let time = ui.input(|input| input.time) as f32;
                    let phase = (time * 0.7).fract();
                    let start = track.left() + (phase * 1.3 - 0.3) * track.width();
                    let segment = Rect::from_min_max(
                        pos2(start.max(track.left()), track.top()),
                        pos2(
                            (start + track.width() * 0.3).min(track.right()),
                            track.bottom(),
                        ),
                    );
                    if segment.width() > 0.0 {
                        painter.rect_filled(segment, 2.0, ACCENT);
                    }
                }
            }
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }
}

/// Large clickable option card. Returns true when it was clicked.
#[allow(clippy::too_many_arguments)]
fn welcome_choice(
    ui: &mut egui::Ui,
    rect: Rect,
    id: &str,
    icon: Icon,
    title: &str,
    badge: (&str, Color32),
    description: &str,
    action: &str,
) -> bool {
    let response = ui
        .interact(rect, ui.id().with(("welcome_choice", id)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let hover = ease_out_cubic(
        ui.ctx()
            .animate_bool(response.id.with("hover"), response.hovered()),
    );
    let card = rect.translate(vec2(0.0, -2.0 * hover));
    let painter = ui.painter();
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 10],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha((70.0 * hover) as u8),
        }
        .as_shape(card, 18.0),
    );
    painter.rect_filled(card, 18.0, alpha(CARD, (214.0 + 24.0 * hover) as u8));
    painter.rect_stroke(
        card,
        18.0,
        Stroke::new(1.0_f32, mix(BORDER, alpha(ACCENT, 150), hover)),
        egui::StrokeKind::Inside,
    );

    let tile = Rect::from_min_size(card.min + vec2(18.0, 18.0), Vec2::splat(44.0));
    painter.rect_filled(tile, 12.0, alpha(ACCENT, (24.0 + 16.0 * hover) as u8));
    painter.rect_stroke(
        tile,
        12.0,
        Stroke::new(1.0_f32, alpha(ACCENT, 60)),
        egui::StrokeKind::Inside,
    );
    paint_icon(
        ui,
        icon,
        Rect::from_center_size(tile.center(), Vec2::splat(20.0)),
        ACCENT,
    );
    paint_painter_badge(
        painter,
        card.right() - 18.0,
        tile.center().y,
        badge.0,
        badge.1,
    );

    painter.text(
        pos2(card.left() + 18.0, tile.bottom() + 14.0),
        Align2::LEFT_TOP,
        title,
        semi(16.5),
        TEXT,
    );
    let galley = painter.layout(
        description.to_owned(),
        regular(12.5),
        MUTED,
        card.width() - 36.0,
    );
    painter.galley(
        pos2(card.left() + 18.0, tile.bottom() + 42.0),
        galley,
        MUTED,
    );

    let action_y = card.bottom() - 22.0;
    let drawn = painter.text(
        pos2(card.left() + 18.0, action_y),
        Align2::LEFT_CENTER,
        action,
        semi(12.5),
        ACCENT,
    );
    paint_icon(
        ui,
        Icon::ArrowRight,
        Rect::from_center_size(
            pos2(drawn.right() + 12.0 + 3.0 * hover, action_y),
            Vec2::splat(14.0),
        ),
        ACCENT,
    );
    response.clicked()
}
