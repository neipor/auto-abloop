use anyhow::Result;
use auto_abloop::audio;
use std::path::Path;

fn main() -> Result<()> {
    let filename = "大塚正子,Hungary Phillharmony Orchestra - 好機到来.flac";
    let path = Path::new(filename);
    
    if !path.exists() {
        println!("File not found: {}", filename);
        return Ok(());
    }

    println!("Loading file: {}", filename);
    let audio_data = audio::load_audio_file(path)?;
    println!("Loaded. Sample Rate: {}, Channels: {}, Samples: {}", audio_data.sample_rate, audio_data.channels, audio_data.samples.len());

    // Debug Logic mirroring src/analysis.rs
    let channels = audio_data.channels as usize;
    let mono: Vec<f32> = audio_data.samples
        .chunks_exact(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
        .collect();
    
    println!("Mono samples: {}", mono.len());

    // 1. Silence Detection
    let silence_threshold = 0.0005;
    let mut end_idx = mono.len();
    for (i, &sample) in mono.iter().enumerate().rev() {
        if sample.abs() > silence_threshold {
            end_idx = i + 1;
            break;
        }
    }
    println!("End Index (Silence detection): {} / {} (Last {:.2}s silent)", end_idx, mono.len(), (mono.len() - end_idx) as f32 / audio_data.sample_rate as f32);

    let search_window_sec = 10.0;
    let window_size = (audio_data.sample_rate as f32 * search_window_sec) as usize;
    let query_start_idx = end_idx.saturating_sub(window_size);
    
    println!("Query Window: {} to {} (Size: {})", query_start_idx, end_idx, window_size);
    
    if end_idx < window_size * 2 {
        println!("File too short for analysis.");
        return Ok(());
    }

    let query_raw = &mono[query_start_idx..end_idx];
    let query_rms = calculate_rms(query_raw);
    println!("Query RMS: {:.6}", query_rms);

    // 2. Coarse Search
    let target_rate = 200; // Target ~200Hz for coarse search.
    let coarse_step = (audio_data.sample_rate as usize / target_rate).max(1);
    
    let coarse_query = downsample(query_raw, coarse_step);
    let coarse_search_space = downsample(&mono[0..query_start_idx], coarse_step);
    
    println!("Coarse Search Space Size: {}", coarse_search_space.len());
    
    // Manual NCC to print best candidates
    let candidates = find_matches_ncc(&coarse_query, &coarse_search_space);
    println!("--- Top 3 Coarse Matches ---");
    for (i, (idx, corr)) in candidates.iter().take(3).enumerate() {
        let pos_sec = (idx * coarse_step) as f32 / audio_data.sample_rate as f32; // Original sample rate for time calc
        println!("{}. Time: {:.2}s, Corr: {:.4}", i+1, pos_sec, corr);
    }

    if let Some((best_coarse_idx, best_coarse_corr)) = candidates.first() {
        if *best_coarse_corr < 0.3 { // Adjusted threshold
            println!("Coarse correlation too low (< 0.3). Aborting.");
        } else {
            // Fine Search Simulation
             let estimated_pos = best_coarse_idx * coarse_step;
             let refine_radius = (audio_data.sample_rate as usize) * 4;
             let refine_start = estimated_pos.saturating_sub(refine_radius);
             let refine_end = (estimated_pos + refine_radius).min(query_start_idx - window_size);
             
             println!("Refining around {:.2}s (Range: {:.2}s - {:.2}s)", 
                estimated_pos as f32 / audio_data.sample_rate as f32,
                refine_start as f32 / audio_data.sample_rate as f32,
                refine_end as f32 / audio_data.sample_rate as f32
            );

            let medium_target = 2000;
            let medium_step = (audio_data.sample_rate as usize / medium_target).max(1);
            let refine_slice_raw = &mono[refine_start..refine_end + window_size];
            let medium_query = downsample(query_raw, medium_step);
            let medium_search_slice = downsample(refine_slice_raw, medium_step);
            
            let medium_candidates = find_matches_ncc(&medium_query, &medium_search_slice);
             println!("--- Top 3 Medium/Fine Matches ---");
            for (i, (idx, corr)) in medium_candidates.iter().take(3).enumerate() {
                let abs_pos = refine_start + idx * medium_step;
                let pos_sec = abs_pos as f32 / audio_data.sample_rate as f32;
                
                // Calculate RMS for this match
                let match_slice = &mono[abs_pos..abs_pos + window_size];
                let match_rms = calculate_rms(match_slice);
                let query_rms_for_ratio = calculate_rms(query_raw); // Recalculate or pass query_rms
                let ratio = match_rms / (query_rms_for_ratio + 1e-9);

                println!("{}. Time: {:.2}s, Corr: {:.4}, MatchRMS: {:.6}, Ratio: {:.2}", i+1, pos_sec, corr, match_rms, ratio);
            }
        }
    }

    Ok(())
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

fn find_matches_ncc(query: &[f32], search_space: &[f32]) -> Vec<(usize, f32)> {
    let n = query.len();
    if search_space.len() < n {
        return vec![];
    }

    let query_mean = query.iter().sum::<f32>() / n as f32;
    let query_denom = query.iter().map(|x| (x - query_mean).powi(2)).sum::<f32>().sqrt();

    if query_denom < 1e-9 {
        return vec![];
    }

    let mut matches = Vec::new();

    for i in 0..=(search_space.len() - n) {
        let candidate = &search_space[i..i+n];
        let cand_mean = candidate.iter().sum::<f32>() / n as f32;
        let cand_denom = candidate.iter().map(|x| (x - cand_mean).powi(2)).sum::<f32>().sqrt();

        if cand_denom < 1e-9 {
            continue;
        }

        let numer: f32 = query.iter().zip(candidate.iter())
            .map(|(q, c)| (q - query_mean) * (c - cand_mean))
            .sum();

        let corr = numer / (query_denom * cand_denom);
        matches.push((i, corr));
    }
    
    // Sort desc
    matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    matches
}
