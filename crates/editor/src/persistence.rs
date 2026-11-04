use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// What `.ed/workspace.json` keeps for an editor tab: the path of a file, or the
/// text of an untitled buffer. Files are always reloaded from disk.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct SerializedEditor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) abs_path: Option<PathBuf>,
    /// Unsaved text of an untitled buffer, kept so it survives a restart (hot exit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) contents: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::SerializedEditor;

    #[test]
    fn test_serialized_editor_round_trip() {
        for editor in [
            SerializedEditor { abs_path: Some("/p/a.rs".into()), contents: None },
            SerializedEditor { abs_path: None, contents: Some("unsaved".into()) },
        ] {
            let json = serde_json::to_value(&editor).unwrap();
            assert_eq!(serde_json::from_value::<SerializedEditor>(json).unwrap(), editor);
        }
        assert_eq!(serde_json::to_value(SerializedEditor::default()).unwrap(), serde_json::json!({}));
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
        assert_eq!(editor.abs_path, Some("/p/a.rs".into()));
    }
}
