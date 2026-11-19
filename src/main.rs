#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod dev;
mod engine;
mod ui;

use engine::Engine;

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let _guard = rt.enter();
    let eng = Engine::new(rt.handle().clone());

    dev::apply(&eng);
    if dev::run_cli(&eng) {
        return;
    }

    let size = dev::window_size().unwrap_or([1280.0, 800.0]);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("LetsDownload")
            .with_inner_size(size)
            .with_decorations(false)
            .with_min_inner_size([1000.0, 640.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    let e2 = eng.clone();
    if let Err(e) = eframe::run_native(
        "LetsDownload",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, e2)))),
    ) {
        eprintln!("arayüz başlatılamadı: {e}");
    }
    eng.shutdown();
}

fn app_icon() -> eframe::egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    let rings = [((22.0f32, 32.0f32), [34u8, 211, 238]), ((42.0, 32.0), [244, 114, 182])];
    for y in 0..n {
        for x in 0..n {
            let mut px = [0u8; 4];
            for ((cx, cy), c) in rings {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let a = (1.0 - ((d - 15.0).abs() - 3.0).max(0.0)).clamp(0.0, 1.0);
                if a > 0.0 {
                    let o = a;
                    for k in 0..3 {
                        px[k] = (px[k] as f32 * (1.0 - o) + c[k] as f32 * o) as u8;
                    }
                    px[3] = px[3].max((a * 255.0) as u8);
                }
            }
            let i = (y * n + x) * 4;
            rgba[i..i + 4].copy_from_slice(&px);
        }
    }
    eframe::egui::IconData { rgba, width: n as u32, height: n as u32 }
}
