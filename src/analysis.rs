use crate::audio::AudioData;
use crate::{LoopPoints, FadeOutInfo, AnalysisSettings, DetectionMode, FadeOutMode, AnalysisResult};
use realfft::RealFftPlanner;

// Constants for Loop Detection
const COARSE_SAMPLE_RATE: u32 = 4000;
const QUERY_DURATION_SEC: f32 = 15.0;
const MIN_LOOP_DURATION_SEC: f32 = 10.0;
const SILENCE_THRESHOLD: f32 = 0.001;

pub fn run_analysis(audio: &AudioData, settings: &AnalysisSettings) -> AnalysisResult {
    run_analysis_with_progress(audio, settings, |_| {})
}

pub fn run_analysis_with_progress<F>(audio: &AudioData, settings: &AnalysisSettings, mut progress_callback: F) -> AnalysisResult 
where
    F: FnMut(&str),
{
    let mut result = AnalysisResult::default();

    progress_callback("正在预处理音频...");
    
    // 1. Mix to Mono
    let mono_samples = mix_to_mono(audio);
    let sample_rate = audio.sample_rate;
    let channels = audio.channels as usize;
    
    // 2. Detect Fade Out
    progress_callback("正在检测淡出...");
    let fade_out_info = if settings.fade_out_mode == FadeOutMode::None {
        None
    } else {
        detect_fade_out(&mono_samples, sample_rate, channels, settings)
    };
    result.fade_out_info = fade_out_info.clone();

    // 3. Detect Loop
    match settings.detection_mode {
        DetectionMode::FadeOutOnly => {},
        DetectionMode::LoopOnly | DetectionMode::Auto => {
            progress_callback("正在进行FFT粗略搜索...");
            
            // Determine the effective end of the searchable audio
            let search_end_idx; // Will be initialized
            if let Some(fo) = &fade_out_info {
                let fo_start_mono = fo.start_sample / channels;
                
                // Add a safety buffer
                 let buffer = (settings.fade_out_buffer_ms as f32 / 1000.0 * sample_rate as f32) as usize;
                 search_end_idx = fo_start_mono.saturating_sub(buffer);
            }
            else {
                // If no fade out, trim silence
                search_end_idx = find_effective_end(&mono_samples, SILENCE_THRESHOLD);
            }
            
            if search_end_idx < (sample_rate as f32 * MIN_LOOP_DURATION_SEC) as usize {
                 // Too short to loop
            } else {
                 let loop_points = detect_loop_fft(
                     &mono_samples, 
                     sample_rate, 
                     search_end_idx,
                     channels,
                     &mut progress_callback
                 );
                 
                 // Post-process loop points to ensure they are valid
                 if let Some(mut lp) = loop_points {
                     // Ensure loop end doesn't exceed search_end_idx
                     let max_end = search_end_idx * channels;
                     if lp.end_sample > max_end {
                         lp.end_sample = max_end;
                     }
                     result.loop_points = Some(lp);
                 }
            }
        }
    }
    
    progress_callback("分析完成！");
    result
}

fn mix_to_mono(audio: &AudioData) -> Vec<f32> {
    let channels = audio.channels as usize;
    if channels == 1 {
        return audio.samples.clone();
    }
    audio.samples
        .chunks_exact(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn find_effective_end(mono: &[f32], threshold: f32) -> usize {
    // Scan backwards
    for (i, &sample) in mono.iter().enumerate().rev() {
        if sample.abs() > threshold {
            return i + 1;
        }
    }
    mono.len()
}

fn detect_loop_fft<F>(
    mono: &[f32], 
    sample_rate: u32, 
    search_end_idx: usize,
    original_channels: usize,
    progress_callback: &mut F
) -> Option<LoopPoints> 
where F: FnMut(&str)
{
    // 1. Prepare Query
    let query_len_samples = (sample_rate as f32 * QUERY_DURATION_SEC) as usize;
    if search_end_idx < query_len_samples * 2 {
        return None;
    }
    
    let query_start_idx = search_end_idx - query_len_samples;
    let query_raw = &mono[query_start_idx..search_end_idx];
    
    // 2. Coarse Search (FFT)
    let downsample_factor = (sample_rate / COARSE_SAMPLE_RATE).max(1) as usize;
    
    // Downsample signal and query
    let coarse_signal = downsample(&mono[0..search_end_idx], downsample_factor);
    let coarse_query = downsample(query_raw, downsample_factor);
    
    let best_coarse_lag = find_best_lag_fft(&coarse_signal, &coarse_query)?;
    
    // 3. Fine Search (NCC)
    progress_callback("正在进行精细匹配...");
    let estimated_lag_samples = best_coarse_lag * downsample_factor;
    
    // Search window: +/- 2 seconds around estimated lag
    let refine_radius = (sample_rate * 2) as usize;
    let search_start = estimated_lag_samples.saturating_sub(refine_radius);
    let search_end = (estimated_lag_samples + refine_radius).min(query_start_idx - 1000); 
    
    if search_end <= search_start { return None; }
    
    // We need to match `query_raw` against `mono[search_start..search_end + query_len]`
    // The `find_best_match_ncc_fine` will return offset relative to `search_start`
    
    let (best_rel_offset, correlation) = find_best_match_ncc_fine(
        query_raw, 
        mono, 
        search_start, 
        search_end
    );
    
    if correlation < 0.3 { 
        // If correlation is too low, we fail
        return None; 
    }
    
    let loop_start_sample_mono = search_start + best_rel_offset;
    let loop_end_sample_mono = search_end_idx;

    Some(LoopPoints {
        start_sample: loop_start_sample_mono * original_channels,
        end_sample: loop_end_sample_mono * original_channels,
        confidence: correlation,
    })
}

fn downsample(data: &[f32], step: usize) -> Vec<f32> {
    if step <= 1 {
        return data.to_vec();
    }
    // Simple averaging downsampler
    data.chunks(step)
        .map(|chunk| chunk.iter().sum::<f32>() / chunk.len() as f32)
        .collect()
}

fn find_best_lag_fft(signal: &[f32], query: &[f32]) -> Option<usize> {
    let n = signal.len();
    let m = query.len();
    if n < m { return None; }
    
    // Padding size for linear convolution
    let fft_len = (n + m).next_power_of_two();
    
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(fft_len);
    let c2r = planner.plan_fft_inverse(fft_len);
    
    // Prepare Signal
    let mut signal_padded = signal.to_vec();
    signal_padded.resize(fft_len, 0.0);
    let mut signal_spectrum = r2c.make_output_vec();
    r2c.process(&mut signal_padded, &mut signal_spectrum).ok()?;
    
    // Prepare Query (Reversed for Correlation)
    let mut query_padded = vec![0.0; fft_len];
    for (i, &val) in query.iter().enumerate() {
        query_padded[m - 1 - i] = val;
    }
    let mut query_spectrum = r2c.make_output_vec();
    r2c.process(&mut query_padded, &mut query_spectrum).ok()?;
    
    // Multiply in Frequency Domain
    // Result = Signal * Query
    for (s, q) in signal_spectrum.iter_mut().zip(query_spectrum.iter()) {
        *s = *s * q;
    }
    
    // Inverse FFT
    let mut result = c2r.make_output_vec();
    c2r.process(&mut signal_spectrum, &mut result).ok()?;
    
    // Normalize Output
    // Proper NCC requires normalizing by the local energy of the signal.
    // LocalEnergy[i] = Sum(signal[i..i+m]^2)
    // We can compute this efficiently.
    let local_energy = compute_moving_sum_squares(signal, m);
    let query_energy: f32 = query.iter().map(|x| x*x).sum();
    let query_norm = query_energy.sqrt();
    
    let scale = 1.0 / fft_len as f32; // FFT scaling factor
    
    let mut best_corr = -1.0;
    let mut best_lag = 0;
    
    // The result[i] corresponds to the dot product of signal and query
    // where the query ends at index `i - (m - 1)` in the signal ??
    // Let's verify lag:
    // Convolution: (f * g)(t) = \int f(tau) g(t - tau) dtau
    // We computed IFFT( F * G_rev ). G_rev(t) = g(-t).
    // Result(t) = (f * g_rev)(t) = \int f(tau) g_rev(t - tau) dtau = \int f(tau) g(tau - t) dtau.
    // This is Cross-Correlation at lag t.
    // Wait, definition of Corr(t) = \int f(tau) g(tau + t) dtau ?
    // Let's stick to indices.
    // Peak at index `k` means `signal[k]` matches `query[0]`? No.
    // Usually, index `k` in `conv(f, rev(g))` means the dot product when `g` is aligned such that its last element overlaps `f[k]`.
    // So the start of `g` (query) is at `k - (m - 1)`.
    // So `lag = k - (m - 1)`.
    
    let search_limit = n.saturating_sub(m + 100); // Avoid self-match at end
    
    for k in (m - 1)..result.len() {
        let lag = k - (m - 1);
        if lag >= search_limit { break; }
        
        let dot_product = result[k] * scale;
        
        // Normalization
        if lag < local_energy.len() {
            let signal_norm = local_energy[lag].sqrt();
            let denom = signal_norm * query_norm;
            
            if denom > 1e-9 {
                let corr = dot_product / denom;
                if corr > best_corr {
                    best_corr = corr;
                    best_lag = lag;
                }
            }
        }
    }
    
    if best_corr <= 0.0 { return None; }
    Some(best_lag)
}

fn compute_moving_sum_squares(data: &[f32], window_size: usize) -> Vec<f32> {
    let mut energy = Vec::with_capacity(data.len() - window_size + 1);
    let mut current_sum = 0.0;
    
    // Initial window
    for i in 0..window_size {
        if i < data.len() {
            current_sum += data[i] * data[i];
        }
    }
    energy.push(current_sum);
    
    // Slide
    for i in 1..=(data.len() - window_size) {
        let remove = data[i-1];
        let add = data[i + window_size - 1];
        current_sum = current_sum - (remove * remove) + (add * add);
        // Prevent negative due to float errors
        if current_sum < 0.0 { current_sum = 0.0; }
        energy.push(current_sum);
    }
    
    energy
}

fn find_best_match_ncc_fine(
    query: &[f32], 
    full_mono: &[f32], 
    search_start_idx: usize, 
    search_end_idx: usize
) -> (usize, f32) {
    let m = query.len();
    let query_mean = query.iter().sum::<f32>() / m as f32;
    let query_denom = query.iter().map(|x| (x - query_mean).powi(2)).sum::<f32>().sqrt();
    
    if query_denom < 1e-9 { return (0, 0.0); }

    let mut best_corr = -1.0;
    let mut best_rel_offset = 0;
    
    // We iterate through the search range
    // Limit the loop to avoid out of bounds
    let max_offset = search_end_idx.min(full_mono.len().saturating_sub(m));
    if max_offset < search_start_idx { return (0, 0.0); }
    
    for i in search_start_idx..max_offset {
        let candidate = &full_mono[i..i+m];
        
        let cand_mean = candidate.iter().sum::<f32>() / m as f32;
        let cand_denom = candidate.iter().map(|x| (x - cand_mean).powi(2)).sum::<f32>().sqrt();
        
        if cand_denom < 1e-9 { continue; }
        
        let numer: f32 = query.iter().zip(candidate.iter())
            .map(|(q, c)| (q - query_mean) * (c - cand_mean))
            .sum();
            
        let corr = numer / (query_denom * cand_denom);
        
        if corr > best_corr {
            best_corr = corr;
            best_rel_offset = i - search_start_idx;
        }
    }
    
    (best_rel_offset, best_corr)
}


// --- FADE OUT DETECTION (Ported & Simplified) ---

pub fn detect_fade_out(mono: &[f32], sample_rate: u32, channels: usize, settings: &AnalysisSettings) -> Option<FadeOutInfo> {
    // 1. Find effective end (ignore trailing silence)
    let silence_threshold = settings.fade_out_threshold_volume * 0.5;
    let effective_end_idx = find_effective_end(mono, silence_threshold);
    
    let min_audio_samples = (sample_rate as f32 * 5.0) as usize;
    if effective_end_idx < min_audio_samples { return None; }

    let window_size_samples = (settings.fade_out_window_size_ms as f32 / 1000.0 * sample_rate as f32) as usize;
    if window_size_samples == 0 || window_size_samples >= effective_end_idx { return None; }

    // 2. Scan RMS backwards to find where volume drops significantly
    let _scan_duration_samples = (settings.fade_out_window_size_ms as f32 * 100.0 / 1000.0 * sample_rate as f32) as usize; // Scan last few seconds? No, scan reasonably far back.
    let scan_start = effective_end_idx.saturating_sub(sample_rate as usize * 60); // Scan last 60s max
    
    let mut rms_history = Vec::new();
    let mut indices = Vec::new();
    
    // Step backwards in windows
    let step = window_size_samples;
    let mut curr = effective_end_idx;
    while curr >= scan_start + window_size_samples && curr >= step {
        curr -= step;
        let window = &mono[curr..curr + window_size_samples];
        let rms = calculate_rms(window);
        rms_history.push(rms);
        indices.push(curr);
    }
    
    // rms_history is from End -> Start
    // We look for a region where RMS is consistently increasing (since we are going backwards)
    // and then plateaus or goes higher.
    
    if rms_history.len() < 5 { return None; }
    
    // Simple heuristic: Find the longest chain of increasing RMS values from the start (end of file)
    let _fade_end_idx_in_history = 0;
    let mut fade_start_idx_in_history = 0;
    
    for i in 0..rms_history.len()-1 {
        if rms_history[i+1] > rms_history[i] {
            fade_start_idx_in_history = i + 1;
        } else {
            // If it drops significantly, the fade might have stopped (going backwards)
            // But allow some jitter
            if rms_history[i+1] < rms_history[i] * 0.9 {
                 break;
            }
        }
    }
    
    // Map back to samples
    let start_sample = indices[fade_start_idx_in_history];
    let end_sample = effective_end_idx;
    let duration = end_sample - start_sample;
    
    let min_duration = (settings.min_fade_out_duration_ms as f32 / 1000.0 * sample_rate as f32) as usize;
    if duration < min_duration { return None; }
    
    // Check if it's a real fade: Volume at start should be significantly higher than end
    let start_rms = rms_history[fade_start_idx_in_history];
    let end_rms = rms_history[0];
    if start_rms < settings.fade_out_threshold_volume { return None; }
    if start_rms < end_rms * 2.0 { return None; } // At least 6dB drop
    
    Some(FadeOutInfo {
        start_sample: start_sample * channels, 
        duration_samples: duration * channels,
        confidence: 0.8,
    })
}

fn calculate_rms(data: &[f32]) -> f32 {
    let sum_sq: f32 = data.iter().map(|x| x * x).sum();
    (sum_sq / (data.len() as f32 + 1e-9)).sqrt()
}