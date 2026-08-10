//! Which models tangt knows how to fetch and run in process.
//!
//! Weights are never bundled — they dwarf the binary and carry their own
//! licences — so this is a catalogue: where each model lives, what it is
//! called on disk, and the checksums that say a download arrived intact.
//!
//! Each entry lists two independent sources. They are separate quantizations
//! by different people, so each carries its own checksum and size: whichever
//! one answers, the file is verified against *that* source.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelSource {
    pub url: &'static str,
    /// Lowercase hex SHA-256, from the Hugging Face LFS pointer.
    pub sha256: &'static str,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Stable identifier, stored in preferences.
    pub id: &'static str,
    pub label: &'static str,
    /// tangt's own name for the file — the sources disagree about theirs.
    pub filename: &'static str,
    /// Mirrors, tried in order.
    pub sources: [ModelSource; 2],
    pub summary: &'static str,
    pub license: &'static str,
    pub attribution: &'static str,
    pub attribution_url: &'static str,
}

impl CatalogEntry {
    /// Roughly how large the download is, for the UI.
    pub fn size(&self) -> u64 {
        self.sources[0].size
    }

    pub fn path_in(&self, dir: &Path) -> PathBuf {
        dir.join(self.filename)
    }

    pub fn installed_in(&self, dir: &Path) -> Option<PathBuf> {
        let path = self.path_in(dir);
        path.is_file().then_some(path)
    }
}

/// The models offered in Preferences; the first is the default.
pub static CATALOG: [CatalogEntry; 2] = [
    CatalogEntry {
        id: "qwen3-4b-instruct-2507",
        label: "Qwen3 4B Instruct 2507",
        filename: "qwen3-4b-instruct-2507-q4_k_m.gguf",
        sources: [
            ModelSource {
                url: "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
                sha256: "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597",
                size: 2_497_281_120,
            },
            ModelSource {
                url: "https://huggingface.co/bartowski/Qwen_Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
                sha256: "2fde00ce69dd4899c70d020845e2638353015bba0fdf161b3eb965f2bca4464e",
                size: 2_497_280_736,
            },
        ],
        summary: "The default. Answers straight away and is the more reliable of the two at \
                  calling tools.",
        license: "Apache-2.0",
        attribution: "Qwen3-4B-Instruct-2507 © Alibaba Cloud, Apache-2.0",
        attribution_url: "https://huggingface.co/Qwen/Qwen3-4B-Instruct-2507",
    },
    CatalogEntry {
        id: "qwen3-1.7b",
        label: "Qwen3 1.7B",
        filename: "qwen3-1.7b-q4_k_m.gguf",
        sources: [
            ModelSource {
                url: "https://huggingface.co/unsloth/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q4_K_M.gguf",
                sha256: "b139949c5bd74937ad8ed8c8cf3d9ffb1e99c866c823204dc42c0d91fa181897",
                size: 1_107_409_472,
            },
            ModelSource {
                url: "https://huggingface.co/bartowski/Qwen_Qwen3-1.7B-GGUF/resolve/main/Qwen_Qwen3-1.7B-Q4_K_M.gguf",
                sha256: "72c5c3cb38fa32d5256e2fe30d03e7a64c6c79e668ad84057e3bd66e250b24fb",
                size: 1_282_439_584,
            },
        ],
        summary: "Less than half the size, for older machines. It reasons out loud first, so \
                  replies begin with its thinking.",
        license: "Apache-2.0",
        attribution: "Qwen3-1.7B © Alibaba Cloud, Apache-2.0",
        attribution_url: "https://huggingface.co/Qwen/Qwen3-1.7B",
    },
];

pub const DEFAULT_MODEL: &str = "qwen3-4b-instruct-2507";

/// Only the engine resolves an id to an entry; without it the catalogue is
/// still shown, but nothing looks a model up to run it.
#[cfg_attr(not(feature = "builtin-llm"), allow(dead_code))]
pub fn entry(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|e| e.id == id)
}

/// Where downloaded weights live. Path only — nothing is created until
/// something is actually downloaded.
pub fn models_dir() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "tangt")?;
    Some(dirs.data_dir().join("models"))
}

/// Total bytes the downloaded models take up.
pub fn disk_usage(dir: &Path) -> u64 {
    CATALOG
        .iter()
        .filter_map(|e| std::fs::metadata(e.path_in(dir)).ok())
        .map(|m| m.len())
        .sum()
}

/// Remove a downloaded model and any half-finished download of it.
pub fn delete_model(dir: &Path, entry: &CatalogEntry) -> std::io::Result<()> {
    // The file may be mapped into this process; on Windows that alone would
    // stop it being deleted.
    super::unload();
    let path = entry.path_in(dir);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let part = path.with_extension("part");
    match std::fs::remove_file(part) {
        Ok(()) | Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_entries_are_well_formed() {
        for e in &CATALOG {
            assert!(!e.id.is_empty() && !e.label.is_empty());
            assert!(e.filename.ends_with(".gguf"));
            for s in &e.sources {
                assert!(s.url.starts_with("https://"), "{} uses a plaintext URL", e.id);
                assert_eq!(s.sha256.len(), 64, "{} has a malformed checksum", e.id);
                assert!(
                    s.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
                    "{} checksum must be lowercase hex",
                    e.id
                );
                assert!(s.size > 100_000_000, "{} size looks wrong", e.id);
            }
            // Two genuinely independent mirrors, not the same file twice.
            assert_ne!(e.sources[0].url, e.sources[1].url);
            assert_ne!(e.sources[0].sha256, e.sources[1].sha256);
            assert!(!e.license.is_empty() && e.attribution_url.starts_with("https://"));
        }
    }

    #[test]
    fn default_model_is_in_the_catalogue() {
        assert!(entry(DEFAULT_MODEL).is_some());
        assert_eq!(CATALOG[0].id, DEFAULT_MODEL);
        assert!(entry("not-a-model").is_none());
    }
}
