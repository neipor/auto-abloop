use clap::{Parser};
use std::path::PathBuf;
use auto_abloop::{audio, analysis, player, export, gui, LoopPoints};
use rodio::{OutputStream, Sink};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Input file (if not provided, opens GUI)
    input: Option<PathBuf>,

    /// Output file to export loop
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Number of times to loop (default: Infinite, unless exporting)
    #[arg(short, long)]
    loops: Option<u32>,

    /// Force GUI mode even if input provided (optional)
    #[arg(long)]
    gui: bool,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    if cli.input.is_none() || cli.gui {
        // Launch GUI
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
            auto_abloop::LoopPoints { start_sample: 0, end_sample: audio_data.samples.len(), confidence: 0.0 }
        }
    };

    // Handle Export
    if let Some(output_path) = cli.output {
        let loop_count = cli.loops.unwrap_or(5); // Default 5 for export if not specified
        println!("Exporting to {:?} with {} loops...", output_path, loop_count);
        export::export_loop(&output_path, audio_data, points, loop_count)?;
        println!("Export complete.");
    } else {
        // Playback
        println!("Playing... (Ctrl+C to stop)");
        let (_stream, stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&stream_handle)?;
        
        // Default to infinite if not specified, unless user explicitly gave a number
        let max_loops = cli.loops; 
        
        let source = player::LoopingSource::new(audio_data.clone(), points, max_loops);
        
        sink.append(source);
        sink.sleep_until_end();
    }

    Ok(())
}
