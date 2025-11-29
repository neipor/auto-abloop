use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use crossbeam_channel::{unbounded, Receiver, Sender};
use crate::{audio, analysis, player, export, i18n, LoopPoints};
use rodio::{OutputStream, Sink};

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

pub fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_gray(20);
    ctx.set_visuals(visuals);
}

#[derive(Clone)]
enum AppState {
    Idle,
    Loading,
    Analyzing(Arc<audio::AudioData>), 
    Ready(Arc<audio::AudioData>, Option<LoopPoints>),
    Error(String),
    Exporting,
    ExportSuccess,
    ExportError(String),
}

enum AppMessage {
    Loaded(String, Arc<audio::AudioData>), 
    Analyzed(Option<LoopPoints>),
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
    
    // UI State
    loop_count: u32,
    infinite_loop: bool,
    file_name: Option<String>,
    
    // Controls
    volume: f32, 
    cover_texture: Option<egui::TextureHandle>,
    
    // Visualization
    waveform_cache: Option<Vec<f32>>,
    
    // Export
    export_loops: u32,
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
        };

        // Initialize Audio 
        if let Ok((stream, stream_handle)) = OutputStream::try_default() {
             let sink = Sink::try_new(&stream_handle).ok();
             if let Some(s) = &sink {
                 s.set_volume(app.volume);
             }
             app._stream = Some(stream);
             app.sink = sink;
        }
        
        // Initial file load (Native only)
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = initial_file {
            app.load_file_native(path);
        }

        app
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
        
        thread::spawn(move || {
            match audio::load_audio_file(&path) {
                Ok(data) => {
                    let arc_data = Arc::new(data);
                    sender.send(AppMessage::Loaded(name, arc_data.clone())).ok();
                    ctx.request_repaint();
                    
                    let points = analysis::detect_loop(&arc_data);
                    sender.send(AppMessage::Analyzed(points)).ok();
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
                         
                         // Sync analysis on main thread 
                         let points = analysis::detect_loop(&arc_data);
                         sender.send(AppMessage::Analyzed(points)).ok();
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
        if let AppState::Ready(data, points) = &*state {
            if let Some(sink) = &self.sink {
                sink.stop(); 
                let lp = points.clone().unwrap_or(LoopPoints { start_sample: 0, end_sample: data.samples.len(), confidence: 0.0 });
                let max_loops = if self.infinite_loop { None } else { Some(self.loop_count) };
                let source = player::LoopingSource::new((**data).clone(), lp, max_loops);
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
         if let AppState::Ready(data, points) = &*state_guard {
             let data = data.clone();
             let points = points.clone().unwrap_or(LoopPoints { start_sample: 0, end_sample: data.samples.len(), confidence: 0.0 });
             let loops = self.export_loops;
             
             drop(state_guard);
             
             // Native Export
             #[cfg(not(target_arch = "wasm32"))]
             {
                 if let Some(path) = rfd::FileDialog::new().set_file_name("loop_export.wav").save_file() {
                     *self.state.lock().unwrap() = AppState::Exporting;
                     let sender = self.msg_sender.clone();
                     let ctx = self.ctx.clone();
                     thread::spawn(move || {
                         let res = export::export_loop(&path, (*data).clone(), points, loops);
                         sender.send(AppMessage::ExportFinished(res)).ok();
                         ctx.request_repaint();
                     });
                 }
             }
             
             // Web Export (Stub)
             #[cfg(target_arch = "wasm32")]
             {
                 let sender = self.msg_sender.clone();
                 let ctx = self.ctx.clone();
                 wasm_bindgen_futures::spawn_local(async move {
                     let _cursor = std::io::Cursor::new(Vec::<u8>::new());
                     sender.send(AppMessage::Error("Export not fully ported to Web yet".into())).ok();
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
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.msg_receiver.try_recv() {
             let mut state = self.state.lock().unwrap();
            match msg {
                AppMessage::Loaded(name, data) => {
                    self.file_name = Some(name);
                    drop(state);
                    self.generate_waveform(&data);
                    
                    let mut state = self.state.lock().unwrap();
                    *state = AppState::Analyzing(data.clone());
                    
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
                }
                AppMessage::Analyzed(points) => {
                     match &*state {
                         AppState::Analyzing(data) => {
                             *state = AppState::Ready(data.clone(), points);
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
        
        let current_state = self.state.lock().unwrap().clone();

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

            match current_state {
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
                AppState::Analyzing(data) => {
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
                AppState::Ready(data, points) => {
                    self.render_player_ui(ui, &data, points.as_ref());
                }
                AppState::Exporting => {
                     ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.label(i18n::t("exporting"));
                        });
                    });
                }
                AppState::ExportSuccess => {
                    ui.centered_and_justified(|ui| {
                         ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new(i18n::t("export_success")).color(egui::Color32::GREEN).size(20.0));
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
                }
                AppState::ExportError(e) => {
                     ui.centered_and_justified(|ui| {
                        ui.colored_label(egui::Color32::RED, format!("{}{}", i18n::t("export_fail"), e));
                    });
                }
                AppState::Error(e) => {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(egui::Color32::RED, format!("Error: {}", e));
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
                        
                         wasm_bindgen_futures::spawn_local(async move {
                            let hint = name.split('.').last().map(|s| s.to_string());
                            match audio::load_audio_from_bytes(data_bytes, hint.as_deref()) {
                                Ok(audio_data) => {
                                     let arc_data = Arc::new(audio_data);
                                     sender.send(AppMessage::Loaded(name, arc_data.clone())).ok();
                                     ctx.request_repaint();
                                     let points = analysis::detect_loop(&arc_data);
                                     sender.send(AppMessage::Analyzed(points)).ok();
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
    }
}