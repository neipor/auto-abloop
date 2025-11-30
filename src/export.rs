use anyhow::Result;
use std::io::Cursor;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use crate::audio::AudioData;
use crate::LoopPoints;
use crate::player::LoopingSource;

fn export_loop_internal(data: AudioData, points: LoopPoints, loops: u32) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: data.channels,
        sample_rate: data.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut buffer = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut buffer, spec)?;

    // Create a temporary source just for exporting
    // We use Some(loops) because export cannot be infinite
    let source = LoopingSource::new(data, points, Some(loops));

    for sample in source {
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
    loops: u32
) -> Result<()> {
    let wav_data = export_loop_internal(data, points, loops)?;
    std::fs::write(output_path, wav_data)?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub fn export_loop_web(data: AudioData, points: LoopPoints, loops: u32) -> Result<Vec<u8>> {
    export_loop_internal(data, points, loops)
}
