use anyhow::{Context, Result};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use super::STT_SAMPLE_RATE;

/// Resample mono audio to 16 kHz for Whisper.
pub fn to_16k(samples: &[f32], sample_rate: u32) -> Result<Vec<f32>> {
    if sample_rate == STT_SAMPLE_RATE || samples.is_empty() {
        return Ok(samples.to_vec());
    }
    let ratio = STT_SAMPLE_RATE as f64 / sample_rate as f64;
    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    };
    const CHUNK: usize = 4096;
    let mut resampler = SincFixedIn::<f32>::new(ratio, 1.0, params, CHUNK, 1).context("create resampler")?;
    let mut out = Vec::with_capacity((samples.len() as f64 * ratio) as usize + CHUNK);
    let mut chunks = samples.chunks_exact(CHUNK);
    for chunk in &mut chunks {
        let res = resampler.process(&[chunk], None).context("resample")?;
        out.extend_from_slice(&res[0]);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let res = resampler.process_partial(Some(&[rest]), None).context("resample tail")?;
        out.extend_from_slice(&res[0]);
    }
    // The partial last chunk is zero-padded to a full one; drop that padding but
    // keep the resampler's own delay, so the end of the speech is not cut.
    let expected = (samples.len() as f64 * ratio).ceil() as usize + resampler.output_delay();
    out.truncate(expected);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_at_16k() {
        let input = vec![0.25f32; 1000];
        assert_eq!(to_16k(&input, 16_000).unwrap(), input);
    }

    #[test]
    fn downsamples_48k_to_a_third() {
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin()).collect();
        let out = to_16k(&input, 48_000).unwrap();
        let expected = 16_000i64;
        assert!((out.len() as i64 - expected).abs() < 600, "got {} samples", out.len());
        // A 440 Hz tone survives with roughly the same energy.
        let rms = crate::audio::level::rms(&out[2000..14_000]);
        assert!((rms - 0.707).abs() < 0.05, "rms {rms}");
    }

    #[test]
    fn handles_44100() {
        let input = vec![0.1f32; 44_100 / 2];
        let out = to_16k(&input, 44_100).unwrap();
        assert!((out.len() as i64 - 8000).abs() < 600, "got {}", out.len());
    }
}
