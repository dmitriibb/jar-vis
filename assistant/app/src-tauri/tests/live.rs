//! Tests against real services and real models. They are ignored by default:
//!
//! ```text
//! cargo test --test live -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! `opencode_round_trip` needs opencode installed and signed in to a provider.
//! `speech_round_trip` downloads the default speech models (about 130 MB) into
//! `<repo>/models` on first run.

use std::path::PathBuf;
use std::time::Duration;

use jar_vis_lib::backend::opencode::OpenCodeBackend;
use jar_vis_lib::backend::{AgentBackend, BackendEvent, SessionOpts};
use jar_vis_lib::config::Config;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("repo root")
}

/// Strip the `\\?\` prefix Windows adds when canonicalizing.
fn plain(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy().into_owned();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}

#[tokio::test]
#[ignore]
async fn opencode_round_trip() {
    let backend = OpenCodeBackend::new(Config::default().opencode);
    let info = backend.connect().await.expect("opencode is reachable");
    println!("opencode {} at {}", info.version, info.url);

    let mut events = backend.subscribe();
    backend.start();
    let connected = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(BackendEvent::Connected) = events.recv().await {
                break;
            }
        }
    })
    .await;
    assert!(connected.is_ok(), "the event stream did not connect");
    assert!(backend.info().is_some());

    let dir = plain(repo_root().join("sessions"));
    let agents = backend.list_agents(&dir).await.expect("list agents");
    println!("agents: {agents:?}");
    assert!(agents.iter().any(|a| a == "build"), "opencode's built-in agents are listed");

    let projects = backend.list_projects().await.expect("list projects");
    assert!(!projects.is_empty());

    let agent = agents.iter().find(|a| *a == "jarvis").cloned();
    let session = backend.create_session(&dir, &SessionOpts { agent: agent.clone(), model: None }).await.expect("create session");
    let sref = session.as_ref();
    println!("session {} (agent {:?})", session.id, session.agent);
    assert_eq!(session.agent, agent);

    let result = async {
        assert!(backend.get_session(&session.id).await?.is_some());
        let listed = backend.list_sessions(&dir).await?;
        assert!(listed.iter().any(|s| s.id == session.id), "the new session is listed for its directory");
        assert!(backend.pending_permissions(&sref).await?.is_empty());

        backend.send_prompt(&sref, "Reply with exactly one word: pong").await?;
        let outcome = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                match events.recv().await {
                    Ok(BackendEvent::SessionIdle { session: s, .. }) if s == session.id => break Ok(()),
                    Ok(BackendEvent::SessionError { session: s, message }) if s == session.id => break Err(message),
                    Ok(other) => println!("event: {other:?}"),
                    Err(e) => break Err(format!("event stream: {e}")),
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("the turn did not finish in time"))?;

        match outcome {
            Ok(()) => {
                let text = backend.last_assistant_text(&sref).await?;
                println!("answer: {text}");
                assert!(text.to_lowercase().contains("pong"), "unexpected answer: {text}");
            }
            Err(message) if std::env::var_os("JARVIS_ALLOW_PROVIDER_ERROR").is_some() => {
                println!("the turn failed inside opencode (allowed by JARVIS_ALLOW_PROVIDER_ERROR): {message}");
            }
            Err(message) => anyhow::bail!("the turn failed: {message}"),
        }
        assert!(!backend.is_busy(&sref).await?);
        anyhow::Ok(())
    }
    .await;

    backend.delete_session(&session.id).await.expect("delete the test session");
    assert!(backend.get_session(&session.id).await.expect("get").is_none(), "a deleted session is reported as gone");
    result.expect("round trip");
}

#[cfg(all(feature = "stt-whisper", feature = "tts-sherpa"))]
#[tokio::test]
#[ignore]
async fn speech_round_trip() {
    use jar_vis_lib::audio::resample;
    use jar_vis_lib::models::ModelManager;
    use jar_vis_lib::stt::{filter, SttService};
    use jar_vis_lib::tts::sherpa::SherpaEngine;
    use jar_vis_lib::tts::TtsEngine;

    let cfg = Config::default();
    let models = ModelManager::new(plain(repo_root().join("models")));
    for id in [cfg.stt.model.as_str(), cfg.tts.voice.as_str()] {
        if !models.is_installed(id) {
            println!("downloading {id}…");
            models.download(id, |_, _| {}).await.unwrap_or_else(|e| panic!("download {id}: {e:#}"));
        }
        assert!(models.is_installed(id), "{id} is installed");
    }

    // Speak a sentence with the default voice, then listen to it with the default model.
    let voice = models.voice(&cfg.tts.voice).expect("voice");
    let model = models.stt_path(&cfg.stt.model).expect("stt model");
    let text = tokio::task::spawn_blocking(move || {
        let mut tts = SherpaEngine::default();
        tts.load(&voice).expect("load voice");
        let started = std::time::Instant::now();
        let (samples, rate) =
            tts.synthesize("Hello. Start a new session and check the build.", 1.0).expect("synthesize");
        println!(
            "synthesized {:.1}s of audio at {rate} Hz in {:?}",
            samples.len() as f32 / rate as f32,
            started.elapsed()
        );
        assert!(samples.len() > rate as usize, "more than a second of audio");

        let pcm = resample::to_16k(&samples, rate).expect("resample");
        let stt = SttService::default();
        let started = std::time::Instant::now();
        let text = stt.transcribe(&model, &pcm, &Config::default().stt.vocabulary).expect("transcribe");
        println!("transcribed in {:?}: {text}", started.elapsed());
        text
    })
    .await
    .expect("speech task");

    let cleaned = filter::clean(&text).expect("a real transcript survives the hallucination filter");
    let lower = cleaned.to_lowercase();
    // Piper is not deterministic, so only the sturdy words are checked.
    assert!(lower.contains("hello"), "{cleaned}");
    assert!(lower.contains("build"), "{cleaned}");
    println!("parsed: {:?}", jar_vis_lib::commands::parse(&cleaned, &cfg.commands, false));
}

/// Silence must come back quickly and be dropped by the hallucination filter.
#[cfg(feature = "stt-whisper")]
#[test]
#[ignore]
fn whisper_handles_silence() {
    use jar_vis_lib::models::ModelManager;
    use jar_vis_lib::stt::{filter, SttService};

    let models = ModelManager::new(plain(repo_root().join("models")));
    let model = models.stt_path(&Config::default().stt.model).expect("run speech_round_trip first to download the model");
    let stt = SttService::default();
    stt.ensure_loaded(&model).expect("load");
    let started = std::time::Instant::now();
    let text = stt.transcribe(&model, &vec![0.0f32; 32_000], "").expect("transcribe");
    let took = started.elapsed();
    println!("silence -> {text:?} in {took:?}");
    assert_eq!(filter::clean(&text), None, "silence produced {text:?}");
    // An unoptimized whisper.cpp build takes over ten seconds here.
    assert!(took < Duration::from_secs(6), "transcription took {took:?}");
}
