use anyhow::Result;
use std::path::Path;
use crate::audio::AudioData;
use crate::LoopPoints;
use crate::player::LoopingSource;

pub fn export_loop<P: AsRef<Path>>(
    output_path: P,
    data: AudioData,
    points: LoopPoints,
    loops: u32
) -> Result<()> {
    let spec = hound::WavSpec {
        channels: data.channels,
        sample_rate: data.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    
    let mut writer = hound::WavWriter::create(output_path, spec)?;
    
    // Create a temporary source just for exporting
    // We use Some(loops) because export cannot be infinite
    let source = LoopingSource::new(data, points, Some(loops));
    
    for sample in source {
        writer.write_sample(sample)?;
    }
    
    writer.finalize()?;
    Ok(())
}
