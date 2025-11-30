use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use crossbeam_channel::{unbounded, Receiver, Sender};
use crate::{audio, analysis, player, export, i18n, LoopPoints, AnalysisResult, AnalysisSettings, DetectionMode, FadeOutMode};
use rodio::{OutputStream, Sink};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use web_sys::{HtmlElement, Url};

#[cfg(not(target_arch = "wasm32"))]
use std::thread;

#[cfg(not(target_arch = "wasm32"))]
pub fn run(initial_file: Option<PathBuf>) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 650.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Auto A-B Loop",
        options,
        Box::new(move |cc| {
            configure_visuals(&cc.egui_ctx);
            Ok(Box::new(MyApp::new(initial_file, cc.egui_ctx.clone())))
        }),
    ).map_err(|e| anyhow::anyhow!("GUI Error: {}", e))
}

#[cfg(target_arch = "wasm32")]
pub fn run(_initial_file: Option<PathBuf>) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("CLI mode not supported on WASM"))
}

pub fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_gray(20);
    ctx.set_visuals(visuals);

    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "DroidSansFallback".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "fonts/DroidSansFallback.ttf"
        ))),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "DroidSansFallback".to_owned());

    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("DroidSansFallback".to_owned());

    ctx.set_fonts(fonts);
}

#[derive(Clone)]
enum AppState {
    Idle,
    Loading,
    Analyzing(Arc<audio::AudioData>, AnalysisSettings), 
    Ready(Arc<audio::AudioData>, AnalysisResult),
    Error(String),
    Exporting,
    ExportSuccess,
    ExportError(String),
}

enum AppMessage {
    Loaded(String, Arc<audio::AudioData>), 
    Analyzed(AnalysisResult),
    Error(String),
    ExportFinished(anyhow::Result<()>),
}

pub struct MyApp {
    state: Arc<Mutex<AppState>>, 
    msg_receiver: Receiver<AppMessage>,
    msg_sender: Sender<AppMessage>,
    ctx: egui::Context, 

    _stream: Option<OutputStream>,
    sink: Option<Sink>,
    
    loop_count: u32,
    infinite_loop: bool,
    file_name: Option<String>,
    volume: f32, 
    cover_texture: Option<egui::TextureHandle>,
    waveform_cache: Option<Vec<f32>>,
    export_loops: u32,
    analysis_settings: AnalysisSettings, // New: Analysis settings
}

impl MyApp {
    pub fn new(initial_file: Option<PathBuf>, ctx: egui::Context) -> Self {
        let (sender, receiver) = unbounded();
        
        let mut app = Self {
            state: Arc::new(Mutex::new(AppState::Idle)),
            msg_receiver: receiver,
            msg_sender: sender.clone(),
            ctx,
            _stream: None,
            sink: None,
            loop_count: 5, 
            infinite_loop: true, 
            file_name: None,
            volume: 0.8,
            cover_texture: None,
            waveform_cache: None,
            export_loops: 5,
            analysis_settings: AnalysisSettings::default(), // New: Initialize
        };

        if let Ok((stream, stream_handle)) = OutputStream::try_default() {
             let sink = Sink::try_new(&stream_handle).ok();
             if let Some(s) = &sink {
                 s.set_volume(app.volume);
             }
             app._stream = Some(stream);
             app.sink = sink;
        }
        
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = initial_file {
            app.load_file_native(path);
        }

        app
    }
    
    // New function to trigger analysis
    fn trigger_analysis(&mut self, audio_data: Arc<audio::AudioData>) {
        *self.state.lock().unwrap() = AppState::Analyzing(audio_data.clone(), self.analysis_settings.clone());
        self.ctx.request_repaint();

        let sender = self.msg_sender.clone();
        let ctx = self.ctx.clone();
        let settings = self.analysis_settings.clone(); // Capture current settings
        
        thread::spawn(move || {
            let result = analysis::run_analysis(&audio_data, &settings);
            sender.send(AppMessage::Analyzed(result)).ok();
            ctx.request_repaint();
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn load_file_native(&mut self, path: PathBuf) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        self.file_name = Some(name.clone());
        self.cover_texture = None;
        self.waveform_cache = None;
        *self.state.lock().unwrap() = AppState::Loading;
        
        let sender = self.msg_sender.clone();
        let ctx = self.ctx.clone();
        let analysis_settings = self.analysis_settings.clone(); // Capture settings for analysis thread
        
        thread::spawn(move || {
            match audio::load_audio_file(&path) {
                Ok(data) => {
                    let arc_data = Arc::new(data);
                    sender.send(AppMessage::Loaded(name, arc_data.clone())).ok();
                    ctx.request_repaint();
                    
                    // Run analysis with current settings
                    let result = analysis::run_analysis(&arc_data, &analysis_settings);
                    sender.send(AppMessage::Analyzed(result)).ok();
                    ctx.request_repaint();
                }
                Err(e) => {
                    sender.send(AppMessage::Error(e.to_string())).ok();
                    ctx.request_repaint();
                }
            }
        });
    }
    
    #[cfg(target_arch = "wasm32")]
    fn pick_file_web(&mut self) {
        let sender = self.msg_sender.clone();
        let ctx = self.ctx.clone();
        let analysis_settings = self.analysis_settings.clone(); // Capture settings for analysis thread
        
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(file) = rfd::AsyncFileDialog::new().pick_file().await {
                let name = file.file_name();
                let data = file.read().await;
                let hint = name.split('.').last().map(|s| s.to_string());

                match audio::load_audio_from_bytes(data, hint.as_deref()) {
                    Ok(audio_data) => {
                         let arc_data = Arc::new(audio_data);
                         sender.send(AppMessage::Loaded(name, arc_data.clone())).ok();
                         ctx.request_repaint();
                         
                         // Run analysis with current settings
                         let result = analysis::run_analysis(&arc_data, &analysis_settings);
                         sender.send(AppMessage::Analyzed(result)).ok();
                         ctx.request_repaint();
                    }
                    Err(e) => {
                        sender.send(AppMessage::Error(e.to_string())).ok();
                        ctx.request_repaint();
                    }
                }
            }
        });
    }

    fn start_playback(&mut self) {
        let state = self.state.lock().unwrap();
        if let AppState::Ready(data, analysis_result) = &*state {
            if let Some(sink) = &self.sink {
                sink.stop(); 
                let lp = analysis_result.loop_points.clone().unwrap_or(LoopPoints { start_sample: 0, end_sample: data.samples.len(), confidence: 0.0 });
                let fo_info = analysis_result.fade_out_info.clone(); // New: Pass fade-out info
                let max_loops = if self.infinite_loop { None } else { Some(self.loop_count) };
                
                // Pass fade_out_info to LoopingSource
                let source = player::LoopingSource::new((**data).clone(), lp, max_loops, fo_info);
                sink.append(source);
                sink.set_volume(self.volume);
                sink.play();
            }
        }
    }

    fn stop_playback(&self) {
        if let Some(sink) = &self.sink {
            sink.stop();
        }
    }
    
    fn update_volume(&self) {
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
        }
    }
    
    fn export_file(&mut self) {
         let state_guard = self.state.lock().unwrap();
         if let AppState::Ready(data_arc, analysis_result) = &*state_guard { // Renamed `data` to `data_arc`
             let data_for_thread = data_arc.clone(); // Clone the Arc<AudioData>
             let loop_points_for_thread = analysis_result.loop_points.clone().unwrap_or(LoopPoints { start_sample: 0, end_sample: data_arc.samples.len(), confidence: 0.0 });
             let fade_out_info_for_thread = analysis_result.fade_out_info.clone(); // Clone FadeOutInfo
             let loops_for_thread = self.export_loops; // u32 is Copy, so no explicit clone needed
             
             drop(state_guard); // Now it's safe to drop state_guard as all needed data is cloned

             #[cfg(not(target_arch = "wasm32"))]
             {
                 if let Some(path) = rfd::FileDialog::new().set_file_name("loop_export.wav").save_file() {
                     *self.state.lock().unwrap() = AppState::Exporting;
                     let sender = self.msg_sender.clone();
                     let ctx = self.ctx.clone();
                     thread::spawn(move || {
                         let res = export::export_loop(&path, (*data_for_thread).clone(), loop_points_for_thread, loops_for_thread, fade_out_info_for_thread);
                         sender.send(AppMessage::ExportFinished(res)).ok();
                         ctx.request_repaint();
                     });
                 }
             }
             
             #[cfg(target_arch = "wasm32")]
             {
                 *self.state.lock().unwrap() = AppState::Exporting;
                 let sender = self.msg_sender.clone();
                 let ctx = self.ctx.clone();
                 wasm_bindgen_futures::spawn_local(async move {
                     let file_name = format!("{}_loop_exported.wav", data_for_thread.title.as_deref().unwrap_or("audio"));
                     let res = export::export_loop_web((*data_for_thread).clone(), loop_points_for_thread, loops_for_thread, fade_out_info_for_thread);
                     match res {
                         Ok(wav_data) => {
                             if let Err(e) = download_bytes_as_file(file_name, wav_data).await {
                                 sender.send(AppMessage::ExportFinished(Err(e))).ok();
                             } else {
                                 sender.send(AppMessage::ExportFinished(Ok(()))).ok();
                             }
                         }
                         Err(e) => {
                             sender.send(AppMessage::ExportFinished(Err(e))).ok();
                         }
                     }
                     ctx.request_repaint();
                 });
             }
         }
    }

    fn generate_waveform(&mut self, data: &audio::AudioData) {
        let width = 1200;
        let samples = &data.samples;
        let step = (samples.len() / width).max(1);
        let mut cache = Vec::with_capacity(width * 2);
        
        for i in 0..width {
            let start = i * step;
            let end = (start + step).min(samples.len());
            if start >= end { break; }
            
            let chunk = &samples[start..end];
            let mut min = f32::MAX;
            let mut max = f32::MIN;
            
            let sub_step = (chunk.len() / 10).max(1);
            for j in (0..chunk.len()).step_by(sub_step) {
                let val = chunk[j];
                if val < min { min = val; }
                if val > max { max = val; }
            }
            if min == f32::MAX { min = 0.0; }
            if max == f32::MIN { max = 0.0; }
            
            cache.push(min);
            cache.push(max);
        }
        self.waveform_cache = Some(cache);
    }

    // This function is now correctly part of impl MyApp
    fn render_player_ui(&mut self, ui: &mut egui::Ui, data: &audio::AudioData, analysis_result: &AnalysisResult) {
        ui.horizontal(|ui| {
            let cover_size = egui::vec2(200.0, 200.0);
            if let Some(texture) = &self.cover_texture {
                ui.add(egui::Image::new(texture).max_size(cover_size));
            } else {
                let (rect, _resp) = ui.allocate_exact_size(cover_size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 8.0, egui::Color32::from_gray(40));
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "No Cover", egui::FontId::proportional(20.0), egui::Color32::GRAY);
            }

            ui.vertical(|ui| {
                ui.label(egui::RichText::new(data.title.as_deref().unwrap_or(&i18n::t("unknown_title")))
                    .size(28.0).strong().color(egui::Color32::WHITE));
                ui.label(egui::RichText::new(data.artist.as_deref().unwrap_or(&i18n::t("unknown_artist")))
                    .size(18.0).color(egui::Color32::LIGHT_GRAY));
                ui.label(egui::RichText::new(data.album.as_deref().unwrap_or(&i18n::t("unknown_album")))
                    .size(14.0).color(egui::Color32::GRAY));
                
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Fmt:").strong());
                    ui.label(format!("{}Hz / {}ch", data.sample_rate, data.channels));
                });
                
                ui.add_space(10.0);
                // Display Analysis Result
                ui.group(|ui| {
                    if let Some(p) = &analysis_result.loop_points {
                        let confidence_pct = (p.confidence * 100.0).clamp(0.0, 100.0);
                        let color = if confidence_pct > 80.0 { egui::Color32::GREEN } else { egui::Color32::YELLOW };
                        
                        ui.horizontal(|ui| {
                            ui.colored_label(color, format!("✔ {}", i18n::t("loop_found")));
                            ui.label(format!("{}: {:.0}%", i18n::t("confidence"), confidence_pct));
                        });
                        
                        let duration_fmt = |samples: usize| -> String {
                            let s = samples as f32 / data.sample_rate as f32 / data.channels as f32;
                            format!("{:.2}s", s)
                        };
                        
                        ui.label(format!("{}  ➡  {}", duration_fmt(p.start_sample), duration_fmt(p.end_sample)));
                    } else {
                        ui.colored_label(egui::Color32::YELLOW, i18n::t("no_loop"));
                    }

                    if let Some(fo) = &analysis_result.fade_out_info {
                        ui.colored_label(egui::Color32::LIGHT_BLUE, format!("↘ {}", i18n::t("fade_out_detected")));
                        let duration_s = fo.duration_samples as f32 / data.sample_rate as f32 / data.channels as f32;
                        ui.label(format!("Start: {:.2}s, Duration: {:.2}s", 
                            fo.start_sample as f32 / data.sample_rate as f32 / data.channels as f32, 
                            duration_s));
                    }
                });
                
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(i18n::t("loop_count"));
                    ui.add(egui::DragValue::new(&mut self.export_loops).range(1..=99));
                    if ui.button(i18n::t("export")).clicked() {
                        self.export_file(); 
                    }
                });
            });
        });

        ui.add_space(20.0);
        
        if let Some(waveform) = &self.waveform_cache {
            let (rect, _resp) = ui.allocate_at_least(egui::vec2(ui.available_width(), 100.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 4.0, egui::Color32::from_black_alpha(100));
            
            let points_count = waveform.len() / 2;
            let w_step = rect.width() / points_count as f32;
            let center_y = rect.center().y;
            let height_scale = rect.height() / 2.0;
            let wave_color = egui::Color32::from_rgb(100, 150, 255);
            
            for i in 0..points_count {
                let min = waveform[i*2];
                let max = waveform[i*2+1];
                let x = rect.min.x + i as f32 * w_step;
                 ui.painter().line_segment(
                     [egui::pos2(x, center_y + min * height_scale), 
                      egui::pos2(x, center_y + max * height_scale)], 
                     egui::Stroke::new(1.0, wave_color)
                 );
            }
            
            // Draw Loop Points
            if let Some(p) = &analysis_result.loop_points {
                 let total_samples = data.samples.len();
                 let start_x = rect.min.x + (p.start_sample as f32 / total_samples as f32) * rect.width();
                 let end_x = rect.min.x + (p.end_sample as f32 / total_samples as f32) * rect.width();
                 
                 let loop_color = egui::Color32::GREEN;
                 ui.painter().line_segment([egui::pos2(start_x, rect.min.y), egui::pos2(start_x, rect.max.y)], egui::Stroke::new(2.0, loop_color));
                 ui.painter().line_segment([egui::pos2(end_x, rect.min.y), egui::pos2(end_x, rect.max.y)], egui::Stroke::new(2.0, egui::Color32::RED));
                 
                 if end_x > start_x {
                     ui.painter().rect_filled(
                         egui::Rect::from_min_max(egui::pos2(start_x, rect.min.y), egui::pos2(end_x, rect.max.y)), 
                         0.0, 
                         egui::Color32::from_rgba_unmultiplied(0, 255, 0, 20)
                     );
                 }
            }

            // Draw Fade-Out Info
            if let Some(fo) = &analysis_result.fade_out_info {
                let total_samples = data.samples.len();
                let fo_start_x = rect.min.x + (fo.start_sample as f32 / total_samples as f32) * rect.width();
                let fo_end_x = rect.min.x + ((fo.start_sample + fo.duration_samples) as f32 / total_samples as f32) * rect.width();

                ui.painter().line_segment([egui::pos2(fo_start_x, rect.min.y), egui::pos2(fo_start_x, rect.max.y)], egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE));
                
                if fo_end_x > fo_start_x {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(egui::pos2(fo_start_x, rect.min.y), egui::pos2(fo_end_x, rect.max.y)), 
                        0.0, 
                        egui::Color32::from_rgba_unmultiplied(0, 150, 255, 30) // Light blue, semi-transparent
                    );
                }
            }
        }
        
        ui.add_space(20.0);
        
        ui.horizontal(|ui| {
            let btn_size = egui::vec2(40.0, 40.0);
            if ui.add(egui::Button::new(egui::RichText::new("▶").size(20.0)).min_size(btn_size)).clicked() {
                self.start_playback();
            }
             if ui.add(egui::Button::new(egui::RichText::new("⏹").size(20.0)).min_size(btn_size)).clicked() {
                self.stop_playback();
            }
            
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.label(i18n::t("volume"));
                if ui.add(egui::Slider::new(&mut self.volume, 0.0..=1.5).show_value(false)).changed() {
                    self.update_volume();
                }
            });
            
            ui.add_space(20.0);
             ui.vertical(|ui| {
                ui.label(i18n::t("play")); 
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.infinite_loop, i18n::t("infinite"));
                    if !self.infinite_loop {
                        ui.add(egui::DragValue::new(&mut self.loop_count).range(1..=99));
                    }
                });
            });
        });
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.msg_receiver.try_recv() {
             let mut state = self.state.lock().unwrap();
            match msg {
                AppMessage::Loaded(name, data) => {
                    self.file_name = Some(name);
                    drop(state); // Release lock before calling generate_waveform and trigger_analysis
                    self.generate_waveform(&data);
                    
                    if let Some(img_arc) = &data.cover_art {
                        let img = img_arc.clone();
                        let size = [img.width() as usize, img.height() as usize];
                        let image_buffer = img.to_rgba8();
                        let pixels = image_buffer.as_flat_samples();
                        let color_image = egui::ColorImage::from_rgba_unmultiplied(
                            size,
                            pixels.as_slice(),
                        );
                        self.cover_texture = Some(ctx.load_texture("cover", color_image, Default::default()));
                    }
                    self.trigger_analysis(data); // Trigger analysis after loading and waveform generation
                }
                AppMessage::Analyzed(result) => {
                     match &*state {
                         AppState::Analyzing(data, _settings) => { // Capture settings as well
                             *state = AppState::Ready(data.clone(), result);
                             drop(state);
                             self.start_playback();
                             return;
                         }
                         _ => {}
                     }
                }
                AppMessage::Error(e) => {
                    *state = AppState::Error(e);
                }
                AppMessage::ExportFinished(res) => {
                    match res {
                        Ok(_) => *state = AppState::ExportSuccess,
                        Err(e) => *state = AppState::ExportError(e.to_string()),
                    }
                }
            }
        }
        
        let current_state_clone_for_display = self.state.lock().unwrap().clone(); // Clone for display and later use
        let mut re_analyze_triggered_by_ui = false; // Renamed for clarity

        egui::CentralPanel::default().show(ctx, |ui| {
            let spacing = 10.0;
            ui.spacing_mut().item_spacing = egui::vec2(spacing, spacing);
            
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(i18n::t("app_title")).strong().color(egui::Color32::from_gray(100)));
                
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                     egui::ComboBox::from_id_salt("lang_select")
                        .selected_text(if i18n::get_language() == i18n::Language::Zh { "中文" } else { "English" })
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(i18n::get_language() == i18n::Language::Zh, "中文").clicked() {
                                i18n::set_language(i18n::Language::Zh);
                            }
                            if ui.selectable_label(i18n::get_language() == i18n::Language::En, "English").clicked() {
                                i18n::set_language(i18n::Language::En);
                            }
                        });

                     if ui.button(i18n::t("open_file")).clicked() {
                        #[cfg(not(target_arch = "wasm32"))]
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            self.load_file_native(path);
                        }
                        #[cfg(target_arch = "wasm32")]
                        self.pick_file_web();
                    }
                });
            });
            ui.separator();

            match &current_state_clone_for_display { // Use reference to cloned state
                AppState::Idle => {
                    ui.centered_and_justified(|ui| {
                        ui.label(egui::RichText::new(i18n::t("drag_drop")).heading().color(egui::Color32::GRAY));
                    });
                }
                AppState::Loading => {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.label(i18n::t("reading"));
                        });
                    });
                }
                AppState::Analyzing(data, _settings) => { // Capture settings as well
                     ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.label(i18n::t("detecting"));
                            ui.small(format!("{} ({}Hz)", 
                                data.title.as_deref().unwrap_or("Unknown"), 
                                data.sample_rate));
                        });
                    });
                }
                AppState::Ready(data, analysis_result) => {
                    ui.columns(2, |columns| {
                        columns[0].vertical(|ui| {
                            self.render_player_ui(ui, data, analysis_result); // Pass references
                        });
                        columns[1].vertical(|ui| {
                            ui.heading(i18n::t("analysis_settings"));
                            ui.separator();

                            if ui.radio_value(&mut self.analysis_settings.detection_mode, DetectionMode::Auto, i18n::t("detection_mode_auto")).changed() { re_analyze_triggered_by_ui = true; }
                            if ui.radio_value(&mut self.analysis_settings.detection_mode, DetectionMode::LoopOnly, i18n::t("detection_mode_loop_only")).changed() { re_analyze_triggered_by_ui = true; }
                            if ui.radio_value(&mut self.analysis_settings.detection_mode, DetectionMode::FadeOutOnly, i18n::t("detection_mode_fade_out_only")).changed() { re_analyze_triggered_by_ui = true; }

                            ui.add_space(10.0);
                            ui.label(i18n::t("fade_out_mode"));
                            if ui.radio_value(&mut self.analysis_settings.fade_out_mode, FadeOutMode::Auto, i18n::t("fade_out_mode_auto")).changed() { re_analyze_triggered_by_ui = true; }
                            if ui.radio_value(&mut self.analysis_settings.fade_out_mode, FadeOutMode::None, i18n::t("fade_out_mode_none")).changed() { re_analyze_triggered_by_ui = true; }
                            // Removed FadeOutMode::Only from here as it overlaps with DetectionMode::FadeOutOnly logic.
                            
                            ui.add_space(10.0);
                            ui.label(format!("{}: {:.2}", i18n::t("fade_out_threshold_volume"), self.analysis_settings.fade_out_threshold_volume));
                            if ui.add(egui::Slider::new(&mut self.analysis_settings.fade_out_threshold_volume, 0.0..=0.5)).changed() { re_analyze_triggered_by_ui = true; }

                            ui.label(format!("{}: {}ms", i18n::t("fade_out_window_size"), self.analysis_settings.fade_out_window_size_ms));
                            if ui.add(egui::Slider::new(&mut self.analysis_settings.fade_out_window_size_ms, 10..=200)).changed() { re_analyze_triggered_by_ui = true; }

                            ui.label(format!("{}: {}ms", i18n::t("min_fade_out_duration"), self.analysis_settings.min_fade_out_duration_ms));
                            if ui.add(egui::Slider::new(&mut self.analysis_settings.min_fade_out_duration_ms, 100..=5000)).changed() { re_analyze_triggered_by_ui = true; }

                            ui.add_space(20.0);
                            if ui.button(i18n::t("re_analyze")).clicked() { re_analyze_triggered_by_ui = true; }
                        });
                    });
                }
                AppState::Exporting => { /* ... */ }
                AppState::ExportSuccess => { /* ... */ }
                AppState::ExportError(_e) => {
                     ui.centered_and_justified(|ui| {
                        ui.colored_label(egui::Color32::RED, format!("{}{}", i18n::t("export_fail"), _e));
                    });
                }
                AppState::Error(_e) => {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(egui::Color32::RED, format!("Error: {}", _e));
                    });
                }
            }
            
             if !ctx.input(|i| i.raw.dropped_files.is_empty()) {
                let dropped = ctx.input(|i| i.raw.dropped_files.clone());
                if let Some(file) = dropped.first() {
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(path) = &file.path {
                        self.load_file_native(path.clone());
                    }
                    
                    #[cfg(target_arch = "wasm32")]
                    if let Some(bytes) = &file.bytes {
                        let data_bytes = bytes.to_vec();
                        let name = file.name.clone();
                        let sender = self.msg_sender.clone();
                        let ctx = self.ctx.clone();
                        let analysis_settings = self.analysis_settings.clone();
                        
                         wasm_bindgen_futures::spawn_local(async move {
                            let hint = name.split('.').last().map(|s| s.to_string());
                            match audio::load_audio_from_bytes(data_bytes, hint.as_deref()) {
                                Ok(audio_data) => {
                                     let arc_data = Arc::new(audio_data);
                                     sender.send(AppMessage::Loaded(name, arc_data.clone())).ok();
                                     ctx.request_repaint();
                                     let result = analysis::run_analysis(&arc_data, &analysis_settings); // Pass settings
                                     sender.send(AppMessage::Analyzed(result)).ok();
                                     ctx.request_repaint();
                                }
                                Err(e) => {
                                    sender.send(AppMessage::Error(e.to_string())).ok();
                                    ctx.request_repaint();
                                }
                            }
                         });
                    }
                }
            }
        });

        // If analysis settings changed, re-run analysis
        if re_analyze_triggered_by_ui {
            let current_app_state = self.state.lock().unwrap().clone(); // Acquire lock, clone AppState, then drop MutexGuard immediately
            if let AppState::Ready(data_to_re_analyze, _current_result) = current_app_state {
                // Now self.state is no longer borrowed, so we can mutably borrow self
                self.trigger_analysis(data_to_re_analyze);
            }
        }
    }
}
#[cfg(target_arch = "wasm32")]
async fn download_bytes_as_file(filename: String, bytes: Vec<u8>) -> anyhow::Result<()> {
    let window = web_sys::window().ok_or_else(|| anyhow::anyhow!("No window"))?;
    let document = window.document().ok_or_else(|| anyhow::anyhow!("No document"))?;

    let array_buffer = js_sys::Uint8Array::from(bytes.as_slice());
    let blob_parts = js_sys::Array::new_with_length(1);
    blob_parts.set(0, array_buffer.into());

    let blob_property_bag = web_sys::BlobPropertyBag::new();
    blob_property_bag.set_type("audio/wav"); // Use set_type()

    let blob = web_sys::Blob::new_with_buffer_source_sequence_and_options(
        &blob_parts,
        &blob_property_bag,
    ).map_err(|e| anyhow::anyhow!("Failed to create Blob: {:?}", e))?; // Handle JsValue error

    let url = Url::create_object_url_with_blob(&blob)
        .map_err(|e| anyhow::anyhow!("Failed to create object URL: {:?}", e))?; // Handle JsValue error

    let a = document.create_element("a")
        .map_err(|e| anyhow::anyhow!("Failed to create <a> element: {:?}", e))?; // Handle JsValue error
    let html_element: HtmlElement = a.dyn_into()
        .map_err(|e| anyhow::anyhow!("Failed to cast element to HtmlElement: {:?}", e))?; // Handle Element error
    html_element.set_attribute("download", &filename)
        .map_err(|e| anyhow::anyhow!("Failed to set download attribute: {:?}", e))?; // Handle JsValue error
    html_element.set_attribute("href", &url)
        .map_err(|e| anyhow::anyhow!("Failed to set href attribute: {:?}", e))?; // Handle JsValue error

    html_element.click();

    // Revoke the object URL to free up memory
    Url::revoke_object_url(&url)
        .map_err(|e| anyhow::anyhow!("Failed to revoke object URL: {:?}", e))?; // Handle JsValue error

    Ok(())
}

