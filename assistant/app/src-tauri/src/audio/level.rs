/// Root mean square of a block of samples, 0.0 for an empty block.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Below this RMS a whole recording is treated as silence and discarded.
pub const SILENCE_RMS: f32 = 0.0015;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_values() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 16]), 0.0);
        assert!((rms(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < 1e-6);
    }
}
