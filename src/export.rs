use anyhow::Result;
use std::io::Cursor;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use crate::audio::AudioData;
use crate::{LoopPoints, FadeOutInfo}; // Add FadeOutInfo here
use crate::player::LoopingSource;

fn export_loop_internal(data: AudioData, points: LoopPoints, loops: u32, fade_out_info: Option<FadeOutInfo>) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: data.channels,
        sample_rate: data.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut samples_to_export: Vec<f32> = Vec::new();

    // Create a temporary source just for exporting
    // We use Some(loops) because export cannot be infinite
    let source = LoopingSource::new(data, points, Some(loops), fade_out_info.clone()); // Pass fade_out_info

    for sample in source {
        samples_to_export.push(sample);
    }

    // Apply fade-out if detected and requested
    if let Some(fo_info) = fade_out_info {
        let actual_exported_len = samples_to_export.len();
        
        // Calculate fade-out start for the exported audio, ensuring it doesn't go below 0
        let fade_start_in_exported_samples = actual_exported_len.saturating_sub(fo_info.duration_samples);
        
        for i in fade_start_in_exported_samples..actual_exported_len {
            let relative_index = i - fade_start_in_exported_samples;
            let fade_factor = 1.0 - (relative_index as f32 / fo_info.duration_samples as f32);
            samples_to_export[i] *= fade_factor.max(0.0).min(1.0); // Apply linear fade-out
        }
    }

    let mut buffer = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut buffer, spec)?;

    for sample in samples_to_export {
        writer.write_sample(sample)?;
    }

    writer.finalize()?;
    Ok(buffer.into_inner())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn export_loop<P: AsRef<Path>>(
    output_path: P,
    data: AudioData,
    points: LoopPoints,
    loops: u32,
    fade_out_info: Option<FadeOutInfo>, // New parameter
) -> Result<()> {
    let wav_data = export_loop_internal(data, points, loops, fade_out_info)?;
    std::fs::write(output_path, wav_data)?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub fn export_loop_web(data: AudioData, points: LoopPoints, loops: u32, fade_out_info: Option<FadeOutInfo>) -> Result<Vec<u8>> {
    export_loop_internal(data, points, loops, fade_out_info)
}
