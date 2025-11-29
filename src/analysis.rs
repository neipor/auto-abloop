use crate::audio::AudioData;
use crate::LoopPoints;

pub fn detect_loop(audio: &AudioData) -> Option<LoopPoints> {
    let channels = audio.channels as usize;
    
    // 1. Mix to mono
    let mono: Vec<f32> = audio.samples
        .chunks_exact(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
        .collect();

    // 2. Find effective end (ignore trailing silence)
    // Scan backwards for significant energy
    let silence_threshold = 0.0005; // Very quiet
    let mut end_idx = mono.len();
    for (i, &sample) in mono.iter().enumerate().rev() {
        if sample.abs() > silence_threshold {
            end_idx = i + 1;
            break;
        }
    }
    
    if end_idx < 44100 * 5 { // Too short (< 5s)
        return None;
    }

    // 3. Prepare Query (End of track)
    // Use a 10-second window, or smaller if file is short
    let search_window_sec = 10.0;
    let window_size = (audio.sample_rate as f32 * search_window_sec) as usize;
    
    if end_idx < window_size * 2 {
        // Try smaller window for short files?
        return None; 
    }

    // Shift query back slightly to avoid catching the very tail of a fade which might be noise
    // Let's take the block ending at end_idx.
    let query_start_idx = end_idx - window_size;
    let query_raw = &mono[query_start_idx..end_idx];

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
    let loop_end_sample = query_start_idx * channels;

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
