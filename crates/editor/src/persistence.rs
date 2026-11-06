use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// What `.ed/workspace.json` keeps for an editor tab: the path of its file,
/// always reloaded from disk. Untitled buffers aren't kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SerializedEditor {
    pub(crate) abs_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::SerializedEditor;

    #[test]
    fn test_serialized_editor_round_trip() {
        let editor = SerializedEditor { abs_path: "/p/a.rs".into() };
        let json = serde_json::to_value(&editor).unwrap();
        assert_eq!(json, serde_json::json!({ "abs_path": "/p/a.rs" }));
        assert_eq!(serde_json::from_value::<SerializedEditor>(json).unwrap(), editor);
    }

    #[test]
    fn test_serialized_editor_ignores_legacy_fields() {
        let json = serde_json::json!({
            "abs_path": "/p/a.rs",
            "contents": "old",
            "language": "Rust",
            "mtime": [1, 2],
            "scroll": { "top_row": 3, "offset_x": 0.0, "offset_y": 0.0 },
            "selections": [[1, 2]],
        });
        let editor = serde_json::from_value::<SerializedEditor>(json).unwrap();
        assert_eq!(editor.abs_path, PathBuf::from("/p/a.rs"));
    }

    #[test]
    fn test_serialized_editor_rejects_legacy_untitled_buffer() {
        let json = serde_json::json!({ "contents": "unsaved" });
        serde_json::from_value::<SerializedEditor>(json).unwrap_err();
    }
}
