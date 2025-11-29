use std::time::Duration;
use rodio::Source;
use crate::audio::AudioData;
use crate::LoopPoints;

pub struct LoopingSource {
    data: AudioData,
    loop_points: LoopPoints,
    cursor: usize,
    loop_count: u32,
    max_loops: Option<u32>, // None means infinite
}

impl LoopingSource {
    pub fn new(data: AudioData, loop_points: LoopPoints, max_loops: Option<u32>) -> Self {
        Self {
            data,
            loop_points,
            cursor: 0,
            loop_count: 0,
            max_loops,
        }
    }
}

impl Iterator for LoopingSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.data.samples.len() {
            return None;
        }

        let sample = self.data.samples[self.cursor];
        self.cursor += 1;

        // Check loop condition
        let should_loop = match self.max_loops {
            Some(max) => self.loop_count < max,
            None => true, // Infinite
        };

        if should_loop {
             if self.cursor >= self.loop_points.end_sample {
                 // Jump back
                 // Ensure we align to channel count just in case
                 let align = self.cursor % self.data.channels as usize;
                 if align == 0 {
                     self.cursor = self.loop_points.start_sample;
                     self.loop_count += 1;
                 }
             }
        }

        Some(sample)
    }
}

impl Source for LoopingSource {
    fn current_frame_len(&self) -> Option<usize> {
        None // Infinite or unknown
    }

    fn channels(&self) -> u16 {
        self.data.channels
    }

    fn sample_rate(&self) -> u32 {
        self.data.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}