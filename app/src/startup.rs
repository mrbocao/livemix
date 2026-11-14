use super::*;
use std::time::Duration;

pub(crate) fn icon() -> Arc<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../assets/livemix-icon.png"))
        .expect("embedded LiveMix icon must be a PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    Arc::new(egui::IconData { rgba: image.into_raw(), width, height })
}

pub(crate) fn initialize(ctx: &egui::Context, gl: Option<&eframe::glow::Context>, screenshot: Option<String>) -> App {
    let mut app = App::new(screenshot);
    App::refresh_external_devices(ctx.clone());
    apply_style(ctx, app.s.cfg.theme);
    if let Some(gl) = gl {
        app.graphics_adapter = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
        app.performance.set_adapter(&app.graphics_adapter);
    }
    #[cfg(windows)]
    {
        let mut preference = app.s.cfg.video_adapter.clone();
        if preference.is_empty() {
            if let Some(adapter) = media::video_adapter_names().into_iter()
                .find(|adapter| media::renderer_matches_adapter(&app.graphics_adapter, adapter))
            {
                preference = adapter;
            }
        }
        media::select_video_adapter(&preference);
    }
    app
}

pub(crate) struct Startup {
    logo: Option<egui::TextureHandle>,
    app: Option<App>,
    shown: bool,
}

impl Startup {
    pub fn new(ctx: &egui::Context, icon: &egui::IconData) -> Self {
        let picture = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize], &icon.rgba);
        let logo = ctx.load_texture("livemix-startup", picture, egui::TextureOptions::LINEAR);
        Self { logo: Some(logo), app: None, shown: false }
    }
}

impl eframe::App for Startup {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.shown {
            let app = self.app.get_or_insert_with(|| initialize(ctx, frame.gl().map(Arc::as_ref), None));
            self.logo = None;
            app.update(ctx, frame);
            return;
        }
        self.shown = true;
        let background = Color32::from_rgb(9, 16, 27);
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(background)).show(ctx, |ui| {
            let center = ui.max_rect().center();
            let painter = ui.painter();
            let logo = self.logo.as_ref().unwrap();
            painter.image(logo.id(), egui::Rect::from_center_size(center - egui::vec2(0.0, 76.0), egui::vec2(144.0, 144.0)),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
            painter.text(center + egui::vec2(0.0, 30.0), egui::Align2::CENTER_CENTER, "LiveMix",
                egui::FontId::proportional(46.0), Color32::from_rgb(241, 248, 255));
            painter.text(center + egui::vec2(0.0, 70.0), egui::Align2::CENTER_CENTER, "Starting LiveMix…",
                egui::FontId::proportional(15.0), Color32::from_rgb(163, 184, 207));
            let progress = egui::Rect::from_center_size(center + egui::vec2(0.0, 109.0), egui::vec2(190.0, 3.0));
            painter.rect_filled(progress, 2.0, Color32::from_rgb(27, 45, 63));
            painter.rect_filled(egui::Rect::from_center_size(progress.center(), egui::vec2(52.0, 3.0)), 2.0,
                Color32::from_rgb(47, 211, 239));
        });
        ctx.request_repaint_after(Duration::from_millis(220));
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(app) = &mut self.app { app.on_exit(gl); }
    }
}
