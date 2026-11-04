use std::path::PathBuf;

use fs::MTime;
use serde::Deserialize;
use serde::Serialize;

/// What `.ed/workspace.json` keeps for an editor tab.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct SerializedEditor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) abs_path: Option<PathBuf>,
    /// Unsaved text, kept so it survives a restart (hot exit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) contents: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) language: Option<String>,
    /// Modification time of the file `contents` was edited from, as seconds
    /// and nanoseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mtime: Option<(u64, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scroll: Option<SerializedScroll>,
    /// Selections as buffer byte offsets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) selections: Vec<(usize, usize)>,
}

impl SerializedEditor {
    pub(crate) fn mtime(&self) -> Option<MTime> {
        self.mtime.map(|(seconds, nanos)| MTime::from_seconds_and_nanos(seconds, nanos))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct SerializedScroll {
    pub(crate) top_row: u32,
    pub(crate) offset_x: f64,
    pub(crate) offset_y: f64,
}

#[cfg(test)]
mod tests {
    use super::SerializedEditor;
    use super::SerializedScroll;

    #[test]
    fn test_serialized_editor_round_trip() {
        let editor = SerializedEditor {
            abs_path: Some("/p/a.rs".into()),
            contents: Some("unsaved".into()),
            language: Some("Rust".into()),
            mtime: Some((10, 20)),
            scroll: Some(SerializedScroll { top_row: 3, offset_x: 0., offset_y: 0.5 }),
            selections: vec![(1, 2)],
        };
        let json = serde_json::to_value(&editor).unwrap();
        assert_eq!(serde_json::from_value::<SerializedEditor>(json).unwrap(), editor);
        assert_eq!(serde_json::to_value(SerializedEditor::default()).unwrap(), serde_json::json!({}));
        assert_eq!(editor.mtime().and_then(|mtime| mtime.to_seconds_and_nanos_for_persistence()), Some((10, 20)));
    }
}
