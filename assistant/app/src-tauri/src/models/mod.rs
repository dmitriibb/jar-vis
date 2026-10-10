//! Speech models: the catalog, what is installed, downloading and deleting.

pub mod download;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::tts::VoiceSpec;

const CATALOG_JSON: &str = include_str!("../../resources/models.catalog.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    Stt,
    Tts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub kind: ModelKind,
    /// `whisper`, `piper` or `kokoro`.
    pub engine: String,
    pub display: String,
    pub url: String,
    /// Empty skips verification.
    #[serde(default)]
    pub sha256: String,
    pub size_mb: u32,
    pub ram_mb_est: u32,
    /// STT: the model file name.
    #[serde(default)]
    pub file: Option<String>,
    /// TTS: the top-level folder inside the downloaded archive.
    #[serde(default)]
    pub archive_root: Option<String>,
    #[serde(default)]
    pub license: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub id: String,
    pub kind: ModelKind,
    pub engine: String,
    pub display: String,
    pub size_mb: u32,
    pub ram_mb_est: u32,
    pub license: String,
    pub installed: bool,
    pub downloading: bool,
}

pub fn parse_catalog(json: &str) -> Result<Vec<CatalogEntry>> {
    let entries: Vec<CatalogEntry> = serde_json::from_str(json)?;
    for e in &entries {
        match e.kind {
            ModelKind::Stt if e.file.is_none() => bail!("catalog entry {} has no file", e.id),
            ModelKind::Tts if e.archive_root.is_none() => bail!("catalog entry {} has no archive_root", e.id),
            _ => {}
        }
    }
    Ok(entries)
}

pub struct ModelManager {
    catalog: Vec<CatalogEntry>,
    dir: Mutex<PathBuf>,
    /// Model id -> cancel flag of its running download.
    downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
    http: reqwest::Client,
}

impl ModelManager {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            catalog: parse_catalog(CATALOG_JSON).expect("bundled catalog is valid"),
            dir: Mutex::new(dir),
            downloads: Mutex::new(HashMap::new()),
            http: reqwest::Client::builder().user_agent("jar-vis").build().expect("http client"),
        }
    }

    pub fn set_dir(&self, dir: PathBuf) {
        *self.dir.lock() = dir;
    }

    fn dir(&self) -> PathBuf {
        self.dir.lock().clone()
    }

    pub fn entry(&self, id: &str) -> Option<&CatalogEntry> {
        self.catalog.iter().find(|e| e.id == id)
    }

    /// Where a model lives on disk: a file for STT, a folder for TTS.
    fn location(&self, entry: &CatalogEntry) -> PathBuf {
        match entry.kind {
            ModelKind::Stt => self.dir().join("stt").join(entry.file.as_deref().unwrap_or(&entry.id)),
            ModelKind::Tts => self.dir().join("tts").join(&entry.id),
        }
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.entry(id).is_some_and(|e| match e.kind {
            ModelKind::Stt => self.location(e).is_file(),
            ModelKind::Tts => self.location(e).join("tokens.txt").is_file(),
        })
    }

    pub fn list(&self) -> Vec<ModelStatus> {
        let downloads = self.downloads.lock();
        self.catalog
            .iter()
            .map(|e| ModelStatus {
                id: e.id.clone(),
                kind: e.kind,
                engine: e.engine.clone(),
                display: e.display.clone(),
                size_mb: e.size_mb,
                ram_mb_est: e.ram_mb_est,
                license: e.license.clone(),
                installed: self.is_installed(&e.id),
                downloading: downloads.contains_key(&e.id),
            })
            .collect()
    }

    /// Path of an installed STT model.
    pub fn stt_path(&self, id: &str) -> Result<PathBuf> {
        let entry = self.entry(id).filter(|e| e.kind == ModelKind::Stt).ok_or_else(|| anyhow!("unknown STT model \"{id}\""))?;
        let path = self.location(entry);
        if !path.is_file() {
            bail!("the speech model \"{id}\" is not downloaded yet — get it in Settings → Voice input");
        }
        Ok(path)
    }

    /// An installed TTS voice.
    pub fn voice(&self, id: &str) -> Result<VoiceSpec> {
        let entry = self.entry(id).filter(|e| e.kind == ModelKind::Tts).ok_or_else(|| anyhow!("unknown voice \"{id}\""))?;
        if !self.is_installed(id) {
            bail!("the voice \"{id}\" is not downloaded yet — get it in Settings → Voice output");
        }
        Ok(VoiceSpec { id: entry.id.clone(), dir: self.location(entry), engine: entry.engine.clone() })
    }

    /// Download and install a model. `on_progress(bytes, total)` is called as data arrives.
    pub async fn download(&self, id: &str, on_progress: impl Fn(u64, u64) + Send + Sync + 'static) -> Result<()> {
        let entry = self.entry(id).ok_or_else(|| anyhow!("unknown model \"{id}\""))?.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut downloads = self.downloads.lock();
            if downloads.contains_key(id) {
                bail!("\"{id}\" is already downloading");
            }
            downloads.insert(id.to_string(), cancel.clone());
        }
        let target = self.location(&entry);
        let result = download::install(&self.http, &entry, &target, cancel, on_progress).await;
        self.downloads.lock().remove(id);
        result
    }

    pub fn cancel_download(&self, id: &str) {
        if let Some(flag) = self.downloads.lock().get(id) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let entry = self.entry(id).ok_or_else(|| anyhow!("unknown model \"{id}\""))?;
        let path = self.location(entry);
        remove_path(&path)
    }
}

pub(crate) fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_is_valid() {
        let catalog = parse_catalog(CATALOG_JSON).unwrap();
        let ids: Vec<&str> = catalog.iter().map(|e| e.id.as_str()).collect();
        // The defaults in config.rs must exist in the catalog.
        let cfg = crate::config::Config::default();
        assert!(ids.contains(&cfg.stt.model.as_str()));
        assert!(ids.contains(&cfg.tts.voice.as_str()));
        for e in &catalog {
            assert!(e.url.starts_with("https://"), "{}", e.id);
            assert!(e.sha256.is_empty() || e.sha256.len() == 64, "{}", e.id);
            assert!(e.size_mb > 0);
        }
        // English only: no multilingual-only STT default.
        assert!(cfg.stt.model.contains(".en"));
    }

    #[test]
    fn catalog_rejects_incomplete_entries() {
        let bad = r#"[{"id":"x","kind":"stt","engine":"whisper","display":"X","url":"https://x","size_mb":1,"ram_mb_est":1}]"#;
        assert!(parse_catalog(bad).is_err());
    }

    #[test]
    fn installed_detection() {
        let root = std::env::temp_dir().join(format!("jarvis-models-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let m = ModelManager::new(root.clone());
        assert!(!m.is_installed("base.en-q5_1"));
        assert!(m.stt_path("base.en-q5_1").is_err());
        std::fs::create_dir_all(root.join("stt")).unwrap();
        std::fs::write(root.join("stt/ggml-base.en-q5_1.bin"), b"x").unwrap();
        assert!(m.is_installed("base.en-q5_1"));
        assert!(m.stt_path("base.en-q5_1").is_ok());

        let voice_dir = root.join("tts/piper-en_US-lessac-medium");
        std::fs::create_dir_all(&voice_dir).unwrap();
        assert!(!m.is_installed("piper-en_US-lessac-medium"));
        std::fs::write(voice_dir.join("tokens.txt"), b"x").unwrap();
        let voice = m.voice("piper-en_US-lessac-medium").unwrap();
        assert_eq!(voice.engine, "piper");
        m.delete("piper-en_US-lessac-medium").unwrap();
        assert!(!m.is_installed("piper-en_US-lessac-medium"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
