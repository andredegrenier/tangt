//! Every mutation of a document is an `Op` — a set of position-based text
//! edits plus intent metadata. Ops are serializable and map 1:1 onto CRDT text
//! splices (automerge `splice_text`) should sync ever be wanted.

use crate::model::TaskId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    /// Byte range in the current text to replace.
    pub start: usize,
    pub end: usize,
    pub insert: String,
}

impl TextEdit {
    pub fn new(start: usize, end: usize, insert: impl Into<String>) -> Self {
        TextEdit { start, end, insert: insert.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intent {
    Typing,
    MoveBar(TaskId),
    ResizeStart(TaskId),
    ResizeEnd(TaskId),
    SetProgress(TaskId),
    Rename(TaskId),
    AddDependency,
    AddTask,
    AddSection,
    DeleteTask,
    DeleteSection,
    Reorder(TaskId),
    Paste,
    FileLoad,
}

impl Intent {
    /// Consecutive ops with the same coalescing key merge into one undo step
    /// (drag, drag, drag = one ⌘Z).
    pub fn coalesce_key(&self) -> Option<(&'static str, &TaskId)> {
        match self {
            Intent::MoveBar(id) => Some(("move", id)),
            Intent::ResizeStart(id) => Some(("resize-start", id)),
            Intent::ResizeEnd(id) => Some(("resize-end", id)),
            Intent::SetProgress(id) => Some(("progress", id)),
            Intent::Reorder(id) => Some(("reorder", id)),
            _ => None,
        }
    }
}

pub type ActorId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Op {
    /// Applied atomically, back-to-front (descending start offset), so ranges
    /// refer to the text as it was before the op.
    pub edits: Vec<TextEdit>,
    pub intent: Intent,
    pub actor: ActorId,
    pub seq: u64,
}
