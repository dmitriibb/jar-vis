use std::path::Path;

use anyhow::{anyhow, Context, Result};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::SttEngine;
use crate::audio::STT_SAMPLE_RATE;

#[derive(Default)]
pub struct WhisperEngine {
    ctx: Option<WhisperContext>,
}

fn thread_count() -> i32 {
    // available_parallelism counts logical cores; whisper scales with physical ones.
    let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    (logical / 2).clamp(1, 8) as i32
}

impl SttEngine for WhisperEngine {
    fn load(&mut self, model_path: &Path) -> Result<()> {
        let path = model_path.to_str().ok_or_else(|| anyhow!("model path is not valid UTF-8"))?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("could not load the Whisper model {}", model_path.display()))?;
        self.ctx = Some(ctx);
        Ok(())
    }

    fn is_loaded(&self) -> bool {
        self.ctx.is_some()
    }

    fn unload(&mut self) {
        self.ctx = None;
    }

    fn transcribe(&mut self, pcm_16k_mono: &[f32], vocabulary: &str) -> Result<String> {
        let ctx = self.ctx.as_ref().ok_or_else(|| anyhow!("no Whisper model is loaded"))?;
        let mut state = ctx.create_state().context("create whisper state")?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_translate(false);
        params.set_no_context(true);
        params.set_n_threads(thread_count());
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if !vocabulary.trim().is_empty() {
            params.set_initial_prompt(vocabulary.trim());
        }

        // whisper.cpp refuses input shorter than one second, so pad short clips.
        let min_len = (STT_SAMPLE_RATE as usize * 11) / 10;
        let padded;
        let pcm = if pcm_16k_mono.len() < min_len {
            let mut v = pcm_16k_mono.to_vec();
            v.resize(min_len, 0.0);
            padded = v;
            &padded[..]
        } else {
            pcm_16k_mono
        };

        state.full(params, pcm).context("whisper failed")?;
        let segments = state.full_n_segments().context("read segments")?;
        let mut text = String::new();
        for i in 0..segments {
            text.push_str(&state.full_get_segment_text(i).context("read segment text")?);
        }
        Ok(text.trim().to_string())
    }
}
