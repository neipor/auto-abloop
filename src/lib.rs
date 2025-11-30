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

#[derive(Clone, Debug, PartialEq)]
pub struct FadeOutInfo {
    pub start_sample: usize, // Sample where fade-out effectively begins
    pub duration_samples: usize, // Duration of the fade-out in samples
    pub confidence: f32, // Confidence of the fade-out detection
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum DetectionMode {
    Auto,
    LoopOnly,
    FadeOutOnly,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum FadeOutMode {
    Auto, // Automatically determine if fade-out exists and its duration
    None, // Do not apply any fade-out
    Only, // Only detect fade-out, ignore loop
}

#[derive(Clone, Debug)]
pub struct AnalysisSettings {
    pub detection_mode: DetectionMode,
    pub fade_out_mode: FadeOutMode,
    pub fade_out_threshold_volume: f32, // Volume threshold to consider as start of fade-out
    pub fade_out_window_size_ms: u32, // Window size in milliseconds for RMS calculation during fade-out detection
    pub min_fade_out_duration_ms: u32, // Minimum duration for a fade-out to be considered valid
    pub fade_out_buffer_ms: u32, // New: Small buffer before fade-out adjustment to ensure no audible fade is included in loop
    // Add any other settings that might be relevant later, e.g., for loop detection sensitivity
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            detection_mode: DetectionMode::Auto,
            fade_out_mode: FadeOutMode::Auto,
            fade_out_threshold_volume: 0.1, // A reasonable default, 10% of max volume
            fade_out_window_size_ms: 50, // 50ms window
            min_fade_out_duration_ms: 1000, // 1 second minimum fade-out
            fade_out_buffer_ms: 100, // New: 100ms buffer
        }
    }
}

#[derive(Clone, Debug, Default)] // Default is useful for initializing
pub struct AnalysisResult {
    pub loop_points: Option<LoopPoints>,
    pub fade_out_info: Option<FadeOutInfo>,
}