//! Download a model to `*.part`, verify its checksum, then install it atomically.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use super::{remove_path, CatalogEntry, ModelKind};

fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    target.with_file_name(name)
}

pub async fn install(
    http: &reqwest::Client,
    entry: &CatalogEntry,
    target: &Path,
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(u64, u64) + Send + Sync + 'static,
) -> Result<()> {
    let parent = target.parent().ok_or_else(|| anyhow!("bad model path"))?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let part = part_path(target);

    let result = fetch(http, entry, &part, &cancel, &on_progress).await;
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }

    let outcome = match entry.kind {
        ModelKind::Stt => {
            remove_path(target).ok();
            std::fs::rename(&part, target).context("install model file")
        }
        ModelKind::Tts => {
            let part = part.clone();
            let target = target.to_path_buf();
            let root = entry.archive_root.clone().unwrap_or_default();
            tokio::task::spawn_blocking(move || extract_voice(&part, &target, &root))
                .await
                .map_err(|e| anyhow!("extract task failed: {e}"))?
        }
    };
    let _ = std::fs::remove_file(&part);
    outcome
}

async fn fetch(
    http: &reqwest::Client,
    entry: &CatalogEntry,
    part: &Path,
    cancel: &AtomicBool,
    on_progress: &(impl Fn(u64, u64) + Send + Sync),
) -> Result<()> {
    let res = http.get(&entry.url).send().await.context("download failed")?;
    if !res.status().is_success() {
        bail!("download failed: the server answered {}", res.status());
    }
    let total = res.content_length().unwrap_or(entry.size_mb as u64 * 1_000_000);
    let mut file = std::fs::File::create(part).with_context(|| format!("create {}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut last_reported = 0u64;
    let mut stream = res.bytes_stream();
    on_progress(0, total);
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            bail!("download cancelled");
        }
        let chunk = chunk.context("download interrupted")?;
        file.write_all(&chunk).context("write model file")?;
        hasher.update(&chunk);
        done += chunk.len() as u64;
        // Report roughly every megabyte.
        if done - last_reported >= 1_000_000 {
            last_reported = done;
            on_progress(done, total);
        }
    }
    file.flush().ok();
    drop(file);
    on_progress(done, total.max(done));

    if !entry.sha256.is_empty() {
        let actual = hex(&hasher.finalize());
        if !actual.eq_ignore_ascii_case(&entry.sha256) {
            bail!("the download of {} is corrupt (checksum mismatch) — try again", entry.id);
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpack a sherpa-onnx voice archive (`<root>/model.onnx`, `tokens.txt`,
/// `espeak-ng-data/`) so that `<root>` becomes `target`.
fn extract_voice(archive: &Path, target: &Path, root: &str) -> Result<()> {
    let parent = target.parent().ok_or_else(|| anyhow!("bad voice path"))?;
    let staging = parent.join(format!(".extract-{}", std::process::id()));
    remove_path(&staging).ok();
    std::fs::create_dir_all(&staging)?;

    let unpack = || -> Result<()> {
        let file = std::fs::File::open(archive)?;
        let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(std::io::BufReader::new(file)));
        // `unpack` refuses entries that would escape the destination.
        tar.unpack(&staging).context("unpack voice archive")?;
        let inner = staging.join(root);
        let inner = if inner.is_dir() {
            inner
        } else {
            // Fall back to the single top-level folder, whatever it is called.
            let mut dirs = std::fs::read_dir(&staging)?.flatten().map(|e| e.path()).filter(|p| p.is_dir());
            match (dirs.next(), dirs.next()) {
                (Some(only), None) => only,
                _ => bail!("unexpected voice archive layout"),
            }
        };
        if !inner.join("tokens.txt").is_file() {
            bail!("the voice archive has no tokens.txt");
        }
        remove_path(target).ok();
        std::fs::rename(&inner, target).context("install voice folder")?;
        Ok(())
    };
    let result = unpack();
    remove_path(&staging).ok();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(part_path(Path::new("/m/stt/ggml-base.en.bin")), PathBuf::from("/m/stt/ggml-base.en.bin.part"));
        assert_eq!(part_path(Path::new("/m/tts/voice")), PathBuf::from("/m/tts/voice.part"));
    }

    #[test]
    fn extracts_voice_archive() {
        let root = std::env::temp_dir().join(format!("jarvis-extract-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/vits-test/espeak-ng-data")).unwrap();
        std::fs::write(root.join("src/vits-test/tokens.txt"), b"a 1").unwrap();
        std::fs::write(root.join("src/vits-test/model.onnx"), b"onnx").unwrap();
        std::fs::write(root.join("src/vits-test/espeak-ng-data/phontab"), b"p").unwrap();

        let archive = root.join("voice.tar.bz2");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let enc = bzip2::write::BzEncoder::new(file, bzip2::Compression::fast());
            let mut tar = tar::Builder::new(enc);
            tar.append_dir_all("vits-test", root.join("src/vits-test")).unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }

        let target = root.join("tts/my-voice");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        extract_voice(&archive, &target, "vits-test").unwrap();
        assert!(target.join("tokens.txt").is_file());
        assert!(target.join("model.onnx").is_file());
        assert!(target.join("espeak-ng-data/phontab").is_file());
        // No staging folder is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(root.join("tts")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hex_encoding() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }
}
