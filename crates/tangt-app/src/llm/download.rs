//! Fetching model weights: resumable, verified, and cancellable.
//!
//! A download lands in a `.part` file and is only renamed into place once its
//! SHA-256 matches the source it came from. A partial file is resumed with a
//! Range request; if the server ignores the range, the file is restarted
//! rather than silently concatenated.

use super::catalog::{CatalogEntry, ModelSource};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CHUNK: usize = 1 << 16;

/// Live state of one download, polled by the UI.
#[derive(Default)]
pub struct Progress {
    pub downloaded: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    /// Set when the download finishes, successfully or not.
    pub outcome: Mutex<Option<Result<PathBuf, String>>>,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.downloaded.load(Ordering::Relaxed) as f64 / total as f64) as f32
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn take_outcome(&self) -> Option<Result<PathBuf, String>> {
        self.outcome.lock().ok().and_then(|mut o| o.take())
    }
}

/// Download `entry` into `dir` on this thread, reporting into `progress`.
/// Tries each mirror in turn; the file is verified against whichever answered.
pub fn fetch(entry: &CatalogEntry, dir: &Path, progress: &Arc<Progress>) -> Result<PathBuf, String> {
    let final_path = entry.path_in(dir);
    if final_path.is_file() {
        return Ok(final_path);
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    let part = final_path.with_extension("part");
    let mut last_error = String::new();
    for source in &entry.sources {
        if progress.is_cancelled() {
            return Err("cancelled".into());
        }
        progress.total.store(source.size, Ordering::Relaxed);
        match fetch_from(source, &part, progress) {
            Ok(()) => {
                std::fs::rename(&part, &final_path)
                    .map_err(|e| format!("could not put the model in place: {e}"))?;
                return Ok(final_path);
            }
            Err(e) => {
                // A mirror that failed verification left a bad file behind.
                let _ = std::fs::remove_file(&part);
                if progress.is_cancelled() {
                    return Err("cancelled".into());
                }
                last_error = e;
            }
        }
    }
    Err(last_error)
}

fn fetch_from(
    source: &ModelSource,
    part: &Path,
    progress: &Arc<Progress>,
) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        // No global timeout: this is gigabytes over an unknown connection.
        .build()
        .into();

    let have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let mut request = agent.get(source.url);
    if have > 0 {
        request = request.header("Range", &format!("bytes={have}-"));
    }
    let response = request
        .call()
        .map_err(|e| format!("could not reach {}: {e}", short_host(source.url)))?;

    // 206 means the server honoured the range; 200 means it sent the whole
    // file and anything already on disk must be discarded.
    let resuming = response.status().as_u16() == 206 && have > 0;
    let mut file = if resuming {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(part)
            .map_err(|e| format!("could not reopen the partial download: {e}"))?;
        f.seek(SeekFrom::End(0))
            .map_err(|e| format!("could not seek the partial download: {e}"))?;
        progress.downloaded.store(have, Ordering::Relaxed);
        f
    } else {
        progress.downloaded.store(0, Ordering::Relaxed);
        std::fs::File::create(part).map_err(|e| format!("could not create the download: {e}"))?
    };

    let mut reader = response.into_body().into_reader();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        if progress.is_cancelled() {
            return Err("cancelled".into());
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|e| format!("the download stopped: {e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|e| format!("could not write the download: {e}"))?;
        progress.downloaded.fetch_add(read as u64, Ordering::Relaxed);
    }
    file.flush().map_err(|e| format!("could not flush the download: {e}"))?;
    drop(file);

    verify(part, source.sha256)
}

/// A downloaded file is only a model if its checksum matches.
pub fn verify(path: &Path, expected: &str) -> Result<(), String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("could not read the download back: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("could not read the download back: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hasher.finalize();
    let actual = actual.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "the download did not match its checksum (expected {expected}, got {actual}) — \
             it may have been truncated or tampered with"
        ))
    }
}

fn short_host(url: &str) -> &str {
    url.strip_prefix("https://")
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
}

pub fn human_size(bytes: u64) -> String {
    const GB: u64 = 1 << 30;
    const MB: u64 = 1 << 20;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{} MB", bytes / MB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_accepts_a_known_hash_and_rejects_a_wrong_one() {
        let path = std::env::temp_dir().join("tangt-verify-test.bin");
        std::fs::write(&path, b"abc").unwrap();
        // SHA-256 of "abc".
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify(&path, expected).is_ok());

        let err = verify(&path, &"0".repeat(64)).unwrap_err();
        assert!(err.contains("did not match its checksum"), "{err}");
        assert!(err.contains(expected), "the real hash should be reported: {err}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_already_installed_model_is_not_refetched() {
        let dir = std::env::temp_dir().join("tangt-fetch-test");
        std::fs::create_dir_all(&dir).unwrap();
        let entry = &super::super::catalog::CATALOG[0];
        let path = entry.path_in(&dir);
        std::fs::write(&path, b"pretend weights").unwrap();
        // Would fail instantly if it tried to reach the network.
        let progress = Arc::new(Progress::default());
        assert_eq!(fetch(entry, &dir, &progress).unwrap(), path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cancellation_is_observed_before_any_request() {
        let dir = std::env::temp_dir().join("tangt-cancel-test");
        let progress = Arc::new(Progress::default());
        progress.cancel();
        let err = fetch(&super::super::catalog::CATALOG[0], &dir, &progress).unwrap_err();
        assert_eq!(err, "cancelled");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn progress_fraction_is_safe_before_the_total_is_known() {
        let p = Progress::default();
        assert_eq!(p.fraction(), 0.0);
        p.total.store(200, Ordering::Relaxed);
        p.downloaded.store(50, Ordering::Relaxed);
        assert!((p.fraction() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn sizes_read_the_way_a_person_would_say_them() {
        assert_eq!(human_size(2_497_281_120), "2.3 GB");
        assert_eq!(human_size(700 * (1 << 20)), "700 MB");
    }
}
