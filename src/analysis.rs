use crate::audio::AudioData;
use crate::{LoopPoints, FadeOutInfo, AnalysisSettings, DetectionMode, FadeOutMode, AnalysisResult};

pub fn detect_loop(audio: &AudioData, fade_out_hint: Option<&FadeOutInfo>) -> Option<LoopPoints> {
    let channels = audio.channels as usize;
    
    // 1. Mix to mono
    let mono: Vec<f32> = audio.samples
        .chunks_exact(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
        .collect();

    // 2. Find effective end (ignore trailing silence)
    // Scan backwards for significant energy
    let silence_threshold = 0.0005; // Very quiet
    let mut actual_end_of_content_mono = mono.len(); // Renamed end_idx for clarity
    for (i, &sample) in mono.iter().enumerate().rev() {
        if sample.abs() > silence_threshold {
            actual_end_of_content_mono = i + 1;
            break;
        }
    }
    
    // Adjust the effective end based on fade_out_hint
    let mut loop_search_end_mono = actual_end_of_content_mono;
    if let Some(fo_info) = fade_out_hint {
        // Convert multi-channel fade-out start to mono index
        let fo_start_mono = fo_info.start_sample / channels;
        // The loop search should not go into the fade-out region
        loop_search_end_mono = loop_search_end_mono.min(fo_start_mono);
    }

    if loop_search_end_mono < (audio.sample_rate as f32 * 5.0) as usize { // Too short (< 5s) after adjustment
        return None;
    }

    // 3. Prepare Query (End of track for loop search)
    // Use a 10-second window, or smaller if file is short
    let search_window_sec = 10.0;
    let window_size = (audio.sample_rate as f32 * search_window_sec) as usize;
    
    if loop_search_end_mono < window_size * 2 {
        // Try smaller window for short files?
        return None; 
    }

    // Shift query back slightly to avoid catching the very tail of a fade which might be noise
    // Let's take the block ending at loop_search_end_mono.
    let query_start_idx = loop_search_end_mono - window_size;
    let query_raw = &mono[query_start_idx..loop_search_end_mono];

    // 4. Coarse Search (Downsample)
    // Target ~200Hz for coarse search. 
    // 50Hz was too low (lost transients), 1000Hz is too slow for brute force NCC.
    let target_rate = 200; 
    let coarse_step = (audio.sample_rate as usize / target_rate).max(1);
    
    let coarse_query = downsample(query_raw, coarse_step);
    let coarse_search_space = downsample(&mono[0..query_start_idx], coarse_step);
    
    // Relaxed threshold to 0.3 due to potential fade/differences
    let (best_coarse_idx, coarse_corr) = find_best_match_ncc(&coarse_query, &coarse_search_space);
    
    if coarse_corr < 0.3 { // Very low confidence
        return None;
    }

    // 5. Fine Search
    let estimated_pos = best_coarse_idx * coarse_step;
    let refine_radius = (audio.sample_rate as usize) * 4; // +/- 4 seconds
    let refine_start = estimated_pos.saturating_sub(refine_radius);
    let refine_end = (estimated_pos + refine_radius).min(query_start_idx - window_size);
    
    if refine_end <= refine_start {
         return None;
    }

    // Use step 1 for maximum precision now
    let _fine_step = 1; 
    let refine_slice_raw = &mono[refine_start..refine_end + window_size];
    
    // Medium pass: Target ~2kHz for alignment
    let medium_target = 2000;
    let medium_step = (audio.sample_rate as usize / medium_target).max(1);
    
    let medium_query = downsample(query_raw, medium_step);
    let medium_search_slice = downsample(refine_slice_raw, medium_step);
    
    let (best_medium_rel, medium_corr) = find_best_match_ncc(&medium_query, &medium_search_slice);
    
    let best_pos = refine_start + best_medium_rel * medium_step;

    // 6. Fade Logic & Final Confidence
    let match_slice = &mono[best_pos..best_pos + window_size];
    let query_rms = calculate_rms(query_raw);
    let match_rms = calculate_rms(match_slice);
    
    let volume_ratio = match_rms / (query_rms + 1e-9); // match_vol / query_vol
    
    let mut confidence = medium_corr;
    
    if volume_ratio > 1.2 {
        if confidence > 0.6 {
            confidence = (confidence + 0.2).min(1.0);
        }
    } else if volume_ratio < 0.8 {
        confidence *= 0.8;
    }
    
    let loop_start_sample = best_pos * channels;
    let loop_end_sample = loop_search_end_mono * channels; // Updated to use loop_search_end_mono

    Some(LoopPoints {
        start_sample: loop_start_sample,
        end_sample: loop_end_sample,
        confidence,
    })
}

fn downsample(data: &[f32], step: usize) -> Vec<f32> {
    if step <= 1 {
        return data.to_vec();
    }
    // Averaging (Box filter) to prevent aliasing
    data.chunks(step)
        .map(|chunk| chunk.iter().sum::<f32>() / chunk.len() as f32)
        .collect()
}

fn calculate_rms(data: &[f32]) -> f32 {
    let sum_sq: f32 = data.iter().map(|x| x * x).sum();
    (sum_sq / (data.len() as f32 + 1e-9)).sqrt()
}

fn find_best_match_ncc(query: &[f32], search_space: &[f32]) -> (usize, f32) {
    let n = query.len();
    if search_space.len() < n {
        return (0, 0.0);
    }

    let query_mean = query.iter().sum::<f32>() / n as f32;
    // Precompute query denominator
    let query_denom = query.iter().map(|x| (x - query_mean).powi(2)).sum::<f32>().sqrt();

    if query_denom < 1e-9 {
        return (0, 0.0); // Silence or flat line
    }

    let mut best_corr = -1.0;
    let mut best_idx = 0;

    // Sliding window NCC
    // To optimize:
    // The search space is processed in windows.
    // NCC(x, y) = sum((x-mx)*(y-my)) / (sx*sy)
    // This is still slow O(N*M) for large searches.
    // But our coarse search is small enough.
    
    for i in 0..=(search_space.len() - n) {
        let candidate = &search_space[i..i+n];
        
        // We can skip if candidate is obviously silent?
        // Maybe not, silence matching silence is valid (though query isn't silence).
        
        let cand_mean = candidate.iter().sum::<f32>() / n as f32;
        let cand_denom = candidate.iter().map(|x| (x - cand_mean).powi(2)).sum::<f32>().sqrt();

        if cand_denom < 1e-9 {
            continue;
        }

        let numer: f32 = query.iter().zip(candidate.iter())
            .map(|(q, c)| (q - query_mean) * (c - cand_mean))
            .sum();

        let corr = numer / (query_denom * cand_denom);

        if corr > best_corr {
            best_corr = corr;
            best_idx = i;
        }
    }

    (best_idx, best_corr)
}

pub fn detect_fade_out(audio: &AudioData, settings: &AnalysisSettings) -> Option<FadeOutInfo> {
    let channels = audio.channels as usize;
    let sample_rate = audio.sample_rate as f32;

    // 1. Mix to mono
    let mono: Vec<f32> = audio.samples
        .chunks_exact(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
        .collect();

    // 2. Find effective end (ignore trailing silence)
    let silence_threshold = 0.0005; // Very quiet
    let mut effective_end_idx = mono.len();
    for (i, &sample) in mono.iter().enumerate().rev() {
        if sample.abs() > silence_threshold {
            effective_end_idx = i + 1;
            break;
        }
    }

    // Don't analyze the very beginning if it's too short
    if effective_end_idx < (sample_rate * 5.0) as usize { // At least 5 seconds of audio
        return None;
    }

    let window_size_samples = (settings.fade_out_window_size_ms as f32 / 1000.0 * sample_rate) as usize;
    if window_size_samples == 0 { return None; } // Prevent division by zero

    let mut last_rms = calculate_rms(&mono[effective_end_idx.saturating_sub(window_size_samples)..effective_end_idx]);

    // Iterate backwards from the effective end
    // Look for a significant and consistent drop in RMS
    let mut potential_fade_start = None;

    // Scan backwards, stopping if we reach too far into the track (e.g., more than 30 seconds from end)
    let max_scan_samples = (sample_rate * 30.0) as usize; // Max 30 seconds for fade-out detection
    let scan_start_idx = effective_end_idx.saturating_sub(max_scan_samples).max(window_size_samples);


    for i in (scan_start_idx..effective_end_idx - window_size_samples).rev() {
        let current_window = &mono[i..i + window_size_samples];
        let current_rms = calculate_rms(current_window);

        // Check for consistent drop in volume
        // The current RMS should be lower than the previous (further towards the end of the track)
        // AND the current RMS should be above the threshold to be considered part of the "fading" signal
        if current_rms < last_rms * 0.95 && current_rms > settings.fade_out_threshold_volume { // 5% drop, but still audible
            if potential_fade_start.is_none() {
                potential_fade_start = Some(i + window_size_samples); // Mark this as a potential start
            }
        } else if potential_fade_start.is_some() {
            // If we were in a fade and now the condition is broken, then the fade ended here
            break;
        }
        
        last_rms = current_rms;

        // If current RMS is already below threshold, we've gone too far into silence for a fade
        if current_rms < settings.fade_out_threshold_volume && potential_fade_start.is_some() {
             // Removed unused assignment
             break;
        } else if current_rms < settings.fade_out_threshold_volume && potential_fade_start.is_none() {
            // Just silence, not a fade
            continue;
        }
    }

    if let Some(start) = potential_fade_start {
        let fade_duration_samples = effective_end_idx - start;
        let min_fade_duration_samples = (settings.min_fade_out_duration_ms as f32 / 1000.0 * sample_rate) as usize;

        if fade_duration_samples >= min_fade_duration_samples {
            // Confidence can be based on how consistent the drop was, or just a default high value if detected
            let confidence = 1.0; // Placeholder for now, can be refined
            return Some(FadeOutInfo {
                start_sample: start * channels, // Convert mono sample index back to multi-channel
                duration_samples: fade_duration_samples * channels, // Convert mono duration to multi-channel
                confidence,
            });
        }
    }

    None
}

pub fn run_analysis(audio: &AudioData, settings: &AnalysisSettings) -> AnalysisResult {
    let mut result = AnalysisResult::default();

    // Step 1: Handle Fade-Out Detection based on settings
    let detected_fade_out = if settings.fade_out_mode == FadeOutMode::None {
        None
    } else {
        detect_fade_out(audio, settings)
    };
    result.fade_out_info = detected_fade_out;

    // Step 2: Handle Loop Detection based on settings
    match settings.detection_mode {
        DetectionMode::FadeOutOnly => {
            // Loop detection is skipped in this mode.
        }
        DetectionMode::LoopOnly | DetectionMode::Auto => {
            let mut loop_points = detect_loop(audio, result.fade_out_info.as_ref());

            // If a fade-out was detected AND we have loop points,
            // adjust the loop_end_sample to not include the fade-out region.
            if let Some(fo_info) = &result.fade_out_info {
                if let Some(lp) = &mut loop_points {
                    let fade_out_start_multichannel = fo_info.start_sample;
                    
                    // If the detected loop's end overlaps with the fade-out,
                    // cut the loop's end just before the fade-out starts.
                    if lp.end_sample > fade_out_start_multichannel {
                        let buffer_samples = (settings.fade_out_buffer_ms as f32 / 1000.0 * audio.sample_rate as f32 * audio.channels as f32) as usize;
                        lp.end_sample = fade_out_start_multichannel.saturating_sub(buffer_samples).max(lp.start_sample + 1); // Ensure loop is at least 1 sample long
                        // It's reasonable to assume the confidence for the 'loop' itself
                        // might still be high, even if its end point is adjusted due to fade-out.
                        // We could re-evaluate confidence, but for now, we'll keep the original.
                    }
                    // Also, if the loop's start is within the fade-out, this indicates a bad loop detection
                    // or a very short track. For now, we'll just allow the end_sample adjustment.
                }
            }
            result.loop_points = loop_points;
        }
    }

    result
}
