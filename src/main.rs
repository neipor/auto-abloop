use auto_abloop::{audio, analysis, player, export, gui, LoopPoints};
use rodio::{OutputStream, Sink};

// --- Native (CLI/Desktop) Entry Point ---
#[cfg(not(target_arch = "wasm32"))]
use clap::Parser;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    input: Option<PathBuf>,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long)]
    loops: Option<u32>,
    #[arg(long)]
    gui: bool,
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    if cli.input.is_none() || cli.gui {
        return gui::run(cli.input);
    }

    let input_path = cli.input.unwrap();
    println!("Loading audio: {:?}", input_path);

    let audio_data = audio::load_audio_file(&input_path)?;
    println!("Audio loaded. Sample rate: {}, Channels: {}", audio_data.sample_rate, audio_data.channels);

    println!("Detecting loop points...");
    let loop_points = analysis::detect_loop(&audio_data);

    let points = match loop_points {
        Some(p) => {
            println!("Loop detected!");
            println!("Start sample: {}", p.start_sample);
            println!("End sample: {}", p.end_sample);
            println!("Confidence: {:.2}", p.confidence);
            p
        },
        None => {
            println!("No clear loop detected. Playing normally.");
            LoopPoints { start_sample: 0, end_sample: audio_data.samples.len(), confidence: 0.0 }
        }
    };

    if let Some(output_path) = cli.output {
        let loop_count = cli.loops.unwrap_or(5);
        println!("Exporting to {:?} with {} loops...", output_path, loop_count);
        export::export_loop(&output_path, audio_data, points, loop_count)?;
        println!("Export complete.");
    } else {
        println!("Playing... (Ctrl+C to stop)");
        let (_stream, stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&stream_handle)?;
        let max_loops = cli.loops; 
        let source = player::LoopingSource::new(audio_data.clone(), points, max_loops);
        sink.append(source);
        sink.sleep_until_end();
    }

    Ok(())
}

// --- WASM Entry Point ---
#[cfg(target_arch = "wasm32")]
fn main() {
    // Make sure panics are logged using console.error
    console_error_panic_hook::set_once();
    
    // Redirect tracing to console.log
    tracing_wasm::set_as_global_default();

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        use eframe::wasm_bindgen::JsCast;
        let document = web_sys::window().expect("No window").document().expect("No document");
        let canvas = document.get_element_by_id("the_canvas_id")
            .expect("Failed to find the_canvas_id")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("the_canvas_id was not a HtmlCanvasElement");

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
            .expect("failed to start eframe");
    });
}