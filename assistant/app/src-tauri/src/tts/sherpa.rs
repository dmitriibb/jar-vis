use anyhow::{anyhow, bail, Result};
use sherpa_rs::tts::{KokoroTts, KokoroTtsConfig, VitsTts, VitsTtsConfig};
use sherpa_rs::OnnxConfig;

use super::{TtsEngine, VoiceSpec};

enum Loaded {
    Piper(VitsTts),
    Kokoro(KokoroTts),
}

/// Piper (VITS) and Kokoro voices through sherpa-onnx.
#[derive(Default)]
pub struct SherpaEngine {
    loaded: Option<Loaded>,
}

fn onnx_config() -> OnnxConfig {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4);
    OnnxConfig { provider: "cpu".into(), debug: false, num_threads: threads as i32 }
}

fn existing(path: std::path::PathBuf) -> Result<String> {
    if !path.exists() {
        bail!("voice file is missing: {}", path.display());
    }
    Ok(path.to_string_lossy().into_owned())
}

impl TtsEngine for SherpaEngine {
    fn load(&mut self, voice: &VoiceSpec) -> Result<()> {
        self.loaded = None;
        // sherpa-onnx aborts on missing files instead of returning an error, so check first.
        let tokens = existing(voice.dir.join("tokens.txt"))?;
        let data_dir = existing(voice.dir.join("espeak-ng-data"))?;
        let model = std::fs::read_dir(&voice.dir)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "onnx"))
            .ok_or_else(|| anyhow!("no .onnx model in {}", voice.dir.display()))?
            .to_string_lossy()
            .into_owned();

        self.loaded = Some(match voice.engine.as_str() {
            "kokoro" => Loaded::Kokoro(KokoroTts::new(KokoroTtsConfig {
                model,
                voices: existing(voice.dir.join("voices.bin"))?,
                tokens,
                data_dir,
                length_scale: 1.0,
                onnx_config: onnx_config(),
                ..Default::default()
            })),
            _ => Loaded::Piper(VitsTts::new(VitsTtsConfig {
                model,
                tokens,
                data_dir,
                length_scale: 1.0,
                noise_scale: 0.667,
                noise_scale_w: 0.8,
                onnx_config: onnx_config(),
                ..Default::default()
            })),
        });
        Ok(())
    }

    fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }

    fn unload(&mut self) {
        self.loaded = None;
    }

    fn synthesize(&mut self, text: &str, speed: f32) -> Result<(Vec<f32>, u32)> {
        let audio = match self.loaded.as_mut().ok_or_else(|| anyhow!("no voice is loaded"))? {
            Loaded::Piper(tts) => tts.create(text, 0, speed),
            Loaded::Kokoro(tts) => tts.create(text, 0, speed),
        }
        .map_err(|e| anyhow!("speech synthesis failed: {e}"))?;
        Ok((audio.samples, audio.sample_rate))
    }
}
