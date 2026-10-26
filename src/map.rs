//! World map geometry (Natural Earth 1:110m), camera and route animations.

use crate::theme::{alpha, ACCENT, ACCENT_HOVER, GREEN, RED};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2};
use std::time::{Duration, Instant};

const WORLD_GEOJSON: &str = include_str!("../assets/ne_110m_admin_0_countries.geojson");

#[derive(Debug, Clone, Copy)]
pub struct GeoPoint {
    pub longitude: f32,
    pub latitude: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutePhase {
    Idle,
    Connecting,
    Connected,
    Failed,
    Disconnecting,
}

pub struct MapCameraAnimation {
    pub from_zoom: f32,
    pub from_pan: Vec2,
    pub to_zoom: f32,
    pub to_pan: Vec2,
    started: Instant,
    duration: Duration,
}

impl MapCameraAnimation {
    pub fn new(from_zoom: f32, from_pan: Vec2, to_zoom: f32, to_pan: Vec2) -> Self {
        Self {
            from_zoom,
            from_pan,
            to_zoom,
            to_pan,
            started: Instant::now(),
            duration: Duration::from_millis(380),
        }
    }

    pub fn sample(&self) -> (f32, Vec2, bool) {
        let progress =
            (self.started.elapsed().as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        let eased = ease_in_out_cubic(progress);
        (
            self.from_zoom + (self.to_zoom - self.from_zoom) * eased,
            self.from_pan + (self.to_pan - self.from_pan) * eased,
            progress >= 1.0,
        )
    }
}

pub struct RouteAnimation {
    pub phase: RoutePhase,
    pub started: Instant,
    pub from: Option<GeoPoint>,
    pub via: Option<GeoPoint>,
    pub to: Option<GeoPoint>,
    break_at: f32,
}

impl RouteAnimation {
    pub fn new() -> Self {
        Self {
            phase: RoutePhase::Idle,
            started: Instant::now(),
            from: None,
            via: None,
            to: None,
            break_at: 0.0,
        }
    }

    pub fn begin(&mut self, from: Option<GeoPoint>, via: Option<GeoPoint>, to: Option<GeoPoint>) {
        self.phase = RoutePhase::Connecting;
        self.started = Instant::now();
        self.from = from;
        self.via = via;
        self.to = to;
        self.break_at = 0.0;
    }

    pub fn connected(&mut self) {
        self.phase = RoutePhase::Connected;
        self.started = Instant::now();
        self.break_at = 1.0;
    }

    pub fn failed(&mut self) {
        self.break_at = self.connecting_progress().clamp(0.24, 0.88);
        self.phase = RoutePhase::Failed;
        self.started = Instant::now();
    }

    pub fn disconnect(&mut self) {
        if self.phase != RoutePhase::Idle {
            self.phase = RoutePhase::Disconnecting;
            self.started = Instant::now();
        }
    }

    fn connecting_progress(&self) -> f32 {
        ease_out_cubic((self.started.elapsed().as_secs_f32() / 1.8).min(0.9))
    }

    /// Advances timed phases; returns whether the animation still needs frames.
    pub fn tick(&mut self) -> bool {
        match self.phase {
            RoutePhase::Failed if self.started.elapsed() > Duration::from_secs_f32(1.7) => {
                self.phase = RoutePhase::Idle;
                false
            }
            RoutePhase::Disconnecting if self.started.elapsed() > Duration::from_secs_f32(1.25) => {
                self.phase = RoutePhase::Idle;
                false
            }
            RoutePhase::Idle => false,
            _ => true,
        }
    }
}

#[derive(Default)]
pub struct WorldMap {
    pub polygons: Vec<MapPolygon>,
}

pub struct MapPolygon {
    coordinates: Vec<[f32; 2]>,
    pub triangles: Vec<u32>,
    rings: Vec<Vec<[f32; 2]>>,
}

pub fn parse_world_map() -> WorldMap {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(WORLD_GEOJSON) else {
        return WorldMap::default();
    };
    let mut world = WorldMap::default();
    let Some(features) = document.get("features").and_then(|value| value.as_array()) else {
        return world;
    };
    for feature in features {
        if feature
            .pointer("/properties/ADMIN")
            .and_then(|value| value.as_str())
            == Some("Antarctica")
        {
            continue;
        }
        let Some(geometry) = feature.get("geometry") else {
            continue;
        };
        let Some(kind) = geometry.get("type").and_then(|value| value.as_str()) else {
            continue;
        };
        let Some(coordinates) = geometry.get("coordinates") else {
            continue;
        };
        match kind {
            "Polygon" => {
                if let Some(polygon) = parse_map_polygon(coordinates) {
                    world.polygons.push(polygon);
                }
            }
            "MultiPolygon" => {
                if let Some(polygons) = coordinates.as_array() {
                    for polygon in polygons {
                        if let Some(polygon) = parse_map_polygon(polygon) {
                            world.polygons.push(polygon);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    world
}

fn parse_map_polygon(value: &serde_json::Value) -> Option<MapPolygon> {
    let source_rings = value.as_array()?;
    let mut flat = Vec::<f64>::new();
    let mut coordinates = Vec::<[f32; 2]>::new();
    let mut holes = Vec::<usize>::new();
    let mut rings = Vec::new();
    for (ring_index, source_ring) in source_rings.iter().enumerate() {
        let values = source_ring.as_array()?;
        if ring_index > 0 {
            holes.push(coordinates.len());
        }
        let mut ring = Vec::new();
        for (point_index, point) in values.iter().enumerate() {
            if point_index + 1 == values.len() && values.first() == values.last() {
                break;
            }
            let pair = point.as_array()?;
            let longitude = pair.first()?.as_f64()?;
            let latitude = pair.get(1)?.as_f64()?;
            coordinates.push([longitude as f32, latitude as f32]);
            ring.push([longitude as f32, latitude as f32]);
            flat.push(longitude);
            flat.push(latitude);
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    let triangles = earcutr::earcut(&flat, &holes, 2)
        .ok()?
        .into_iter()
        .map(|index| index as u32)
        .collect();
    Some(MapPolygon {
        coordinates,
        triangles,
        rings,
    })
}

pub struct MapCamera {
    pub world: Rect,
    pub zoom: f32,
    pub pan: Vec2,
}

impl MapCamera {
    pub fn project(&self, longitude: f32, latitude: f32) -> Pos2 {
        map_view_pos(self.world, longitude, latitude, self.zoom, self.pan)
    }

    pub fn project_point(&self, point: GeoPoint) -> Pos2 {
        self.project(point.longitude, point.latitude)
    }
}

/// Paints graticule, land and borders.
/// Horizontal clip bands that together approximate a rounded rectangle.
/// egui only clips to rectangles; the bands never overlap, so translucent
/// shapes painted once per band are not blended twice.
pub fn rounded_clip_bands(rect: Rect, radius: f32, steps: usize) -> Vec<Rect> {
    let radius = radius.min(rect.height() * 0.5).min(rect.width() * 0.5);
    let step = radius / steps as f32;
    let mut bands = Vec::with_capacity(steps * 2 + 1);
    for index in 0..steps {
        let from_edge = index as f32 * step;
        let dy = radius - from_edge;
        let inset = radius - (radius * radius - dy * dy).max(0.0).sqrt();
        bands.push(Rect::from_min_max(
            pos2(rect.left() + inset, rect.top() + from_edge),
            pos2(rect.right() - inset, rect.top() + from_edge + step),
        ));
        bands.push(Rect::from_min_max(
            pos2(rect.left() + inset, rect.bottom() - from_edge - step),
            pos2(rect.right() - inset, rect.bottom() - from_edge),
        ));
    }
    bands.push(Rect::from_min_max(
        pos2(rect.left(), rect.top() + radius),
        pos2(rect.right(), rect.bottom() - radius),
    ));
    bands
}

/// Paints graticule, land and borders, clipped to `clips`.
pub fn paint_world(painter: &egui::Painter, world: &WorldMap, camera: &MapCamera, clips: &[Rect]) {
    // Every shape keeps its bounds so each band only receives what it touches.
    let mut shapes: Vec<(Rect, Shape)> = Vec::new();
    let grid = Stroke::new(0.6_f32, Color32::from_rgba_unmultiplied(120, 124, 160, 16));
    for longitude in [
        -150.0_f32, -120.0, -90.0, -60.0, -30.0, 0.0, 30.0, 60.0, 90.0, 120.0, 150.0,
    ] {
        let points = (-80..=80)
            .step_by(4)
            .map(|latitude| camera.project(longitude, latitude as f32))
            .collect();
        shapes.extend(line_shape(points, grid));
    }
    for latitude in [-60.0_f32, -30.0, 0.0, 30.0, 60.0] {
        let points = (-180..=180)
            .step_by(5)
            .map(|longitude| camera.project(longitude as f32, latitude))
            .collect();
        shapes.extend(line_shape(points, grid));
    }
    let border = Stroke::new(0.7_f32, Color32::from_rgb(52, 52, 62));
    let mut borders = Vec::new();
    for polygon in &world.polygons {
        for ring in &polygon.rings {
            for segment in projected_ring_segments(ring, camera) {
                borders.push(segment);
            }
        }
    }
    let shadow = Color32::from_rgba_unmultiplied(0, 0, 0, 60);
    let land = Color32::from_rgb(34, 34, 41);
    for (color, offset) in [(shadow, vec2(0.0, 3.0)), (land, Vec2::ZERO)] {
        for polygon in &world.polygons {
            if let Some(mesh) = build_polygon_mesh(polygon, camera, color, offset) {
                let bounds = mesh.calc_bounds();
                shapes.push((bounds, Shape::mesh(mesh)));
            }
        }
    }
    for segment in borders {
        shapes.extend(line_shape(segment, border));
    }

    for clip in clips {
        let band = painter.with_clip_rect(*clip);
        for (bounds, shape) in &shapes {
            if bounds.intersects(*clip) {
                band.add(shape.clone());
            }
        }
    }
}

fn line_shape(points: Vec<Pos2>, stroke: Stroke) -> Option<(Rect, Shape)> {
    (points.len() >= 2).then(|| {
        let bounds = Rect::from_points(&points).expand(stroke.width);
        (bounds, Shape::line(points, stroke))
    })
}

fn build_polygon_mesh(
    polygon: &MapPolygon,
    camera: &MapCamera,
    color: Color32,
    offset: Vec2,
) -> Option<egui::Mesh> {
    let mut mesh = egui::Mesh::default();
    for coordinate in &polygon.coordinates {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: camera.project(coordinate[0], coordinate[1]) + offset,
            uv: Pos2::ZERO,
            color,
        });
    }
    for triangle in polygon.triangles.as_chunks::<3>().0 {
        let points = [
            mesh.vertices[triangle[0] as usize].pos,
            mesh.vertices[triangle[1] as usize].pos,
            mesh.vertices[triangle[2] as usize].pos,
        ];
        // Skip triangles stretched across the antimeridian.
        if Rect::from_points(&points).width() < camera.world.width() * camera.zoom * 0.46 {
            mesh.indices.extend([triangle[0], triangle[1], triangle[2]]);
        }
    }
    (!mesh.indices.is_empty()).then_some(mesh)
}

fn projected_ring_segments(ring: &[[f32; 2]], camera: &MapCamera) -> Vec<Vec<Pos2>> {
    let mut segments = vec![Vec::new()];
    let mut previous = None;
    for coordinate in ring {
        if previous.is_some_and(|longitude: f32| (longitude - coordinate[0]).abs() > 180.0) {
            segments.push(Vec::new());
        }
        segments
            .last_mut()
            .expect("segment exists")
            .push(camera.project(coordinate[0], coordinate[1]));
        previous = Some(coordinate[0]);
    }
    segments
}

const WORLD_ASPECT: f32 = 2.05;

/// World rectangle at zoom 1. It fills most of the card height, so on a narrow
/// card it is wider than the view and can be panned sideways.
pub fn map_world_rect(view: Rect) -> Rect {
    let area = view.shrink2(vec2(10.0, 14.0));
    let contain = fit_rect_aspect(area, WORLD_ASPECT);
    let width = (area.height() * 0.8 * WORLD_ASPECT)
        .min(area.width() * 1.7)
        .max(contain.width());
    Rect::from_center_size(view.center(), vec2(width, width / WORLD_ASPECT))
}

/// Zoom at which the whole world fits into the view.
pub fn min_map_zoom(view: Rect) -> f32 {
    ((view.width() - 20.0) / map_world_rect(view).width()).min(1.0)
}

pub fn map_view_pos(rect: Rect, longitude: f32, latitude: f32, zoom: f32, pan: Vec2) -> Pos2 {
    let base = geo_to_pos(rect, longitude, latitude);
    rect.center() + (base - rect.center()) * zoom + pan
}

pub fn clamp_map_pan(pan: Vec2, view: Rect, zoom: f32) -> Vec2 {
    let world = map_world_rect(view);
    let slack = if zoom > 1.001 {
        vec2(54.0, 42.0)
    } else {
        Vec2::ZERO
    };
    let max_x = ((world.width() * zoom - view.width()) * 0.5).max(0.0) + slack.x;
    let max_y = ((world.height() * zoom - view.height()) * 0.5).max(0.0) + slack.y;
    vec2(pan.x.clamp(-max_x, max_x), pan.y.clamp(-max_y, max_y))
}

pub fn zoom_map_at(zoom: &mut f32, pan: &mut Vec2, view: Rect, pointer: Pos2, factor: f32) {
    let old_zoom = *zoom;
    let new_zoom = (old_zoom * factor).clamp(min_map_zoom(view), 10.0);
    if (new_zoom - old_zoom).abs() <= f32::EPSILON {
        return;
    }
    let ratio = new_zoom / old_zoom;
    *pan = (pointer - view.center()) * (1.0 - ratio) + *pan * ratio;
    *zoom = new_zoom;
    *pan = clamp_map_pan(*pan, view, *zoom);
}

pub fn geo_to_pos(rect: Rect, longitude: f32, latitude: f32) -> Pos2 {
    // Equal Earth keeps the real geography recognizable while fitting the
    // world naturally into a wide card.
    const A1: f32 = 1.340_264;
    const A2: f32 = -0.081_106;
    const A3: f32 = 0.000_893;
    const A4: f32 = 0.003_796;
    const M: f32 = 0.866_025_4;
    const X_MAX: f32 = 2.706_63;
    const Y_MAX: f32 = 1.317_36;
    let lambda = longitude.to_radians();
    let theta = (M * latitude.to_radians().sin()).asin();
    let theta2 = theta * theta;
    let theta6 = theta2 * theta2 * theta2;
    let denominator = M * (A1 + 3.0 * A2 * theta2 + theta6 * (7.0 * A3 + 9.0 * A4 * theta2));
    let projected_x = lambda * theta.cos() / denominator;
    let projected_y = theta * (A1 + A2 * theta2 + theta6 * (A3 + A4 * theta2));
    let x = (projected_x + X_MAX) / (2.0 * X_MAX);
    let y = (Y_MAX - projected_y) / (2.0 * Y_MAX);
    pos2(
        egui::lerp(rect.x_range(), x.clamp(0.0, 1.0)),
        egui::lerp(rect.y_range(), y.clamp(0.0, 1.0)),
    )
}

fn fit_rect_aspect(rect: Rect, aspect: f32) -> Rect {
    let size = if rect.aspect_ratio() > aspect {
        vec2(rect.height() * aspect, rect.height())
    } else {
        vec2(rect.width(), rect.width() / aspect)
    };
    Rect::from_center_size(rect.center(), size)
}

pub fn ease_out_cubic(value: f32) -> f32 {
    1.0 - (1.0 - value.clamp(0.0, 1.0)).powi(3)
}

pub fn ease_in_out_cubic(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value < 0.5 {
        4.0 * value.powi(3)
    } else {
        1.0 - (-2.0 * value + 2.0).powi(3) / 2.0
    }
}

fn route_curve(start: Pos2, end: Pos2, steps: usize) -> Vec<Pos2> {
    let lift = (start.distance(end) * 0.23).clamp(34.0, 138.0);
    let control = start.lerp(end, 0.5) - vec2(0.0, lift);
    (0..=steps)
        .map(|index| {
            let t = index as f32 / steps as f32;
            let one_minus = 1.0 - t;
            pos2(
                one_minus * one_minus * start.x + 2.0 * one_minus * t * control.x + t * t * end.x,
                one_minus * one_minus * start.y + 2.0 * one_minus * t * control.y + t * t * end.y,
            )
        })
        .collect()
}

pub fn paint_route_animation(
    painter: &egui::Painter,
    camera: &MapCamera,
    animation: &RouteAnimation,
) {
    if animation.phase == RoutePhase::Idle {
        return;
    }
    let (Some(from), Some(to)) = (animation.from, animation.to) else {
        return;
    };
    let start = camera.project_point(from);
    let end = camera.project_point(to);
    let via = animation.via.map(|point| camera.project_point(point));
    let (points, split_index) = if let Some(via) = via {
        let mut first_leg = route_curve(start, via, 48);
        let split_index = first_leg.len() - 1;
        first_leg.extend(route_curve(via, end, 48).into_iter().skip(1));
        (first_leg, Some(split_index))
    } else {
        (route_curve(start, end, 84), None)
    };
    let elapsed = animation.started.elapsed().as_secs_f32();
    let progress = match animation.phase {
        RoutePhase::Connecting => animation.connecting_progress(),
        RoutePhase::Connected => 1.0,
        RoutePhase::Failed => animation.break_at,
        RoutePhase::Disconnecting => 1.0 - ease_out_cubic(elapsed / 1.2),
        RoutePhase::Idle => 0.0,
    }
    .clamp(0.0, 1.0);
    let last = ((points.len() - 1) as f32 * progress).round() as usize;
    let visible = &points[..=last.min(points.len() - 1)];
    let failed = animation.phase == RoutePhase::Failed;
    let accent = if failed { RED } else { ACCENT };

    for chunk in points.chunks(6).step_by(2) {
        if chunk.len() >= 2 {
            painter.add(Shape::line(
                chunk.to_vec(),
                Stroke::new(0.8_f32, Color32::from_rgba_unmultiplied(120, 124, 160, 46)),
            ));
        }
    }
    let paint_leg = |leg: &[Pos2], color: Color32| {
        if leg.len() < 2 {
            return;
        }
        painter.add(Shape::line(
            leg.to_vec(),
            Stroke::new(7.0_f32, color.gamma_multiply(0.10)),
        ));
        painter.add(Shape::line(
            leg.to_vec(),
            Stroke::new(2.2_f32, color.gamma_multiply(0.86)),
        ));
        painter.add(Shape::line(
            leg.to_vec(),
            Stroke::new(0.75_f32, Color32::from_white_alpha(170)),
        ));
    };
    if let Some(split) = split_index {
        let first_end = split.min(visible.len().saturating_sub(1));
        paint_leg(
            &visible[..=first_end],
            if failed { RED } else { ACCENT_HOVER },
        );
        if visible.len() > split + 1 {
            paint_leg(&visible[split..], accent);
        }
    } else {
        paint_leg(visible, accent);
    }

    painter.circle_filled(start, 7.0, alpha(ACCENT, 30));
    painter.circle_filled(start, 3.2, ACCENT);
    painter.circle_filled(start, 1.3, Color32::WHITE);
    painter.text(
        start + vec2(0.0, -12.0),
        Align2::CENTER_BOTTOM,
        "вы",
        FontId::proportional(10.0),
        Color32::from_white_alpha(150),
    );

    if let Some(via) = via.filter(|_| progress >= 0.48) {
        let pulse = 8.0 + (elapsed * 3.5).sin().abs() * 3.0;
        painter.circle_filled(via, pulse, alpha(ACCENT, 28));
        painter.circle_stroke(via, 6.0, Stroke::new(1.5_f32, ACCENT_HOVER));
        painter.circle_filled(via, 3.0, ACCENT);
    }

    if animation.phase == RoutePhase::Connected {
        let pulse = 8.0 + (elapsed * 3.0).sin().abs() * 5.0;
        painter.circle_stroke(end, pulse, Stroke::new(1.2_f32, alpha(GREEN, 105)));
        painter.circle_filled(end, 5.0, GREEN);
        let runner = ((elapsed * 0.27) % 1.0 * (points.len() - 1) as f32) as usize;
        let head = points[runner.min(points.len() - 1)];
        for layer in (1..=5).rev() {
            painter.circle_filled(
                head,
                layer as f32 * 2.3,
                alpha(ACCENT_HOVER, 10 + (6 - layer) * 8),
            );
        }
        painter.circle_filled(head, 2.6, Color32::WHITE);
    } else if let Some(&head) = visible.last() {
        painter.circle_filled(head, 10.0, accent.gamma_multiply(0.16));
        painter.circle_filled(head, 4.0, accent);
        painter.circle_filled(head, 1.7, if failed { RED } else { Color32::WHITE });
    }

    if failed {
        let break_point = *visible.last().unwrap_or(&start);
        let scatter = ease_out_cubic(elapsed / 1.25);
        for index in 0..8 {
            let angle = index as f32 * std::f32::consts::TAU / 8.0 + 0.35;
            let direction = vec2(angle.cos(), angle.sin());
            let origin = break_point + direction * (4.0 + index as f32 * 0.4);
            let tip = origin + direction * (7.0 + 18.0 * scatter);
            painter.line_segment(
                [origin, tip],
                Stroke::new(1.4_f32, alpha(RED, ((1.0 - scatter) * 210.0) as u8)),
            );
        }
    }
}

pub fn paint_hop_badge(painter: &egui::Painter, center: Pos2, label: &str, color: Color32) {
    painter.circle_filled(center, 7.5, Color32::from_rgb(17, 17, 20));
    painter.circle_stroke(center, 7.5, Stroke::new(1.3_f32, color));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(9.0),
        Color32::WHITE,
    );
}

pub fn haversine_km(from: GeoPoint, to: GeoPoint) -> f32 {
    let earth_radius_km = 6_371.0_f32;
    let latitude_delta = (to.latitude - from.latitude).to_radians();
    let longitude_delta = (to.longitude - from.longitude).to_radians();
    let from_latitude = from.latitude.to_radians();
    let to_latitude = to.latitude.to_radians();
    let half_latitude = (latitude_delta * 0.5).sin();
    let half_longitude = (longitude_delta * 0.5).sin();
    let a = half_latitude * half_latitude
        + from_latitude.cos() * to_latitude.cos() * half_longitude * half_longitude;
    earth_radius_km * 2.0 * a.sqrt().asin()
}

pub fn country_coordinates(country: &str) -> Option<(f32, f32)> {
    let n = country.to_lowercase();
    let point = if n.contains("нидерланд") || n.contains("netherlands") {
        (5.3, 52.1)
    } else if n.contains("герман") || n.contains("germany") {
        (10.4, 51.1)
    } else if n.contains("соединенные штаты")
        || n.contains("соединённые штаты")
        || n.contains("united states")
        || n == "сша"
        || n == "usa"
    {
        (-101.0, 39.0)
    } else if n.contains("финлянд") || n.contains("finland") {
        (25.7, 64.5)
    } else if n.contains("швейцар") || n.contains("switzerland") {
        (8.2, 46.8)
    } else if n.contains("эстон") || n.contains("estonia") {
        (25.5, 58.6)
    } else if n.contains("швец") || n.contains("sweden") {
        (15.0, 62.0)
    } else if n.contains("великобрит") || n.contains("united kingdom") {
        (-3.2, 55.0)
    } else if n.contains("казахстан") || n.contains("kazakhstan") {
        (67.0, 48.0)
    } else if n.contains("турц") || n.contains("turkey") || n.contains("türkiye") {
        (35.2, 39.0)
    } else if n.contains("гонконг") || n.contains("hong kong") {
        (114.2, 22.3)
    } else if n.contains("латви") || n.contains("latvia") {
        (24.6, 57.0)
    } else if n.contains("словаки") || n.contains("slovakia") {
        (19.7, 48.7)
    } else if n.contains("сингапур") || n.contains("singapore") {
        (103.8, 1.3)
    } else if n.contains("польш") || n.contains("poland") {
        (19.1, 52.1)
    } else if n.contains("япони") || n.contains("japan") {
        (138.2, 36.2)
    } else if n.contains("чех") || n.contains("czech") {
        (15.5, 49.8)
    } else if n.contains("коре") || n.contains("korea") {
        (127.8, 36.4)
    } else if n.contains("малайзи") || n.contains("malaysia") {
        (102.0, 4.2)
    } else if n.contains("тайван") || n.contains("taiwan") {
        (121.0, 23.7)
    } else if n.contains("австри") || n.contains("austria") {
        (14.4, 47.5)
    } else if n.contains("норвеги") || n.contains("norway") {
        (8.5, 61.4)
    } else if n.contains("росси") || n.contains("russia") {
        (84.0, 61.0)
    } else if n.contains("франц") || n.contains("france") {
        (2.2, 46.2)
    } else if n.contains("испани") || n.contains("spain") {
        (-3.7, 40.4)
    } else if n.contains("итали") || n.contains("italy") {
        (12.6, 42.8)
    } else if n.contains("румын") || n.contains("romania") {
        (24.9, 45.9)
    } else if n.contains("болгар") || n.contains("bulgaria") {
        (25.5, 42.7)
    } else if n.contains("украин") || n.contains("ukraine") {
        (31.2, 49.0)
    } else if n.contains("молдов") || n.contains("moldova") {
        (28.4, 47.0)
    } else if n.contains("индонези") || n.contains("indonesia") {
        (118.0, -2.5)
    } else if n.contains("инд") || n.contains("india") {
        (78.9, 22.7)
    } else if n.contains("китай") || n.contains("china") {
        (104.2, 35.8)
    } else if n.contains("австрали") || n.contains("australia") {
        (134.5, -25.7)
    } else if n.contains("канад") || n.contains("canada") {
        (-106.0, 57.0)
    } else if n.contains("бразил") || n.contains("brazil") {
        (-51.9, -14.2)
    } else if n.contains("аргентин") || n.contains("argentina") {
        (-64.0, -34.0)
    } else if n.contains("мексик") || n.contains("mexico") {
        (-102.5, 23.6)
    } else if n.contains("оаэ") || n.contains("emirates") {
        (53.8, 23.4)
    } else if n.contains("израил") || n.contains("israel") {
        (35.0, 31.5)
    } else if n.contains("таиланд") || n.contains("thailand") {
        (100.9, 15.8)
    } else if n.contains("вьетнам") || n.contains("vietnam") {
        (108.0, 15.8)
    } else if n.contains("дания") || n.contains("denmark") {
        (9.5, 56.1)
    } else if n.contains("бельги") || n.contains("belgium") {
        (4.7, 50.7)
    } else if n.contains("португал") || n.contains("portugal") {
        (-8.2, 39.6)
    } else if n.contains("ирлан") || n.contains("ireland") {
        (-8.0, 53.2)
    } else if n.contains("литв") || n.contains("lithuania") {
        (23.9, 55.2)
    } else if n.contains("беларус") || n.contains("belarus") {
        (28.0, 53.7)
    } else if n.contains("грузи") || n.contains("georgia") {
        (43.4, 42.1)
    } else if n.contains("армени") || n.contains("armenia") {
        (45.0, 40.2)
    } else if n.contains("сербия") || n.contains("serbia") {
        (20.9, 44.0)
    } else if n.contains("венгр") || n.contains("hungary") {
        (19.5, 47.2)
    } else if n.contains("греци") || n.contains("greece") {
        (21.8, 39.1)
    } else if n.contains("кипр") || n.contains("cyprus") {
        (33.4, 35.1)
    } else if n.contains("исланд") || n.contains("iceland") {
        (-19.0, 64.9)
    } else if n.contains("люксембург") || n.contains("luxembourg") {
        (6.1, 49.8)
    } else if n.contains("хорват") || n.contains("croatia") {
        (15.2, 45.1)
    } else if n.contains("словени") || n.contains("slovenia") {
        (14.9, 46.1)
    } else if n.contains("узбек") || n.contains("uzbekistan") {
        (64.6, 41.4)
    } else if n.contains("азербайджан") || n.contains("azerbaijan") {
        (47.6, 40.1)
    } else if n.contains("южная африка") || n.contains("south africa") || n.contains("юар")
    {
        (24.0, -29.0)
    } else if n.contains("чили") || n.contains("chile") {
        (-71.5, -35.7)
    } else if n.contains("филиппин") || n.contains("philippines") {
        (122.0, 12.9)
    } else if n.contains("новая зеланд") || n.contains("new zealand") {
        (172.5, -41.5)
    } else {
        return None;
    };
    Some(point)
}
