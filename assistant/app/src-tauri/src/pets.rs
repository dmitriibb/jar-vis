//! Custom pet skins: `<home>/assistant/pets/<name>/pet.json` plus image files.
//!
//! Images are handed to the pet window as data URLs, so the webview needs no
//! file-system access.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

/// Sprite sheets are small; anything bigger than this is a mistake.
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    name: String,
    #[serde(default = "default_size")]
    size: u32,
    /// The code-drawn pet has no image states.
    #[serde(default)]
    builtin: bool,
    #[serde(default)]
    states: BTreeMap<String, Animation>,
}

fn default_size() -> u32 {
    128
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Animation {
    /// File name in the manifest, a data URL once loaded.
    pub src: String,
    /// Cells in a horizontal sprite sheet. Omitted for animated gif/webp/apng.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f32>,
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub looping: Option<bool>,
    /// Scale the pet with the microphone level while this state shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic_level: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct Skin {
    pub name: String,
    pub size: u32,
    pub states: BTreeMap<String, Animation>,
}

fn mime_for(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" | "apng" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        _ => None,
    }
}

/// A plain folder or file name: no separators, no `..`.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

/// Load a skin. `Ok(None)` means "use the built-in pet".
pub fn load(pets_dir: &Path, name: &str) -> Result<Option<Skin>> {
    if !is_plain_name(name) {
        bail!("invalid pet name");
    }
    let dir = pets_dir.join(name);
    let manifest_path = dir.join("pet.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let manifest: Manifest = serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)
        .with_context(|| format!("invalid {}", manifest_path.display()))?;
    if manifest.builtin || manifest.states.is_empty() {
        return Ok(None);
    }

    let mut states = BTreeMap::new();
    for (state, mut anim) in manifest.states {
        if !is_plain_name(&anim.src) {
            bail!("pet state \"{state}\": src must be a file name inside the pet folder");
        }
        let file = dir.join(&anim.src);
        let mime = mime_for(&file).with_context(|| format!("pet state \"{state}\": unsupported image type"))?;
        let size = std::fs::metadata(&file).with_context(|| format!("pet image {} is missing", file.display()))?.len();
        if size > MAX_IMAGE_BYTES {
            bail!("pet image {} is too large", file.display());
        }
        let bytes = std::fs::read(&file)?;
        anim.src = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
        states.insert(state, anim);
    }
    Ok(Some(Skin {
        name: if manifest.name.is_empty() { name.to_string() } else { manifest.name },
        size: manifest.size,
        states,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jarvis-pets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtin_and_missing_use_the_default() {
        let dir = temp("builtin");
        std::fs::create_dir_all(dir.join("default")).unwrap();
        std::fs::write(dir.join("default/pet.json"), r#"{"name":"default","builtin":true}"#).unwrap();
        assert!(load(&dir, "default").unwrap().is_none());
        assert!(load(&dir, "nope").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_sprites_as_data_urls() {
        let dir = temp("cat");
        std::fs::create_dir_all(dir.join("cat")).unwrap();
        std::fs::write(dir.join("cat/idle.png"), [1u8, 2, 3]).unwrap();
        std::fs::write(
            dir.join("cat/pet.json"),
            r#"{"name":"Cat","size":96,"states":{"idle":{"src":"idle.png","frames":8,"fps":6,"loop":true},
               "listening":{"src":"idle.png","micLevel":true}}}"#,
        )
        .unwrap();
        let skin = load(&dir, "cat").unwrap().unwrap();
        assert_eq!(skin.name, "Cat");
        assert_eq!(skin.size, 96);
        let idle = &skin.states["idle"];
        assert_eq!(idle.src, "data:image/png;base64,AQID");
        assert_eq!(idle.frames, Some(8));
        assert_eq!(idle.looping, Some(true));
        assert_eq!(skin.states["listening"].mic_level, Some(true));
        // `loop` and `micLevel` keep their manifest spelling on the way out.
        let json = serde_json::to_string(&skin).unwrap();
        assert!(json.contains("\"loop\":true") && json.contains("\"micLevel\":true"), "{json}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_paths_outside_the_pet_folder() {
        let dir = temp("evil");
        std::fs::create_dir_all(dir.join("evil")).unwrap();
        std::fs::write(dir.join("evil/pet.json"), r#"{"states":{"idle":{"src":"../../secret.png"}}}"#).unwrap();
        assert!(load(&dir, "evil").is_err());
        assert!(load(&dir, "../evil").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
