//! Speech to text. The engine runs in-process and is called from blocking threads.

pub mod filter;
#[cfg(feature = "stt-whisper")]
pub mod whisper;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use parking_lot::Mutex;

pub trait SttEngine: Send {
    fn load(&mut self, model_path: &Path) -> Result<()>;
    fn is_loaded(&self) -> bool;
    fn unload(&mut self);
    /// `vocabulary` biases recognition towards the listed terms.
    fn transcribe(&mut self, pcm_16k_mono: &[f32], vocabulary: &str) -> Result<String>;
}

/// Stands in when the app is built without the `stt-whisper` feature.
#[cfg(not(feature = "stt-whisper"))]
struct NoStt;

#[cfg(not(feature = "stt-whisper"))]
impl SttEngine for NoStt {
    fn load(&mut self, _: &Path) -> Result<()> {
        anyhow::bail!("this build has no speech-to-text engine (feature `stt-whisper` is off)")
    }
    fn is_loaded(&self) -> bool {
        false
    }
    fn unload(&mut self) {}
    fn transcribe(&mut self, _: &[f32], _: &str) -> Result<String> {
        anyhow::bail!("this build has no speech-to-text engine (feature `stt-whisper` is off)")
    }
}

fn new_engine() -> Box<dyn SttEngine> {
    #[cfg(feature = "stt-whisper")]
    {
        Box::new(whisper::WhisperEngine::default())
    }
    #[cfg(not(feature = "stt-whisper"))]
    {
        Box::new(NoStt)
    }
}

struct Inner {
    engine: Box<dyn SttEngine>,
    loaded: Option<PathBuf>,
    last_used: Instant,
}

/// Owns the engine, loads models lazily and swaps them when the choice changes.
pub struct SttService {
    inner: Mutex<Inner>,
}

impl Default for SttService {
    fn default() -> Self {
        Self { inner: Mutex::new(Inner { engine: new_engine(), loaded: None, last_used: Instant::now() }) }
    }
}

impl SttService {
    /// Make sure `model_path` is the loaded model. Blocking.
    pub fn ensure_loaded(&self, model_path: &Path) -> Result<()> {
        let mut inner = self.inner.lock();
        if inner.loaded.as_deref() == Some(model_path) && inner.engine.is_loaded() {
            return Ok(());
        }
        inner.engine.unload();
        inner.loaded = None;
        let started = Instant::now();
        inner.engine.load(model_path)?;
        inner.loaded = Some(model_path.to_path_buf());
        inner.last_used = Instant::now();
        tracing::info!("loaded STT model {} in {:?}", model_path.display(), started.elapsed());
        Ok(())
    }

    /// Transcribe 16 kHz mono audio, loading the model first if needed. Blocking.
    pub fn transcribe(&self, model_path: &Path, pcm: &[f32], vocabulary: &str) -> Result<String> {
        self.ensure_loaded(model_path)?;
        let mut inner = self.inner.lock();
        let started = Instant::now();
        let text = inner.engine.transcribe(pcm, vocabulary)?;
        inner.last_used = Instant::now();
        tracing::debug!("transcribed {:.1}s of audio in {:?}", pcm.len() as f32 / 16_000.0, started.elapsed());
        Ok(text)
    }

    pub fn is_loaded(&self) -> bool {
        self.inner.lock().engine.is_loaded()
    }

    pub fn unload(&self) {
        let mut inner = self.inner.lock();
        if inner.engine.is_loaded() {
            inner.engine.unload();
            inner.loaded = None;
            tracing::info!("unloaded the STT model");
        }
    }

    /// Unload when the model has not been used for `minutes` (0 = never).
    pub fn unload_if_idle(&self, minutes: u32) {
        if minutes == 0 {
            return;
        }
        let idle = self.inner.lock().last_used.elapsed();
        if idle.as_secs() >= minutes as u64 * 60 {
            self.unload();
        }
    }
}
