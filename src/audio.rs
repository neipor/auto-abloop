use anyhow::{Context, Result};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::{MetadataOptions, StandardTagKey};
use symphonia::core::probe::Hint;

use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use image::DynamicImage;

#[derive(Clone)]
pub struct AudioData {
    pub samples: Vec<f32>, // Interleaved samples
    pub sample_rate: u32,
    pub channels: u16,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover_art: Option<std::sync::Arc<DynamicImage>>, // Arc to make AudioData cheap to clone
}

// Core loading function that takes a generic MediaSource
pub fn load_audio_from_source(source: Box<dyn MediaSource>, hint: &Hint) -> Result<AudioData> {
    let mss = MediaSourceStream::new(source, Default::default());

    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();

    let mut probed = symphonia::default::get_probe()
        .format(hint, mss, &fmt_opts, &meta_opts)
        .context("unsupported format")?;

    // Extract Metadata
    let mut title = None;
    let mut artist = None;
    let mut album = None;
    let mut cover_art = None;

    if let Some(metadata) = probed.format.metadata().current() {
        for tag in metadata.tags() {
            match tag.std_key {
                Some(StandardTagKey::TrackTitle) => title = Some(tag.value.to_string()),
                Some(StandardTagKey::Artist) => artist = Some(tag.value.to_string()),
                Some(StandardTagKey::Album) => album = Some(tag.value.to_string()),
                _ => (),
            }
        }
        
        // Visuals
        if let Some(visual) = metadata.visuals().first() {
             if let Ok(img) = image::load_from_memory(&visual.data) {
                 cover_art = Some(std::sync::Arc::new(img));
             }
        }
    }

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .context("no supported audio track")?;

    let dec_opts: DecoderOptions = Default::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &dec_opts)
        .context("unsupported codec")?;

    let track_id = track.id;
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = 0;
    let mut channels = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(_)) => break,
            Err(Error::ResetRequired) => break, 
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                if sample_rate == 0 {
                    let spec = decoded.spec();
                    sample_rate = spec.rate;
                    channels = spec.channels.count() as u16;
                }
                
                match decoded {
                    AudioBufferRef::F32(buf) => {
                        for i in 0..buf.frames() {
                            for c in 0..buf.spec().channels.count() {
                                samples.push(buf.chan(c)[i]);
                            }
                        }
                    }
                    AudioBufferRef::F64(buf) => {
                        for i in 0..buf.frames() {
                            for c in 0..buf.spec().channels.count() {
                                samples.push(buf.chan(c)[i] as f32);
                            }
                        }
                    }
                    AudioBufferRef::U8(buf) => {
                        for i in 0..buf.frames() {
                            for c in 0..buf.spec().channels.count() {
                                samples.push((buf.chan(c)[i] as f32 / 128.0) - 1.0);
                            }
                        }
                    }
                    AudioBufferRef::U16(buf) => {
                        for i in 0..buf.frames() {
                            for c in 0..buf.spec().channels.count() {
                                samples.push((buf.chan(c)[i] as f32 / 32768.0) - 1.0);
                            }
                        }
                    }
                    AudioBufferRef::U24(buf) => {
                        for i in 0..buf.frames() {
                             for c in 0..buf.spec().channels.count() {
                                samples.push(buf.chan(c)[i].0 as f32 / 8388608.0);
                            }
                        }
                    }
                    AudioBufferRef::U32(buf) => {
                        for i in 0..buf.frames() {
                            for c in 0..buf.spec().channels.count() {
                                samples.push((buf.chan(c)[i] as f32 / 2147483648.0) - 1.0);
                            }
                        }
                    }
                    AudioBufferRef::S8(buf) => {
                         for i in 0..buf.frames() {
                            for c in 0..buf.spec().channels.count() {
                                samples.push(buf.chan(c)[i] as f32 / 128.0);
                            }
                        }
                    }
                    AudioBufferRef::S16(buf) => {
                        for i in 0..buf.frames() {
                            for c in 0..buf.spec().channels.count() {
                                samples.push(buf.chan(c)[i] as f32 / 32768.0);
                            }
                        }
                    }
                    AudioBufferRef::S24(buf) => {
                        for i in 0..buf.frames() {
                            for c in 0..buf.spec().channels.count() {
                                samples.push(buf.chan(c)[i].0 as f32 / 8388608.0);
                            }
                        }
                    }
                    AudioBufferRef::S32(buf) => {
                        for i in 0..buf.frames() {
                            for c in 0..buf.spec().channels.count() {
                                samples.push(buf.chan(c)[i] as f32 / 2147483648.0);
                            }
                        }
                    }
                }
            }
            Err(Error::DecodeError(_)) => (),
            Err(_) => break,
        }
    }

    Ok(AudioData {
        samples,
        sample_rate,
        channels,
        title,
        artist,
        album,
        cover_art,
    })
}

pub fn load_audio_file<P: AsRef<Path>>(path: P) -> Result<AudioData> {
    let src = File::open(&path).context("failed to open audio file")?;
    
    let mut hint = Hint::new();
    if let Some(ext) = path.as_ref().extension() {
        if let Some(ext_str) = ext.to_str() {
            hint.with_extension(ext_str);
        }
    }
    
    load_audio_from_source(Box::new(src), &hint)
}

pub fn load_audio_from_bytes(data: Vec<u8>, extension_hint: Option<&str>) -> Result<AudioData> {
    let src = Cursor::new(data);
    
    let mut hint = Hint::new();
    if let Some(ext) = extension_hint {
        hint.with_extension(ext);
    }
    
    load_audio_from_source(Box::new(src), &hint)
}