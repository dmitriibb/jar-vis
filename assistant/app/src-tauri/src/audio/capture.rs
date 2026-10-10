//! Microphone capture. The input stream is opened on key-down and closed on
//! key-up, so the mic (and the OS mic indicator) is off while Jar-vis is idle.

use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use parking_lot::Mutex;

use super::level::rms;

pub struct Captured {
    /// Mono samples at the device's sample rate.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl Captured {
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate.max(1) as f32
    }
}

/// A recording in progress. cpal streams are not `Send`, so the stream lives on
/// its own thread and this handle only talks to it.
pub struct Recorder {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<Captured>>,
}

pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

fn pick_device(name: &str) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if !name.trim().is_empty() {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(device) = devices.find(|d| d.name().is_ok_and(|n| n == name)) {
                return Ok(device);
            }
        }
        tracing::warn!("input device \"{name}\" not found, using the system default");
    }
    host.default_input_device().ok_or_else(|| anyhow!("no microphone found"))
}

impl Recorder {
    /// Start recording. `on_level` receives the RMS of roughly every 50 ms of audio.
    pub fn start(
        device_name: &str,
        max_seconds: u32,
        on_level: impl Fn(f32) + Send + 'static,
    ) -> Result<Recorder> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let device_name = device_name.to_string();

        let thread = std::thread::Builder::new()
            .name("jarvis-capture".into())
            .spawn(move || {
                let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
                let mut sample_rate = 0;
                let stream = open_stream(&device_name, max_seconds, buffer.clone(), on_level).map(|(s, rate)| {
                    sample_rate = rate;
                    s
                });
                match stream {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        // Block until `finish`, `cancel`, or the handle is dropped.
                        let _ = stop_rx.recv();
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
                let samples = std::mem::take(&mut *buffer.lock());
                Captured { samples, sample_rate }
            })
            .context("start capture thread")?;

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Recorder { stop: stop_tx, thread: Some(thread) }),
            Ok(Err(e)) => Err(e),
            Err(_) => bail!("the microphone did not start in time"),
        }
    }

    /// Stop recording and return what was captured.
    pub fn finish(mut self) -> Result<Captured> {
        let _ = self.stop.send(());
        let thread = self.thread.take().expect("recorder thread");
        thread.join().map_err(|_| anyhow!("the capture thread panicked"))
    }

    /// Stop recording and throw the audio away.
    pub fn cancel(self) {
        let _ = self.finish();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn open_stream(
    device_name: &str,
    max_seconds: u32,
    buffer: Arc<Mutex<Vec<f32>>>,
    on_level: impl Fn(f32) + Send + 'static,
) -> Result<(cpal::Stream, u32)> {
    let device = pick_device(device_name)?;
    let supported = device.default_input_config().context("the microphone has no usable format")?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels().max(1) as usize;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let max_samples = sample_rate as usize * max_seconds.max(1) as usize;
    let level_block = (sample_rate as usize / 20).max(1);
    let mut level_acc: Vec<f32> = Vec::with_capacity(level_block);

    // Downmix one interleaved block to mono, store it, and report levels.
    let mut push = move |frames: &mut dyn Iterator<Item = f32>| {
        let mut buf = buffer.lock();
        let mut frame_sum = 0.0;
        let mut in_frame = 0;
        for sample in frames {
            frame_sum += sample;
            in_frame += 1;
            if in_frame == channels {
                let mono = frame_sum / channels as f32;
                if buf.len() < max_samples {
                    buf.push(mono);
                }
                level_acc.push(mono);
                if level_acc.len() >= level_block {
                    on_level(rms(&level_acc));
                    level_acc.clear();
                }
                frame_sum = 0.0;
                in_frame = 0;
            }
        }
    };
    let on_error = |e: cpal::StreamError| tracing::warn!("microphone stream error: {e}");

    let stream = match format {
        SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| push(&mut data.iter().copied()),
            on_error,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| push(&mut data.iter().map(|s| *s as f32 / 32_768.0)),
            on_error,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                push(&mut data.iter().map(|s| (*s as f32 - 32_768.0) / 32_768.0))
            },
            on_error,
            None,
        ),
        SampleFormat::I32 => device.build_input_stream(
            &config,
            move |data: &[i32], _: &cpal::InputCallbackInfo| {
                push(&mut data.iter().map(|s| *s as f32 / 2_147_483_648.0))
            },
            on_error,
            None,
        ),
        other => bail!("unsupported microphone sample format {other:?}"),
    }
    .context("could not open the microphone")?;
    stream.play().context("could not start the microphone")?;
    Ok((stream, sample_rate))
}
