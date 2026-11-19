use eframe::egui::{
    self, Align2, Color32, FontFamily, FontId, Mesh, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2,
};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(11, 10, 20);
pub const SIDEBAR: Color32 = Color32::from_rgb(14, 13, 26);
pub const CARD: Color32 = Color32::from_rgb(20, 18, 35);
pub const CARD2: Color32 = Color32::from_rgb(29, 26, 49);
pub const ACCENT: Color32 = Color32::from_rgb(139, 92, 246);
pub const ACCENT2: Color32 = Color32::from_rgb(167, 139, 250);
pub const NAV_ACTIVE: Color32 = Color32::from_rgb(58, 46, 108);
pub const TEXT: Color32 = Color32::from_rgb(243, 242, 251);
pub const MUTED: Color32 = Color32::from_rgb(150, 147, 181);
pub const GREEN: Color32 = Color32::from_rgb(52, 211, 153);
pub const RED: Color32 = Color32::from_rgb(248, 113, 113);
pub const AMBER: Color32 = Color32::from_rgb(251, 191, 36);

const PALETTE: [Color32; 8] = [
    Color32::from_rgb(139, 92, 246),
    Color32::from_rgb(96, 140, 255),
    Color32::from_rgb(196, 140, 252),
    Color32::from_rgb(45, 212, 191),
    Color32::from_rgb(244, 114, 182),
    Color32::from_rgb(251, 191, 36),
    Color32::from_rgb(56, 189, 248),
    Color32::from_rgb(163, 230, 53),
];

pub fn slot_color(slot: usize) -> Color32 {
    PALETTE[slot % PALETTE.len()]
}

pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}
pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

fn fmt_num(v: f64) -> String {
    if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

pub fn split_bytes(b: f64) -> (String, &'static str) {
    if b >= 1e12 {
        (fmt_num(b / 1e12), "TB")
    } else if b >= 1e9 {
        (fmt_num(b / 1e9), "GB")
    } else if b >= 1e6 {
        (fmt_num(b / 1e6), "MB")
    } else if b >= 1e3 {
        (fmt_num(b / 1e3), "KB")
    } else {
        (format!("{b:.0}"), "B")
    }
}

pub fn fmt_bytes(b: u64) -> String {
    let (n, u) = split_bytes(b as f64);
    format!("{n} {u}")
}

pub fn fmt_speed(b: f64) -> String {
    let (n, u) = split_bytes(b);
    format!("{n} {u}/s")
}

pub fn fmt_eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} sa {} dk", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} dk {} sn", secs / 60, secs % 60)
    } else {
        format!("{secs} sn")
    }
}

pub fn card(p: &egui::Painter, rect: Rect, radius: f32) {
    p.rect_filled(rect, radius, CARD);
    p.rect_stroke(rect, radius, Stroke::new(1.0, alpha(Color32::WHITE, 14)), StrokeKind::Inside);
}

pub fn ellipsize(p: &egui::Painter, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<egui::Galley> {
    let mut s = text.to_string();
    let mut g = p.layout_no_wrap(s.clone(), font.clone(), color);
    if g.size().x <= max_w {
        return g;
    }
    while s.chars().count() > 1 {
        s.pop();
        g = p.layout_no_wrap(format!("{}…", s.trim_end()), font.clone(), color);
        if g.size().x <= max_w {
            break;
        }
    }
    g
}

pub fn text_vc(p: &egui::Painter, pos: Pos2, s: &str, font: FontId, color: Color32) -> Rect {
    p.text(pos, Align2::LEFT_CENTER, s, font, color)
}

pub fn text_vc_right(p: &egui::Painter, pos: Pos2, s: &str, font: FontId, color: Color32) -> Rect {
    p.text(pos, Align2::RIGHT_CENTER, s, font, color)
}

pub fn gradient_bar(p: &egui::Painter, rect: Rect, c1: Color32, c2: Color32) {
    let r = rect.height() / 2.0;
    if rect.width() < rect.height() {
        p.rect_filled(rect, r, c1);
        return;
    }
    p.circle_filled(pos2(rect.left() + r, rect.center().y), r, c1);
    p.circle_filled(pos2(rect.right() - r, rect.center().y), r, c2);
    let mut m = Mesh::default();
    let (x0, x1) = (rect.left() + r, rect.right() - r);
    m.colored_vertex(pos2(x0, rect.top()), c1);
    m.colored_vertex(pos2(x1, rect.top()), c2);
    m.colored_vertex(pos2(x1, rect.bottom()), c2);
    m.colored_vertex(pos2(x0, rect.bottom()), c1);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

fn poly(p: &egui::Painter, c: Pos2, s: f32, pts: &[(f32, f32)], stroke: Stroke, closed: bool) {
    let mut v: Vec<Pos2> = pts.iter().map(|(x, y)| c + vec2(x * s, y * s)).collect();
    if closed {
        v.push(v[0]);
    }
    p.add(Shape::line(v, stroke));
}

pub fn draw_bolt(p: &egui::Painter, rect: Rect, c1: Color32, c2: Color32) {
    let pt = |x: f32, y: f32| pos2(rect.left() + rect.width() * x, rect.top() + rect.height() * y);
    let a = pt(0.66, 0.02);
    let b = pt(0.14, 0.58);
    let c = pt(0.44, 0.58);
    let d = pt(0.32, 0.98);
    let e = pt(0.88, 0.38);
    let f = pt(0.58, 0.38);
    let mut m = Mesh::default();
    for (q, col) in [(a, c1), (b, c1), (c, mix(c1, c2, 0.5)), (f, c1), (e, c2), (d, c2)] {
        m.colored_vertex(q, col);
    }
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    m.add_triangle(3, 4, 5);
    m.add_triangle(3, 5, 2);
    p.add(Shape::mesh(m));
}

pub fn paint_waves(p: &egui::Painter, rect: Rect, time: f32) {
    let ribbons = [
        (0.55f32, 0.9f32, 0.0f32, ACCENT, 70u8),
        (0.40, 1.3, 1.7, ACCENT2, 50),
        (0.30, 0.7, 3.1, Color32::from_rgb(110, 70, 230), 60),
    ];
    for (amp, freq, ph, col, a) in ribbons {
        let n = 28;
        let mut m = Mesh::default();
        let mut line = Vec::new();
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let x = rect.left() + rect.width() * t;
            let base = rect.bottom() - rect.height() * (0.10 + 0.55 * t.powf(1.6));
            let y = base - rect.height() * 0.12 * amp * (t * freq * 5.0 + ph + time * 0.25).sin();
            m.colored_vertex(pos2(x, y), alpha(col, a));
            m.colored_vertex(pos2(x, rect.bottom()), alpha(col, 0));
            line.push(pos2(x, y));
        }
        for i in 0..n {
            let k = (i * 2) as u32;
            m.add_triangle(k, k + 1, k + 2);
            m.add_triangle(k + 1, k + 3, k + 2);
        }
        let pp = p.with_clip_rect(rect);
        pp.add(Shape::mesh(m));
        pp.add(Shape::line(line, Stroke::new(1.4, alpha(mix(col, Color32::WHITE, 0.3), a.saturating_mul(3)))));
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Ethernet,
    Wifi,
    Usb,
    Globe,
    Play,
    Pause,
    Folder,
    File,
    Close,
    Check,
    Gear,
    Download,
    Layers,
    Info,
    Plus,
    Trash,
    Dots,
    Clock,
    Drive,
    Network,
    Minimize,
    Maximize,
    Restore,
}

pub fn draw_icon(p: &egui::Painter, c: Pos2, s: f32, icon: Icon, color: Color32) {
    let w = (s * 0.075).max(1.5);
    let st = Stroke::new(w, color);
    match icon {
        Icon::Play => {
            poly(p, c, s, &[(-0.2, -0.3), (0.32, 0.0), (-0.2, 0.3)], st, true);
        }
        Icon::Pause => {
            for dx in [-0.16, 0.16] {
                p.line_segment([c + vec2(s * dx, -s * 0.28), c + vec2(s * dx, s * 0.28)], Stroke::new(w * 1.5, color));
            }
        }
        Icon::Close => {
            let d = s * 0.24;
            p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], st);
            p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], st);
        }
        Icon::Check => {
            poly(p, c, s, &[(-0.28, 0.02), (-0.08, 0.22), (0.3, -0.22)], Stroke::new(w * 1.2, color), false);
        }
        Icon::Plus => {
            p.line_segment([c + vec2(-s * 0.26, 0.0), c + vec2(s * 0.26, 0.0)], Stroke::new(w * 1.2, color));
            p.line_segment([c + vec2(0.0, -s * 0.26), c + vec2(0.0, s * 0.26)], Stroke::new(w * 1.2, color));
        }
        Icon::Minimize => {
            p.line_segment([c + vec2(-s * 0.28, 0.0), c + vec2(s * 0.28, 0.0)], st);
        }
        Icon::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, vec2(s * 0.52, s * 0.52)), 1.5, st, StrokeKind::Middle);
        }
        Icon::Restore => {
            p.rect_stroke(Rect::from_center_size(c + vec2(-s * 0.04, s * 0.05), vec2(s * 0.44, s * 0.44)), 1.5, st, StrokeKind::Middle);
            poly(p, c, s, &[(0.0, -0.2), (0.0, -0.3), (0.3, -0.3), (0.3, 0.0), (0.2, 0.0)], st, false);
        }
        Icon::Dots => {
            for dx in [-0.26, 0.0, 0.26] {
                p.circle_filled(c + vec2(s * dx, 0.0), s * 0.065, color);
            }
        }
        Icon::Trash => {
            p.line_segment([c + vec2(-s * 0.32, -s * 0.18), c + vec2(s * 0.32, -s * 0.18)], st);
            poly(p, c, s, &[(-0.1, -0.18), (-0.1, -0.3), (0.1, -0.3), (0.1, -0.18)], st, false);
            poly(p, c, s, &[(-0.22, -0.18), (-0.18, 0.34), (0.18, 0.34), (0.22, -0.18)], st, false);
            p.line_segment([c + vec2(-s * 0.06, -s * 0.06), c + vec2(-s * 0.06, s * 0.22)], Stroke::new(w * 0.8, color));
            p.line_segment([c + vec2(s * 0.06, -s * 0.06), c + vec2(s * 0.06, s * 0.22)], Stroke::new(w * 0.8, color));
        }
        Icon::Folder => {
            poly(p, c, s, &[(-0.38, -0.22), (-0.14, -0.22), (-0.06, -0.12), (0.38, -0.12), (0.38, 0.28), (-0.38, 0.28)], st, true);
        }
        Icon::File => {
            poly(p, c, s, &[(-0.26, -0.38), (0.08, -0.38), (0.28, -0.18), (0.28, 0.38), (-0.26, 0.38)], st, true);
            poly(p, c, s, &[(0.08, -0.38), (0.08, -0.18), (0.28, -0.18)], st, false);
            for (i, y) in [-0.1f32, 0.02, 0.14].iter().enumerate() {
                let x = if i % 2 == 0 { -0.02 } else { 0.06 };
                p.line_segment([c + vec2(s * x - s * 0.05, s * y), c + vec2(s * x + s * 0.05, s * y)], Stroke::new(w * 0.9, color));
            }
        }
        Icon::Gear => {
            p.circle_stroke(c, s * 0.15, st);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * (s * 0.3), c + d * (s * 0.4)], Stroke::new(w * 1.5, color));
            }
            p.circle_stroke(c, s * 0.3, Stroke::new(w * 0.9, color));
        }
        Icon::Info => {
            p.circle_stroke(c, s * 0.36, st);
            p.circle_filled(c + vec2(0.0, -s * 0.15), w * 0.75, color);
            p.line_segment([c + vec2(0.0, -s * 0.04), c + vec2(0.0, s * 0.18)], st);
        }
        Icon::Download => {
            p.line_segment([c + vec2(0.0, -s * 0.34), c + vec2(0.0, s * 0.14)], st);
            poly(p, c, s, &[(-0.2, -0.06), (0.0, 0.16), (0.2, -0.06)], st, false);
            poly(p, c, s, &[(-0.34, 0.1), (-0.34, 0.34), (0.34, 0.34), (0.34, 0.1)], st, false);
        }
        Icon::Layers => {
            poly(p, c, s, &[(-0.4, -0.2), (0.0, -0.42), (0.4, -0.2), (0.0, 0.02)], st, true);
            poly(p, c, s, &[(-0.4, 0.0), (0.0, 0.22), (0.4, 0.0)], st, false);
            poly(p, c, s, &[(-0.4, 0.18), (0.0, 0.4), (0.4, 0.18)], st, false);
        }
        Icon::Clock => {
            p.circle_stroke(c, s * 0.36, st);
            poly(p, c, s, &[(0.0, -0.2), (0.0, 0.0), (0.14, 0.1)], st, false);
        }
        Icon::Drive => {
            let r = Rect::from_center_size(c + vec2(0.0, s * 0.08), vec2(s * 0.8, s * 0.4));
            p.rect_stroke(r, 4.0, st, StrokeKind::Middle);
            p.circle_filled(pos2(r.right() - s * 0.14, r.center().y), w * 0.8, color);
            poly(p, c, s, &[(-0.3, -0.12), (-0.18, -0.34), (0.18, -0.34), (0.3, -0.12)], st, false);
        }
        Icon::Network => {
            let top = c + vec2(0.0, -s * 0.24);
            let l = c + vec2(-s * 0.28, s * 0.24);
            let r = c + vec2(s * 0.28, s * 0.24);
            for (a, b) in [(top, l), (top, r), (l, r)] {
                p.line_segment([a, b], Stroke::new(w * 0.8, color));
            }
            for q in [top, l, r] {
                p.circle_filled(q, s * 0.09, CARD);
                p.circle_stroke(q, s * 0.09, st);
            }
        }
        Icon::Ethernet => {
            let r = Rect::from_center_size(c + vec2(0.0, -s * 0.04), vec2(s * 0.7, s * 0.46));
            p.rect_stroke(r, 3.0, st, StrokeKind::Middle);
            for i in 0..4 {
                let x = r.left() + r.width() * (0.2 + 0.2 * i as f32);
                p.line_segment([pos2(x, r.top() + 4.0), pos2(x, r.top() + r.height() * 0.5)], Stroke::new(w * 0.8, color));
            }
            p.line_segment([pos2(c.x - s * 0.12, r.bottom()), pos2(c.x - s * 0.12, r.bottom() + s * 0.14)], st);
            p.line_segment([pos2(c.x + s * 0.12, r.bottom()), pos2(c.x + s * 0.12, r.bottom() + s * 0.14)], st);
        }
        Icon::Wifi => {
            let base = c + vec2(0.0, s * 0.28);
            for r in [0.24f32, 0.44, 0.64] {
                let mut pts = Vec::new();
                for k in 0..=14 {
                    let a = -std::f32::consts::FRAC_PI_2 + (k as f32 / 14.0 - 0.5) * 1.55;
                    pts.push(base + vec2(a.cos(), a.sin()) * s * r);
                }
                p.add(Shape::line(pts, st));
            }
            p.circle_filled(base, w * 0.9, color);
        }
        Icon::Usb => {
            p.line_segment([c + vec2(0.0, s * 0.3), c + vec2(0.0, -s * 0.34)], st);
            poly(p, c, s, &[(-0.1, -0.24), (0.0, -0.4), (0.1, -0.24)], st, true);
            p.circle_stroke(c + vec2(0.0, s * 0.32), s * 0.08, st);
            poly(p, c, s, &[(0.0, 0.04), (0.24, -0.1), (0.24, -0.16)], st, false);
            p.rect_stroke(Rect::from_center_size(c + vec2(s * 0.24, -s * 0.2), vec2(s * 0.12, s * 0.1)), 0.0, st, StrokeKind::Middle);
            poly(p, c, s, &[(0.0, 0.16), (-0.22, 0.02), (-0.22, -0.04)], st, false);
            p.circle_stroke(c + vec2(-s * 0.22, -s * 0.1), s * 0.06, st);
        }
        Icon::Globe => {
            p.circle_stroke(c, s * 0.36, st);
            p.line_segment([c + vec2(-s * 0.36, 0.0), c + vec2(s * 0.36, 0.0)], Stroke::new(w * 0.8, color));
            let mut pts = Vec::new();
            for k in 0..=16 {
                let a = k as f32 / 16.0 * std::f32::consts::TAU;
                pts.push(c + vec2(a.cos() * s * 0.15, a.sin() * s * 0.36));
            }
            p.add(Shape::line(pts, Stroke::new(w * 0.8, color)));
        }
    }
}
