#![allow(deprecated)]
mod theme;

use crate::engine::{Engine, JobAction, JobView, LinkView, Settings, View, job::Status, link::LinkKind};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, Id, Key, Margin, Modifiers, Rect, RichText, Sense, Stroke,
    StrokeKind, UiBuilder, ViewportCommand, pos2, vec2,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};
use theme::*;

const SIDEBAR_W: f32 = 236.0;
const HEADER_H: f32 = 92.0;
const PAD: f32 = 28.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Downloads,
    Queue,
    Done,
    Settings,
    About,
}

pub struct App {
    eng: Arc<Engine>,
    view: View,
    last_view: Instant,
    t0: Instant,
    page: Page,
    selected: Option<u64>,
    new_dialog: bool,
    input: String,
    link_dialog: Option<String>,
    menu_open: bool,
    units_gb: HashMap<String, bool>,
    show_virtual: bool,
    folder_rx: Option<Receiver<String>>,
    toast: Option<(String, Instant)>,
    #[cfg(debug_assertions)]
    dev_added: bool,
    #[cfg(debug_assertions)]
    dev_shot_requested: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, eng: Arc<Engine>) -> Self {
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);
        let view = eng.view();
        let page = match crate::dev::env("LETSDOWNLOAD_PAGE").as_deref() {
            Some("queue") => Page::Queue,
            Some("done") => Page::Done,
            Some("settings") => Page::Settings,
            Some("about") => Page::About,
            _ => Page::Downloads,
        };
        let open = crate::dev::env("LETSDOWNLOAD_OPEN").unwrap_or_default();
        App {
            eng,
            view,
            last_view: Instant::now(),
            t0: Instant::now(),
            page,
            selected: None,
            new_dialog: open == "new",
            input: String::new(),
            link_dialog: if open == "link" { Some("fake-a".into()) } else { None },
            menu_open: open == "menu",
            units_gb: HashMap::new(),
            show_virtual: false,
            folder_rx: None,
            toast: None,
            #[cfg(debug_assertions)]
            dev_added: false,
            #[cfg(debug_assertions)]
            dev_shot_requested: false,
        }
    }

    #[cfg(debug_assertions)]
    fn dev_hooks(&mut self, ctx: &egui::Context) {
        let t = self.t0.elapsed().as_secs_f32();
        if !self.dev_added {
            self.dev_added = true;
            if let Some(urls) = crate::dev::env("LETSDOWNLOAD_AUTOADD") {
                self.eng.add_urls(&urls);
            }
        }
        let Some(path) = crate::dev::env("LETSDOWNLOAD_SHOT") else { return };
        let at: f32 = crate::dev::env("LETSDOWNLOAD_SHOT_AT").and_then(|v| v.parse().ok()).unwrap_or(7.0);
        if t > at && !self.dev_shot_requested {
            self.dev_shot_requested = true;
            let cb = egui::ScreenshotCallback::new(move |img: Arc<egui::ColorImage>| {
                let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                let _ = image::save_buffer(&path, &rgba, img.size[0] as u32, img.size[1] as u32, image::ColorType::Rgba8);
                std::process::exit(0);
            });
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(cb));
        }
    }
}

fn setup_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    let mut fonts = FontDefinitions::default();
    let load = |fonts: &mut FontDefinitions, name: &str, file: &str| -> bool {
        match std::fs::read(format!("C:\\Windows\\Fonts\\{file}")) {
            Ok(b) => {
                fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_owned(b)));
                true
            }
            Err(_) => false,
        }
    };
    let regular = load(&mut fonts, "segoe", "segoeui.ttf");
    let bold = load(&mut fonts, "segoe-bold", "seguisb.ttf") || load(&mut fonts, "segoe-bold", "segoeuib.ttf");
    let fallback: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
    if regular {
        fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "segoe".into());
    }
    let mut bold_list = fallback.clone();
    if bold {
        bold_list.insert(0, "segoe-bold".into());
    } else if regular {
        bold_list.insert(0, "segoe".into());
    }
    fonts.families.insert(FontFamily::Name("bold".into()), bold_list);
    ctx.set_fonts(fonts);
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = Color32::from_rgb(22, 20, 40);
    v.window_stroke = Stroke::new(1.0, alpha(Color32::WHITE, 26));
    v.window_corner_radius = egui::CornerRadius::same(16);
    v.extreme_bg_color = Color32::from_rgb(12, 11, 24);
    v.faint_bg_color = Color32::from_rgb(24, 22, 42);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = alpha(ACCENT, 110);
    v.selection.stroke = Stroke::new(1.0, ACCENT2);
    let r = egui::CornerRadius::same(9);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.noninteractive,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.inactive.bg_fill = CARD2;
    v.widgets.inactive.weak_bg_fill = CARD2;
    v.widgets.hovered.bg_fill = Color32::from_rgb(42, 38, 70);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(42, 38, 70);
    v.widgets.active.bg_fill = Color32::from_rgb(56, 50, 92);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(56, 50, 92);
    ctx.set_visuals(v);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(debug_assertions)]
        self.dev_hooks(&ctx);
        if self.last_view.elapsed() > Duration::from_millis(60) {
            self.view = self.eng.view();
            self.last_view = Instant::now();
        }
        let view = self.view.clone();
        let time = self.t0.elapsed().as_secs_f32();

        if let Some(rx) = &self.folder_rx {
            match rx.try_recv() {
                Ok(path) => {
                    let mut s = self.view.settings.clone();
                    s.download_dir = path;
                    self.apply_settings(s);
                    self.folder_rx = None;
                }
                Err(TryRecvError::Disconnected) => self.folder_rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }

        let full = ui.max_rect();
        paint_background(ui.painter(), full);
        let side = Rect::from_min_max(full.min, pos2(full.left() + SIDEBAR_W, full.bottom()));
        let main = Rect::from_min_max(pos2(side.right(), full.top()), full.max);

        self.sidebar(ui, side, &view, time);
        self.header(ui, main, &view);

        let content = Rect::from_min_max(
            pos2(main.left() + PAD, main.top() + HEADER_H),
            pos2(main.right() - PAD + 10.0, main.bottom()),
        );
        let mut child = ui.new_child(UiBuilder::new().max_rect(content));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
            ui.set_width(content.width() - 10.0);
            ui.spacing_mut().item_spacing.y = 16.0;
            let cur = current_job(&view, self.selected).map(|j| j.id);
            match self.page {
                Page::Downloads => {
                    let jobs: Vec<&JobView> = view.jobs.iter().rev().filter(|j| is_running(j)).collect();
                    if jobs.is_empty() {
                        empty_card(ui, "Aktif indirme yok");
                    }
                    for j in jobs {
                        self.job_card(ui, j, cur);
                    }
                    let curj = current_job(&view, self.selected);
                    self.links_panel(ui, &view, curj);
                    self.chunks_panel(ui, &view, curj, time);
                }
                Page::Queue => {
                    let jobs: Vec<&JobView> = view.jobs.iter().rev().filter(|j| matches!(j.status, Status::Paused | Status::Error)).collect();
                    if jobs.is_empty() {
                        empty_card(ui, "Kuyruk boş");
                    }
                    for j in jobs {
                        self.job_card(ui, j, cur);
                    }
                }
                Page::Done => {
                    let jobs: Vec<&JobView> = view.jobs.iter().rev().filter(|j| j.status == Status::Completed).collect();
                    if jobs.is_empty() {
                        empty_card(ui, "Tamamlanan indirme yok");
                    }
                    for j in jobs {
                        self.job_card(ui, j, cur);
                    }
                }
                Page::Settings => self.settings_page(ui),
                Page::About => self.about_page(ui),
            }
            ui.add_space(24.0);
        });

        self.dialogs(&ctx, &view, main);
        self.window_resize(&ctx, full);
        self.draw_toast(&ctx);

        let busy = view.total_rate > 0.0 || view.jobs.iter().any(|j| matches!(j.status, Status::Downloading | Status::Probing));
        ctx.request_repaint_after(Duration::from_millis(if busy { 16 } else { 100 }));
    }

    fn on_exit(&mut self) {
        self.eng.shutdown();
    }
}

fn is_running(j: &JobView) -> bool {
    matches!(j.status, Status::Downloading | Status::Probing | Status::Waiting)
}

fn current_job(view: &View, selected: Option<u64>) -> Option<&JobView> {
    selected
        .and_then(|id| view.jobs.iter().find(|j| j.id == id))
        .or_else(|| view.jobs.iter().rev().find(|j| is_running(j)))
        .or_else(|| view.jobs.last())
}

fn paint_background(p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 0.0, BG);
    for (c, r, a) in [
        (pos2(rect.right() - 80.0, rect.top() + 40.0), 520.0, 4u8),
        (pos2(rect.left() + SIDEBAR_W + 100.0, rect.bottom() - 40.0), 420.0, 3),
    ] {
        for i in 0..14 {
            let k = i as f32 / 14.0;
            p.circle_filled(c, r * (1.0 - k * 0.92), alpha(ACCENT, a));
        }
    }
}

fn empty_card(ui: &mut egui::Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), Sense::hover());
    let p = ui.painter();
    card(p, rect, 16.0);
    draw_icon(p, pos2(rect.center().x, rect.top() + 34.0), 30.0, Icon::Download, alpha(MUTED, 170));
    p.text(pos2(rect.center().x, rect.top() + 66.0), Align2::CENTER_CENTER, title, bold(16.0), MUTED);
}

impl App {
    fn sidebar(&mut self, ui: &mut egui::Ui, rect: Rect, view: &View, time: f32) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        p.rect_filled(rect, 0.0, SIDEBAR);
        p.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, alpha(Color32::WHITE, 14)));
        paint_waves(&p, Rect::from_min_max(pos2(rect.left(), rect.bottom() - 280.0), pos2(rect.right() - 1.0, rect.bottom())), time);

        let drag = Rect::from_min_size(rect.left_top(), vec2(rect.width(), 84.0));
        let r = ui.interact(drag, Id::new("side-drag"), Sense::click_and_drag());
        if r.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        let logo = Rect::from_min_size(rect.left_top() + vec2(26.0, 26.0), vec2(26.0, 32.0));
        for i in 0..6 {
            p.circle_filled(logo.center(), 30.0 - i as f32 * 4.0, alpha(ACCENT, 5));
        }
        draw_bolt(&p, logo, ACCENT2, ACCENT);
        text_vc(&p, pos2(rect.left() + 66.0, rect.top() + 42.0), "LetsDownload", bold(21.0), TEXT);

        let running = view.jobs.iter().filter(|j| is_running(j)).count();
        let queued = view.jobs.iter().filter(|j| matches!(j.status, Status::Paused | Status::Error)).count();
        let items = [
            (Page::Downloads, Icon::Download, "İndirmeler", running),
            (Page::Queue, Icon::Layers, "Kuyruk", queued),
            (Page::Done, Icon::Check, "Tamamlanan", 0),
            (Page::Settings, Icon::Gear, "Ayarlar", 0),
            (Page::About, Icon::Info, "Hakkında", 0),
        ];
        let mut y = rect.top() + 98.0;
        for (i, (page, icon, label, count)) in items.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(rect.left() + 14.0, y), vec2(rect.width() - 28.0, 46.0));
            let resp = ui.interact(r, Id::new(("nav", i)), Sense::click());
            let active = self.page == page;
            if active {
                p.rect_filled(r, 12.0, NAV_ACTIVE);
            } else if resp.hovered() {
                p.rect_filled(r, 12.0, alpha(Color32::WHITE, 8));
            }
            let col = if active { TEXT } else { alpha(TEXT, 190) };
            draw_icon(&p, pos2(r.left() + 28.0, r.center().y), 24.0, icon, col);
            text_vc(&p, pos2(r.left() + 54.0, r.center().y), label, if active { bold(15.0) } else { regular(15.0) }, col);
            if count > 0 {
                let b = Rect::from_center_size(pos2(r.right() - 24.0, r.center().y), vec2(26.0, 24.0));
                p.rect_filled(b, 8.0, if active { ACCENT } else { alpha(ACCENT, 120) });
                p.text(b.center(), Align2::CENTER_CENTER, count.to_string(), bold(12.5), Color32::WHITE);
            }
            if resp.clicked() {
                self.page = page;
            }
            y += 52.0;
        }

    }
}

impl App {
    fn header(&mut self, ui: &mut egui::Ui, main: Rect, view: &View) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

        let drag = Rect::from_min_size(main.left_top(), vec2(main.width(), HEADER_H));
        let r = ui.interact(drag, Id::new("hdr-drag"), Sense::click_and_drag());
        if r.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if r.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }

        let mut x = main.right() - 46.0 * 3.0;
        for (i, icon) in [Icon::Minimize, if maximized { Icon::Restore } else { Icon::Maximize }, Icon::Close].into_iter().enumerate() {
            let rr = Rect::from_min_size(pos2(x, main.top()), vec2(46.0, 30.0));
            let resp = ui.interact(rr, Id::new(("wctl", i)), Sense::click());
            if resp.hovered() {
                p.rect_filled(rr, 0.0, if i == 2 { Color32::from_rgb(200, 50, 60) } else { alpha(Color32::WHITE, 18) });
            }
            draw_icon(&p, rr.center(), 17.0, icon, TEXT);
            if resp.clicked() {
                match i {
                    0 => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                    1 => ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
                    _ => ctx.send_viewport_cmd(ViewportCommand::Close),
                }
            }
            x += 46.0;
        }

        let nb = Rect::from_min_size(pos2(main.left() + PAD, main.top() + 30.0), vec2(160.0, 46.0));
        let resp = ui.interact(nb, Id::new("btn-new"), Sense::click());
        p.rect_filled(nb, 13.0, if resp.hovered() { Color32::from_rgb(34, 31, 58) } else { CARD2 });
        p.rect_stroke(nb, 13.0, Stroke::new(1.0, alpha(Color32::WHITE, 20)), StrokeKind::Inside);
        let pb = Rect::from_min_size(nb.left_top() + vec2(8.0, 7.0), vec2(32.0, 32.0));
        p.rect_filled(pb, 9.0, ACCENT);
        draw_icon(&p, pb.center(), 20.0, Icon::Plus, Color32::WHITE);
        text_vc(&p, pos2(nb.left() + 52.0, nb.center().y), "Yeni İndirme", bold(15.0), TEXT);
        if resp.clicked() {
            self.new_dialog = true;
        }

        let acts = [
            (Icon::Play, "Tümünü sürdür"),
            (Icon::Pause, "Tümünü duraklat"),
            (Icon::Trash, "Tamamlananları listeden temizle"),
            (Icon::Dots, "Daha fazla"),
        ];
        let mut ax = main.right() - PAD - 4.0 * 44.0 + 4.0;
        for (i, (icon, tip)) in acts.into_iter().enumerate() {
            let rr = Rect::from_min_size(pos2(ax, main.top() + 34.0), vec2(40.0, 40.0));
            let resp = ui.interact(rr, Id::new(("act", i)), Sense::click());
            p.rect_filled(rr, 11.0, if resp.hovered() { Color32::from_rgb(40, 36, 66) } else { CARD2 });
            p.rect_stroke(rr, 11.0, Stroke::new(1.0, alpha(Color32::WHITE, 14)), StrokeKind::Inside);
            draw_icon(&p, rr.center(), 20.0, icon, if resp.hovered() { TEXT } else { alpha(TEXT, 210) });
            if resp.clicked() {
                match i {
                    0 => self.eng.resume_all(),
                    1 => self.eng.pause_all(),
                    2 => {
                        for j in view.jobs.iter().filter(|j| j.status == Status::Completed) {
                            self.eng.job_action(j.id, JobAction::Remove);
                        }
                    }
                    _ => self.menu_open = !self.menu_open,
                }
            }
            resp.on_hover_text(tip);
            ax += 44.0;
        }
    }

    fn window_resize(&self, ctx: &egui::Context, full: Rect) {
        use egui::ResizeDirection as D;
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let controls = Rect::from_min_size(pos2(full.right() - 46.0 * 3.0, full.top()), vec2(46.0 * 3.0, 30.0));
        if controls.contains(pos) {
            return;
        }
        let m = 5.0;
        let (l, r, t, b) = (pos.x - full.left() < m, full.right() - pos.x < m, pos.y - full.top() < m, full.bottom() - pos.y < m);
        let dir = match (l, r, t, b) {
            (true, _, true, _) => Some((D::NorthWest, CursorIcon::ResizeNorthWest)),
            (_, true, true, _) => Some((D::NorthEast, CursorIcon::ResizeNorthEast)),
            (true, _, _, true) => Some((D::SouthWest, CursorIcon::ResizeSouthWest)),
            (_, true, _, true) => Some((D::SouthEast, CursorIcon::ResizeSouthEast)),
            (true, ..) => Some((D::West, CursorIcon::ResizeWest)),
            (_, true, ..) => Some((D::East, CursorIcon::ResizeEast)),
            (_, _, true, _) => Some((D::North, CursorIcon::ResizeNorth)),
            (_, _, _, true) => Some((D::South, CursorIcon::ResizeSouth)),
            _ => None,
        };
        if let Some((d, cur)) = dir {
            ctx.set_cursor_icon(cur);
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
            }
        }
    }
}

impl App {
    fn job_card(&mut self, ui: &mut egui::Ui, j: &JobView, cur: Option<u64>) {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, 124.0), Sense::hover());
        let p = ui.painter().clone();
        let t = ui.input(|i| i.time) as f32;
        let running = matches!(j.status, Status::Downloading | Status::Probing);
        let done = j.status == Status::Completed;
        let selected = cur == Some(j.id);

        card(&p, rect, 16.0);
        if selected {
            p.rect_stroke(rect, 16.0, Stroke::new(1.5, alpha(ACCENT, 150)), StrokeKind::Inside);
        }
        let sel = ui.interact(rect, Id::new(("job", j.id)), Sense::click());
        if sel.clicked() {
            self.selected = Some(j.id);
        }
        if sel.double_clicked() && done {
            open_in_explorer(&j.dest, false);
        }

        let ib = Rect::from_min_size(rect.left_top() + vec2(20.0, 22.0), vec2(56.0, 56.0));
        p.rect_filled(ib, 14.0, CARD2);
        draw_icon(&p, ib.center(), 30.0, Icon::File, alpha(TEXT, 225));

        let nb: usize = if done { 3 } else { 2 };
        let mut bcx = rect.right() - 22.0 - 21.0;
        let bcy = rect.top() + 62.0;
        let mut clicked = [false; 3];
        let mut kinds: Vec<(Icon, Color32, &str)> = Vec::new();
        if done {
            kinds.push((Icon::Close, MUTED, "Listeden kaldır"));
            kinds.push((Icon::Folder, TEXT, "Klasörde göster"));
            kinds.push((Icon::File, TEXT, "Dosyayı aç"));
        } else {
            kinds.push((Icon::Close, MUTED, "İptal et ve kaldır"));
            if matches!(j.status, Status::Paused | Status::Error) {
                kinds.push((Icon::Play, GREEN, "Devam et"));
            } else {
                kinds.push((Icon::Pause, TEXT, "Duraklat"));
            }
        }
        for (k, (icon, col, tip)) in kinds.iter().enumerate() {
            let r = Rect::from_center_size(pos2(bcx, bcy), vec2(42.0, 42.0));
            let resp = ui.interact(r, Id::new(("jb", j.id, k)), Sense::click());
            p.rect_filled(r, 12.0, if resp.hovered() { Color32::from_rgb(42, 38, 70) } else { CARD2 });
            draw_icon(&p, r.center(), 22.0, *icon, *col);
            clicked[k] = resp.on_hover_text(*tip).clicked();
            bcx -= 50.0;
        }
        let sep_x = rect.right() - 22.0 - nb as f32 * 50.0 - 4.0;
        p.line_segment([pos2(sep_x, rect.top() + 18.0), pos2(sep_x, rect.bottom() - 18.0)], Stroke::new(1.0, alpha(Color32::WHITE, 16)));
        if clicked[0] {
            self.eng.job_action(j.id, JobAction::Remove);
        }
        if done {
            if clicked[1] {
                open_in_explorer(&j.dest, true);
            }
            if clicked[2] {
                open_in_explorer(&j.dest, false);
            }
        } else if clicked[1] {
            let a = if matches!(j.status, Status::Paused | Status::Error) { JobAction::Resume } else { JobAction::Pause };
            self.eng.job_action(j.id, a);
        }

        let tx = rect.left() + 96.0;
        let bar_end = sep_x - 24.0;
        let ext = j.name.rsplit_once('.').map(|(_, e)| e.to_uppercase()).filter(|e| e.len() <= 5 && !e.is_empty());
        let name = ellipsize(&p, &j.name, bold(18.0), TEXT, (bar_end - tx - 110.0).max(60.0));
        p.galley(pos2(tx, rect.top() + 27.0 - name.size().y / 2.0), name, TEXT);
        let size_s = j.size.map(fmt_bytes).unwrap_or_else(|| "boyut bilinmiyor".into());
        let sub = match &ext {
            Some(e) => format!("{size_s}   •   {e}"),
            None => size_s,
        };
        text_vc(&p, pos2(tx, rect.top() + 50.0), &sub, regular(13.5), MUTED);

        let (stxt, scol) = match j.status {
            Status::Probing => ("Bağlanıyor...", MUTED),
            Status::Downloading => ("İndiriliyor...", MUTED),
            Status::Paused => ("Duraklatıldı", AMBER),
            Status::Waiting => ("Bekliyor", AMBER),
            Status::Completed => ("Tamamlandı", GREEN),
            Status::Error => ("Hata", RED),
        };
        let pct = match j.size {
            Some(s) if s > 0 => (j.downloaded as f32 / s as f32).clamp(0.0, 1.0),
            _ => 0.0,
        };
        text_vc_right(&p, pos2(bar_end, rect.top() + 34.0), stxt, regular(13.0), scol);
        if j.size.is_some() {
            text_vc_right(&p, pos2(bar_end, rect.top() + 54.0), &format!("{:.0}%", pct * 100.0), bold(19.0), if done { GREEN } else if j.status == Status::Error { RED } else { ACCENT2 });
        }

        let bar = Rect::from_min_size(pos2(tx, rect.top() + 72.0), vec2((bar_end - tx).max(40.0), 8.0));
        p.rect_filled(bar, 4.0, Color32::from_rgb(46, 42, 74));
        if j.size.is_some() {
            if pct > 0.0 {
                let fill = Rect::from_min_size(bar.left_top(), vec2((bar.width() * pct).max(8.0), bar.height()));
                gradient_bar(&p, fill, ACCENT, if done { GREEN } else { ACCENT2 });
                if running {
                    let sw = 46.0f32.min(fill.width());
                    let x = fill.left() + ((t * 0.5) % 1.0) * (fill.width() + sw) - sw;
                    let s = Rect::from_min_max(pos2(x.max(fill.left()), fill.top()), pos2((x + sw).min(fill.right()), fill.bottom()));
                    if s.width() > 1.0 {
                        p.rect_filled(s, 4.0, alpha(Color32::WHITE, 40));
                    }
                }
            }
        } else if running {
            let seg = bar.width() * 0.25;
            let x = bar.left() + ((t * 0.6) % 1.0) * (bar.width() + seg) - seg;
            let s = Rect::from_min_max(pos2(x.max(bar.left()), bar.top()), pos2((x + seg).min(bar.right()), bar.bottom()));
            if s.width() > 1.0 {
                p.rect_filled(s, 4.0, ACCENT);
            }
        }

        let msg = if let Some(e) = &j.error {
            Some((e.clone(), RED))
        } else if let Some(n) = &j.note {
            Some((n.clone(), AMBER))
        } else if let Some((ln, m)) = j.dead.first() {
            Some((format!("{ln}: {m}"), AMBER))
        } else {
            None
        };
        let by = rect.top() + 100.0;
        if let Some((m, c)) = msg {
            let g = ellipsize(&p, &m, regular(13.0), c, bar.width());
            p.galley(pos2(tx, by - g.size().y / 2.0), g, c);
        } else {
            let left = match j.size {
                Some(s) if done => format!("{}  •  tamamlandı", fmt_bytes(s)),
                Some(s) => {
                    let mut l = format!("{} / {}", fmt_bytes(j.downloaded), fmt_bytes(s));
                    if let Some(e) = j.eta {
                        if running {
                            l += &format!("   •   Kalan: {}", fmt_eta(e));
                        }
                    }
                    l
                }
                None => fmt_bytes(j.downloaded),
            };
            text_vc(&p, pos2(tx, by), &left, regular(13.5), MUTED);
            if running || j.speed > 0.0 {
                text_vc_right(&p, pos2(bar_end, by), &format!("↓ {}", fmt_speed(j.speed)), bold(14.5), TEXT);
            }
        }
    }
}

fn link_icon(l: &LinkView) -> Icon {
    let d = format!("{} {}", l.name, l.desc).to_lowercase();
    if ["usb", "modem", "mobile", "huawei", "ndis", "tether", "internet sharing", "lte", "4g", "5g"].iter().any(|k| d.contains(k)) {
        return Icon::Usb;
    }
    match l.kind {
        LinkKind::Ethernet => Icon::Ethernet,
        LinkKind::Wifi => Icon::Wifi,
        _ => Icon::Globe,
    }
}

fn kind_label(k: LinkKind) -> &'static str {
    match k {
        LinkKind::Ethernet => "Ethernet",
        LinkKind::Wifi => "Wi-Fi",
        LinkKind::Virtual => "Sanal adaptör",
        LinkKind::Other => "Ağ adaptörü",
    }
}

impl App {
    fn links_panel(&mut self, ui: &mut egui::Ui, view: &View, cur: Option<&JobView>) {
        let w = ui.available_width();
        let links: Vec<&LinkView> = view.links.iter().filter(|l| self.show_virtual || l.kind != LinkKind::Virtual || l.enabled).collect();
        let row_h = 60.0;
        let rows = links.len().max(1) as f32;
        let h = 62.0 + 78.0 + 12.0 + rows * row_h + 14.0;
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let p = ui.painter().clone();
        card(&p, rect, 16.0);

        draw_icon(&p, pos2(rect.left() + 34.0, rect.top() + 32.0), 24.0, Icon::Network, ACCENT2);
        let r = text_vc(&p, pos2(rect.left() + 56.0, rect.top() + 32.0), "Bağlantılar", bold(19.0), TEXT);
        text_vc(&p, pos2(r.right() + 8.0, rect.top() + 33.0), "(Çoklu Ağ)", regular(15.0), MUTED);

        let inner = Rect::from_min_max(rect.left_top() + vec2(20.0, 62.0), rect.right_bottom() - vec2(20.0, 0.0));
        let gap = 12.0;
        let tw = (inner.width() - 3.0 * gap) / 4.0;
        let eta = cur.and_then(|j| j.eta).filter(|_| cur.is_some_and(is_running)).map(fmt_eta).unwrap_or_else(|| "—".into());
        let tiles = [
            (Icon::Download, fmt_speed(view.total_rate), "Toplam Hız"),
            (Icon::Layers, cur.map(|j| j.active).unwrap_or(0).to_string(), "Aktif Parça"),
            (Icon::Drive, fmt_bytes(cur.map(|j| j.downloaded).unwrap_or(0)), "İndirilen"),
            (Icon::Clock, eta, "Kalan Süre"),
        ];
        for (i, (icon, val, label)) in tiles.into_iter().enumerate() {
            let tr = Rect::from_min_size(pos2(inner.left() + i as f32 * (tw + gap), inner.top()), vec2(tw, 74.0));
            p.rect_filled(tr, 13.0, Color32::from_rgb(25, 23, 43));
            p.rect_stroke(tr, 13.0, Stroke::new(1.0, alpha(Color32::WHITE, 12)), StrokeKind::Inside);
            let ibx = Rect::from_min_size(tr.left_top() + vec2(14.0, 15.0), vec2(44.0, 44.0));
            p.rect_filled(ibx, 11.0, alpha(ACCENT, 38));
            draw_icon(&p, ibx.center(), 24.0, icon, ACCENT2);
            let g = ellipsize(&p, &val, bold(19.0), TEXT, tw - 76.0);
            p.galley(pos2(tr.left() + 70.0, tr.top() + 27.0 - g.size().y / 2.0), g, TEXT);
            text_vc(&p, pos2(tr.left() + 70.0, tr.top() + 53.0), label, regular(13.0), MUTED);
        }

        if links.is_empty() {
            p.text(pos2(rect.center().x, inner.top() + 74.0 + 12.0 + 30.0), Align2::CENTER_CENTER, "Kullanılabilir ağ bağlantısı bulunamadı. Bir kablo veya Wi-Fi bağlayın.", regular(14.0), AMBER);
            return;
        }
        let narrow = w < 900.0;
        let cols: [f32; 4] = if narrow { [0.36, 0.50, 2.0, 0.65] } else { [0.34, 0.47, 0.60, 0.72] };
        for (i, l) in links.iter().enumerate() {
            let y = inner.top() + 74.0 + 12.0 + i as f32 * row_h;
            let rr = Rect::from_min_size(pos2(rect.left() + 12.0, y), vec2(rect.width() - 24.0, row_h - 2.0));
            let live = l.enabled && l.present && l.rate > 1000.0;
            if i % 2 == 0 {
                p.rect_filled(rr, 12.0, alpha(Color32::WHITE, 5));
            }
            let cx = |f: f32| rr.left() + rr.width() * f;
            let cy = rr.center().y;

            let cb = Rect::from_center_size(pos2(rr.left() + 26.0, cy), vec2(22.0, 22.0));
            let cresp = ui.interact(cb.expand(6.0), Id::new(("lk-cb", &l.key)), Sense::click());
            if l.enabled {
                p.rect_filled(cb, 6.0, ACCENT);
                draw_icon(&p, cb.center(), 16.0, Icon::Check, Color32::WHITE);
            } else {
                p.rect_stroke(cb, 6.0, Stroke::new(1.5, alpha(Color32::WHITE, 70)), StrokeKind::Inside);
            }
            if cresp.clicked() {
                self.eng.set_link_enabled(&l.key, !l.enabled);
            }
            cresp.on_hover_text(if l.enabled { "Bu bağlantıyı kullanma" } else { "Bu bağlantıyı kullan" });

            let ib = Rect::from_center_size(pos2(rr.left() + 76.0, cy), vec2(42.0, 42.0));
            p.rect_filled(ib, 11.0, Color32::from_rgb(36, 33, 60));
            draw_icon(&p, ib.center(), 24.0, link_icon(l), if l.enabled { slot_color(l.slot) } else { MUTED });
            let name_w = rr.width() * cols[0] - 120.0;
            let g = ellipsize(&p, &l.name, bold(16.0), TEXT, name_w.max(50.0));
            p.galley(pos2(rr.left() + 108.0, cy - 11.0 - g.size().y / 2.0), g, TEXT);
            let desc = if l.desc.is_empty() || l.desc == "test" { kind_label(l.kind).to_string() } else { l.desc.clone() };
            let g = ellipsize(&p, &desc, regular(12.5), MUTED, name_w.max(50.0));
            p.galley(pos2(rr.left() + 108.0, cy + 11.0 - g.size().y / 2.0), g, MUTED);

            let (st, sc) = if !l.present {
                ("Bağlı değil", MUTED)
            } else if !l.enabled {
                ("Kapalı", MUTED)
            } else if l.exhausted {
                ("Kota doldu", RED)
            } else if live {
                ("Aktif", GREEN)
            } else {
                ("Hazır", ACCENT2)
            };
            p.circle_filled(pos2(cx(cols[0]) + 4.0, cy), 4.0, sc);
            text_vc(&p, pos2(cx(cols[0]) + 16.0, cy), st, regular(14.5), sc);

            let speed_txt = format!("↓ {}", fmt_speed(l.rate));
            text_vc(&p, pos2(cx(cols[1]), cy), &speed_txt, bold(15.0), if live { TEXT } else { alpha(TEXT, 150) });

            if cols[2] < 1.5 {
                let b = cur
                    .and_then(|j| j.per_link.iter().find(|(s, _)| *s as usize == l.slot))
                    .map(|(_, b)| *b)
                    .unwrap_or(0);
                text_vc(&p, pos2(cx(cols[2]), cy), &format!("↓ {}", fmt_bytes(b)), regular(14.5), alpha(TEXT, 215));
            }

            let qx = cx(cols[3]);
            let qr = rr.right() - 62.0;
            let (qtxt, pct) = if l.limit == 0 {
                ("Kota: Sınırsız".to_string(), None)
            } else {
                (format!("Kota: {}", fmt_bytes(l.limit)), Some((l.used as f32 / l.limit as f32).clamp(0.0, 1.0)))
            };
            text_vc(&p, pos2(qx, cy - 9.0), &qtxt, regular(13.0), alpha(TEXT, 220));
            let qbar = Rect::from_min_max(pos2(qx, cy + 9.0), pos2(qr.max(qx + 40.0), cy + 15.0));
            p.rect_filled(qbar, 3.0, Color32::from_rgb(46, 42, 74));
            if let Some(pc) = pct {
                text_vc_right(&p, pos2(qbar.right(), cy - 9.0), &format!("{:.0}%", pc * 100.0), regular(13.0), if pc > 0.9 { RED } else { MUTED });
                if pc > 0.0 {
                    let f = Rect::from_min_size(qbar.left_top(), vec2((qbar.width() * pc).max(6.0), qbar.height()));
                    p.rect_filled(f, 3.0, if pc > 0.9 { RED } else if pc > 0.75 { AMBER } else { ACCENT });
                }
            } else if l.used > 0 {
                text_vc_right(&p, pos2(qbar.right(), cy - 9.0), &fmt_bytes(l.used), regular(13.0), MUTED);
            }

            let mb = Rect::from_center_size(pos2(rr.right() - 26.0, cy), vec2(34.0, 34.0));
            let mresp = ui.interact(mb, Id::new(("lk-menu", &l.key)), Sense::click());
            if mresp.hovered() {
                p.rect_filled(mb, 9.0, alpha(Color32::WHITE, 14));
            }
            draw_icon(&p, mb.center(), 20.0, Icon::Dots, TEXT);
            if mresp.on_hover_text("Kota ve hız ayarları").clicked() {
                self.link_dialog = Some(l.key.clone());
            }
        }
    }

    fn chunks_panel(&mut self, ui: &mut egui::Ui, view: &View, cur: Option<&JobView>, time: f32) {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, 122.0), Sense::hover());
        let p = ui.painter().clone();
        card(&p, rect, 16.0);
        let r = text_vc(&p, pos2(rect.left() + 24.0, rect.top() + 32.0), "Parça Dağılımı", bold(18.0), TEXT);
        text_vc(&p, pos2(r.right() + 8.0, rect.top() + 33.0), &format!("({} Parça)", crate::engine::job::MAP_CELLS), regular(14.5), MUTED);

        let cells = cur.map(|j| j.cells.clone()).unwrap_or_default();
        let mut counts = [0usize; 8];
        for c in &cells {
            if c.fill > 0.0 {
                counts[c.slot as usize % 8] += 1;
            }
        }

        let mut lx = rect.right() - 24.0;
        let mut legend: Vec<(Color32, String)> = Vec::new();
        for l in view.links.iter() {
            let n = counts[l.slot % 8];
            if n > 0 {
                legend.push((slot_color(l.slot), format!("{} ({})", l.name, n)));
            }
        }
        for (col, txt) in legend.iter().rev() {
            let g = p.layout_no_wrap(txt.clone(), regular(13.0), TEXT);
            lx -= g.size().x;
            p.galley(pos2(lx, rect.top() + 33.0 - g.size().y / 2.0), g, TEXT);
            lx -= 10.0;
            let sq = Rect::from_center_size(pos2(lx - 7.0, rect.top() + 33.0), vec2(14.0, 14.0));
            p.rect_filled(sq, 4.0, *col);
            lx -= 14.0 + 20.0;
        }

        let n = crate::engine::job::MAP_CELLS;
        let gap = 7.0;
        let x0 = rect.left() + 24.0;
        let cw = ((rect.width() - 48.0 - gap * (n as f32 - 1.0)) / n as f32).max(4.0);
        for i in 0..n {
            let r = Rect::from_min_size(pos2(x0 + i as f32 * (cw + gap), rect.top() + 62.0), vec2(cw, 40.0));
            p.rect_filled(r, 5.0, alpha(Color32::WHITE, 10));
            if let Some(c) = cells.get(i) {
                if c.fill > 0.0 {
                    p.rect_filled(r, 5.0, alpha(slot_color(c.slot as usize), (60.0 + 195.0 * c.fill) as u8));
                }
                if c.active {
                    let s = 0.5 + 0.5 * (time * 7.0 + i as f32 * 0.6).sin();
                    p.rect_filled(r, 5.0, alpha(Color32::WHITE, (8.0 + 34.0 * s) as u8));
                }
            }
        }
        if cells.is_empty() {
            p.text(pos2(rect.right() - 24.0, rect.top() + 33.0), Align2::RIGHT_CENTER, "Henüz indirme yok", regular(13.0), MUTED);
        }
    }
}

fn page_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, alpha(Color32::WHITE, 14)))
        .corner_radius(16)
        .inner_margin(Margin::same(24))
}

impl App {
    fn settings_page(&mut self, ui: &mut egui::Ui) {
        let mut s: Settings = self.view.settings.clone();
        let before = (s.download_dir.clone(), s.conns_per_link);
        page_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Ayarlar").font(bold(20.0)));
            ui.add_space(6.0);
            ui.label(RichText::new("İndirme klasörü").color(MUTED));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut s.download_dir).desired_width(ui.available_width() - 110.0));
                if ui.button("Gözat…").clicked() && self.folder_rx.is_none() {
                    let (tx, rx) = channel();
                    self.folder_rx = Some(rx);
                    std::thread::spawn(move || {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            let _ = tx.send(p.to_string_lossy().to_string());
                        }
                    });
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Hat başına paralel akış").color(MUTED));
            ui.add(egui::Slider::new(&mut s.conns_per_link, 1..=16));
            ui.add_space(6.0);
            ui.checkbox(&mut self.show_virtual, "Sanal adaptörleri göster");
        });
        if (s.download_dir.clone(), s.conns_per_link) != before {
            self.apply_settings(s);
        }
    }

    fn apply_settings(&mut self, s: Settings) {
        self.view.settings = s.clone();
        self.eng.set_settings(s);
    }

    fn about_page(&mut self, ui: &mut egui::Ui) {
        page_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("LetsDownload").font(bold(20.0)));
            ui.label(RichText::new("Birden fazla internet bağlantısını birleştiren indirme yöneticisi.").color(MUTED));
        });
    }
}

impl App {
    fn dialogs(&mut self, ctx: &egui::Context, view: &View, main: Rect) {

        if self.menu_open {
            let pos = pos2(main.right() - PAD - 190.0, main.top() + 80.0);
            let out = egui::Area::new(Id::new("more-menu")).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 22, 44))
                    .stroke(Stroke::new(1.0, alpha(Color32::WHITE, 28)))
                    .corner_radius(14)
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(180.0);
                        let entries = [("Ayarlar", Icon::Gear), ("Hakkında", Icon::Info), ("Bağlantıları yenile", Icon::Network)];
                        for (i, (label, icon)) in entries.into_iter().enumerate() {
                            let (r, resp) = ui.allocate_exact_size(vec2(180.0, 38.0), Sense::click());
                            if resp.hovered() {
                                ui.painter().rect_filled(r, 9.0, alpha(Color32::WHITE, 14));
                            }
                            draw_icon(ui.painter(), pos2(r.left() + 20.0, r.center().y), 18.0, icon, MUTED);
                            ui.painter().text(pos2(r.left() + 40.0, r.center().y), Align2::LEFT_CENTER, label, regular(14.0), TEXT);
                            if resp.clicked() {
                                match i {
                                    0 => self.page = Page::Settings,
                                    1 => self.page = Page::About,
                                    _ => self.eng.refresh_links(),
                                }
                                self.menu_open = false;
                            }
                        }
                    });
            });
            let menu_rect = out.response.rect;
            let dots_rect = Rect::from_min_size(pos2(main.right() - PAD - 44.0 + 4.0, main.top() + 34.0), vec2(40.0, 40.0));
            let outside = ctx.input(|i| i.pointer.any_pressed())
                && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !menu_rect.contains(p) && !dots_rect.contains(p));
            if outside {
                self.menu_open = false;
            }
        }

        if self.new_dialog {
            let mut open = true;
            let mut submit = false;
            egui::Window::new("Yeni İndirme")
                .frame(egui::Frame::window(&ctx.style_of(egui::Theme::Dark)).inner_margin(Margin::same(18)))
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, -20.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.set_width(500.0);
                    ui.label(RichText::new("Bağlantı").color(MUTED));
                    let te = egui::TextEdit::multiline(&mut self.input)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("https://…")
                        .font(regular(15.0));
                    let resp = ui.add(te);
                    if self.input.is_empty() && !resp.has_focus() {
                        resp.request_focus();
                    }
                    if resp.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
                        submit = true;
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let (r, resp) = ui.allocate_exact_size(vec2(120.0, 40.0), Sense::click());
                        ui.painter().rect_filled(r, 11.0, if resp.hovered() { Color32::from_rgb(150, 105, 255) } else { ACCENT });
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, "İndir", bold(15.0), Color32::WHITE);
                        if resp.clicked() {
                            submit = true;
                        }
                    });
                });
            if submit {
                let text = std::mem::take(&mut self.input);
                let n = self.eng.add_urls(&text);
                let msg = if n == 0 { "Geçerli bir bağlantı bulunamadı".to_string() } else { format!("{n} indirme eklendi") };
                self.toast = Some((msg, Instant::now()));
                if n > 0 {
                    self.page = Page::Downloads;
                    open = false;
                }
            }
            if !open {
                self.new_dialog = false;
            }
        }

        if let Some(key) = self.link_dialog.clone() {
            if let Some(l) = view.links.iter().find(|l| l.key == key) {
                let mut open = true;
                let gb = *self.units_gb.entry(l.key.clone()).or_insert_with(|| l.limit >= 1_000_000_000 && l.limit % 100_000_000 == 0);
                egui::Window::new(format!("{} — kota ve hız", l.name))
                    .frame(egui::Frame::window(&ctx.style_of(egui::Theme::Dark)).inner_margin(Margin::same(18)))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, [0.0, -20.0])
                    .open(&mut open)
                    .show(ctx, |ui| {
                        ui.set_width(380.0);
                        let unlimited = l.limit == 0;
                        ui.label(RichText::new("Kota").color(MUTED));
                        ui.horizontal(|ui| {
                            let mult = if gb { 1e9 } else { 1e6 };
                            let mut val = l.limit as f64 / mult;
                            let r = ui.add_enabled(!unlimited, egui::DragValue::new(&mut val).range(0.0..=1_000_000.0).speed(if gb { 0.1 } else { 10.0 }).max_decimals(2));
                            if r.changed() {
                                self.eng.set_link_limit(&l.key, (val.max(0.0) * mult) as u64);
                            }
                            if ui.selectable_label(!gb, "MB").clicked() && gb {
                                self.units_gb.insert(l.key.clone(), false);
                            }
                            if ui.selectable_label(gb, "GB").clicked() && !gb {
                                self.units_gb.insert(l.key.clone(), true);
                            }
                            if ui.selectable_label(unlimited, "Sınırsız").clicked() {
                                self.eng.set_link_limit(&l.key, if unlimited { 10_000_000_000 } else { 0 });
                                if unlimited {
                                    self.units_gb.insert(l.key.clone(), true);
                                }
                            }
                        });
                        ui.label(RichText::new(format!("Kullanılan: {}", fmt_bytes(l.used))).color(TEXT));
                        ui.add_space(4.0);
                        ui.label(RichText::new("Hız sınırı").color(MUTED));
                        let mut cap = l.speed_cap as f64 / 1e6;
                        let r = ui.add(egui::DragValue::new(&mut cap).range(0.0..=10_000.0).speed(0.2).max_decimals(1).suffix(" MB/s"));
                        if r.changed() {
                            self.eng.set_link_speed_cap(&l.key, (cap.max(0.0) * 1e6) as u64);
                        }
                        ui.add_space(6.0);
                        if ui.button("Kullanılan kotayı sıfırla").clicked() {
                            self.eng.reset_quota(&l.key);
                        }
                    });
                if !open {
                    self.link_dialog = None;
                }
            } else {
                self.link_dialog = None;
            }
        }
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        if let Some((msg, t)) = &self.toast {
            let age = t.elapsed().as_secs_f32();
            if age > 2.6 {
                self.toast = None;
                return;
            }
            let a = ((2.6 - age).min(0.4) / 0.4).min(1.0);
            let screen = ctx.content_rect();
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("toast")));
            let g = p.layout_no_wrap(msg.clone(), bold(14.0), alpha(TEXT, (255.0 * a) as u8));
            let r = Rect::from_center_size(pos2(screen.center().x + SIDEBAR_W / 2.0, screen.bottom() - 46.0), g.size() + vec2(40.0, 22.0));
            p.rect_filled(r, 14.0, alpha(Color32::from_rgb(30, 26, 56), (245.0 * a) as u8));
            p.rect_stroke(r, 14.0, Stroke::new(1.0, alpha(ACCENT2, (160.0 * a) as u8)), StrokeKind::Inside);
            p.galley(r.center() - g.size() / 2.0, g, TEXT);
        }
    }
}

fn open_in_explorer(path: &std::path::Path, select: bool) {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("explorer.exe");
    if select {
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    } else {
        cmd.raw_arg(format!("\"{}\"", path.display()));
    }
    let _ = cmd.spawn();
}
