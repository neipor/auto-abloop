pub mod audio;
pub mod analysis;
pub mod player;
pub mod export;
pub mod i18n;
pub mod gui; 

#[derive(Clone, Debug)]
pub struct LoopPoints {
    pub start_sample: usize,
    pub end_sample: usize,
    pub confidence: f32,
}

#[cfg(target_arch = "wasm32")]
use eframe::wasm_bindgen::{self, prelude::*};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub async fn start(canvas_id: &str) -> Result<(), eframe::wasm_bindgen::JsValue> {
    use eframe::wasm_bindgen::JsCast; // Needed for dyn_into

    std::panic::set_hook(Box::new(console_error_panic_hook::hook));
    tracing_wasm::set_as_global_default();

    let document = web_sys::window().expect("No window").document().expect("No document");
    let canvas = document.get_element_by_id(canvas_id)
        .expect("Canvas not found")
        .dyn_into::<web_sys::HtmlCanvasElement>()?;

    let web_options = eframe::WebOptions::default();
    
    eframe::WebRunner::new()
        .start(
            canvas,
            web_options,
            Box::new(|cc| {
                gui::configure_visuals(&cc.egui_ctx);
                Ok(Box::new(gui::MyApp::new(None, cc.egui_ctx.clone())))
            }),
        )
        .await
}
