//! File open/save and recents. The `.tangt` text is the sole source of truth.

use std::path::{Path, PathBuf};
use tangt_core::document::Document;
use tangt_core::resolve::today_local;

pub fn open_document(path: &Path) -> std::io::Result<Document> {
    let text = std::fs::read_to_string(path)?;
    Ok(Document::new(text, today_local()))
}

pub fn save_document(doc: &mut Document, path: &Path) -> std::io::Result<()> {
    std::fs::write(path, doc.text())?;
    doc.mark_saved();
    Ok(())
}

pub fn pick_open() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("tangt chart", &["tangt"])
        .pick_file()
}

pub fn pick_save() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("tangt chart", &["tangt"])
        .set_file_name("chart.tangt")
        .save_file()
}

pub fn push_recent(recents: &mut Vec<PathBuf>, path: &Path) {
    recents.retain(|p| p != path);
    recents.insert(0, path.to_path_buf());
    recents.truncate(10);
}
