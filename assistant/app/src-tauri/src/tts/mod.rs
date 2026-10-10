//! Text to speech. One worker thread owns the engine and the audio output,
//! because neither is `Send` on every platform.

pub mod sanitize;
#[cfg(feature = "tts-sherpa")]
pub mod sherpa;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceSpec {
    /// Catalog id, e.g. `piper-en_US-lessac-medium`.
    pub id: String,
    /// Folder holding the model, `tokens.txt` and `espeak-ng-data/`.
    pub dir: PathBuf,
    /// `piper` or `kokoro`.
    pub engine: String,
}

pub trait TtsEngine: Send {
    fn load(&mut self, voice: &VoiceSpec) -> Result<()>;
    fn is_loaded(&self) -> bool;
    fn unload(&mut self);
    /// Returns mono samples and their sample rate.
    fn synthesize(&mut self, text: &str, speed: f32) -> Result<(Vec<f32>, u32)>;
}

#[cfg(not(feature = "tts-sherpa"))]
struct NoTts;

#[cfg(not(feature = "tts-sherpa"))]
impl TtsEngine for NoTts {
    fn load(&mut self, _: &VoiceSpec) -> Result<()> {
        anyhow::bail!("this build has no text-to-speech engine (feature `tts-sherpa` is off)")
    }
    fn is_loaded(&self) -> bool {
        false
    }
    fn unload(&mut self) {}
    fn synthesize(&mut self, _: &str, _: f32) -> Result<(Vec<f32>, u32)> {
        anyhow::bail!("this build has no text-to-speech engine (feature `tts-sherpa` is off)")
    }
}

fn new_engine() -> Box<dyn TtsEngine> {
    #[cfg(feature = "tts-sherpa")]
    {
        Box::new(sherpa::SherpaEngine::default())
    }
    #[cfg(not(feature = "tts-sherpa"))]
    {
        Box::new(NoTts)
    }
}

enum Cmd {
    Speak { text: String, voice: VoiceSpec, speed: f32, generation: u64 },
    Unload,
}

/// How one utterance ended.
#[derive(Debug)]
pub enum SpeechEnd {
    Finished,
    /// Stopped by `stop()` or replaced by a newer utterance.
    Cancelled,
    Failed(String),
}

struct Shared {
    /// Bumped by every `speak` and `stop`. Work for an older value is abandoned.
    generation: AtomicU64,
    sink: Mutex<Option<Arc<Sink>>>,
    loaded: AtomicBool,
    last_used: Mutex<Instant>,
}

#[derive(Clone)]
pub struct TtsHandle {
    tx: mpsc::Sender<Cmd>,
    shared: Arc<Shared>,
}

impl TtsHandle {
    /// Start the worker. `on_end` is called once per `speak` with its generation.
    pub fn spawn(on_end: impl Fn(u64, SpeechEnd) + Send + 'static) -> Result<TtsHandle> {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            sink: Mutex::new(None),
            loaded: AtomicBool::new(false),
            last_used: Mutex::new(Instant::now()),
        });
        let worker_shared = shared.clone();
        std::thread::Builder::new()
            .name("jarvis-tts".into())
            .spawn(move || worker(rx, worker_shared, on_end))
            .context("start TTS thread")?;
        Ok(TtsHandle { tx, shared })
    }

    /// Speak `text`, replacing whatever is playing. Returns the utterance's generation.
    pub fn speak(&self, text: &str, voice: VoiceSpec, speed: f32) -> u64 {
        let generation = self.interrupt();
        let _ = self.tx.send(Cmd::Speak { text: text.to_string(), voice, speed, generation });
        generation
    }

    /// Stop playback at once and drop queued speech (barge-in).
    pub fn stop(&self) {
        self.interrupt();
    }

    fn interrupt(&self) -> u64 {
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(sink) = self.shared.sink.lock().take() {
            sink.stop();
        }
        generation
    }

    /// Stop speaking and free the model (used by mute).
    pub fn unload(&self) {
        self.stop();
        let _ = self.tx.send(Cmd::Unload);
    }

    pub fn is_loaded(&self) -> bool {
        self.shared.loaded.load(Ordering::SeqCst)
    }

    /// Unload when the voice has not been used for `minutes` (0 = never).
    pub fn unload_if_idle(&self, minutes: u32) {
        if minutes == 0 || !self.is_loaded() {
            return;
        }
        if self.shared.last_used.lock().elapsed().as_secs() >= minutes as u64 * 60 {
            let _ = self.tx.send(Cmd::Unload);
        }
    }
}

fn worker(rx: mpsc::Receiver<Cmd>, shared: Arc<Shared>, on_end: impl Fn(u64, SpeechEnd)) {
    let mut engine = new_engine();
    let mut loaded_voice: Option<VoiceSpec> = None;
    let current = |generation: u64| shared.generation.load(Ordering::SeqCst) == generation;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Unload => {
                if engine.is_loaded() {
                    engine.unload();
                    tracing::info!("unloaded the TTS voice");
                }
                loaded_voice = None;
                shared.loaded.store(false, Ordering::SeqCst);
            }
            Cmd::Speak { text, voice, speed, generation } => {
                if !current(generation) {
                    on_end(generation, SpeechEnd::Cancelled);
                    continue;
                }
                if loaded_voice.as_ref() != Some(&voice) || !engine.is_loaded() {
                    let started = Instant::now();
                    if let Err(e) = engine.load(&voice) {
                        loaded_voice = None;
                        shared.loaded.store(false, Ordering::SeqCst);
                        on_end(generation, SpeechEnd::Failed(format!("{e:#}")));
                        continue;
                    }
                    tracing::info!("loaded TTS voice {} in {:?}", voice.id, started.elapsed());
                    loaded_voice = Some(voice);
                    shared.loaded.store(true, Ordering::SeqCst);
                }
                let end = play(engine.as_mut(), &shared, &text, speed, generation);
                *shared.last_used.lock() = Instant::now();
                on_end(generation, end);
            }
        }
    }
}

/// Synthesize sentence by sentence, so the first one plays while the rest render.
fn play(engine: &mut dyn TtsEngine, shared: &Shared, text: &str, speed: f32, generation: u64) -> SpeechEnd {
    let current = || shared.generation.load(Ordering::SeqCst) == generation;
    let (_stream, handle) = match OutputStream::try_default() {
        Ok(pair) => pair,
        Err(e) => return SpeechEnd::Failed(format!("no audio output: {e}")),
    };
    let sink = match Sink::try_new(&handle) {
        Ok(sink) => Arc::new(sink),
        Err(e) => return SpeechEnd::Failed(format!("no audio output: {e}")),
    };
    *shared.sink.lock() = Some(sink.clone());
    // `stop()` may have run between the check in `worker` and publishing the sink.
    if !current() {
        return SpeechEnd::Cancelled;
    }

    let mut sentences = sanitize::sentences(text);
    if sentences.is_empty() {
        sentences.push(text.to_string());
    }
    for sentence in sentences {
        if !current() {
            return SpeechEnd::Cancelled;
        }
        match engine.synthesize(&sentence, speed) {
            Ok((samples, rate)) if !samples.is_empty() => sink.append(SamplesBuffer::new(1, rate, samples)),
            Ok(_) => {}
            Err(e) => {
                sink.stop();
                return SpeechEnd::Failed(format!("{e:#}"));
            }
        }
    }
    sink.sleep_until_end();
    let end = if current() { SpeechEnd::Finished } else { SpeechEnd::Cancelled };
    shared.sink.lock().take();
    end
}
