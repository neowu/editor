use settings::ScanSymlinksSetting;
use settings::Settings;
use util::paths::PathMatcher;
use util::rel_path::RelPath;

#[derive(Clone, PartialEq, Eq)]
pub struct WorktreeSettings {
    pub file_scan_exclusions: PathMatcher,
    pub file_scan_inclusions: PathMatcher,
    /// This field contains all ancestors of the `file_scan_inclusions`. It's used to
    /// determine whether to terminate worktree scanning for a given dir.
    pub parent_dir_scan_inclusions: PathMatcher,
    pub scan_symlinks: ScanSymlinksSetting,
    pub file_scan_depth: Option<u32>,
    pub hidden_files: PathMatcher,
}

settings::register_setting!(WorktreeSettings);

impl WorktreeSettings {
    pub fn is_path_excluded(&self, path: &RelPath) -> bool {
        path.ancestors().any(|ancestor| self.file_scan_exclusions.is_match(ancestor))
    }

    pub fn is_path_always_included(&self, path: &RelPath, is_dir: bool) -> bool {
        if is_dir { self.parent_dir_scan_inclusions.is_match(path) } else { self.file_scan_inclusions.is_match(path) }
    }

    pub fn is_path_hidden(&self, path: &RelPath) -> bool {
        path.ancestors().any(|ancestor| self.hidden_files.is_match(ancestor))
    }
}

impl Settings for WorktreeSettings {
    fn built_in() -> Self {
        let file_scan_exclusions = vec![
            "**/.git".into(),
            "**/.svn".into(),
            "**/.hg".into(),
            "**/.jj".into(),
            "**/.sl".into(),
            "**/.repo".into(),
            "**/CVS".into(),
            "**/.DS_Store".into(),
            "**/Thumbs.db".into(),
            "**/.classpath".into(),
            "**/.settings".into(),
        ];
        let (file_scan_inclusions, parent_dir_scan_inclusions) = file_scan_inclusion_matchers(vec![".env*".into()]);
        Self {
            file_scan_exclusions: valid_path_matchers(file_scan_exclusions, "file_scan_exclusions"),
            parent_dir_scan_inclusions,
            file_scan_inclusions,
            hidden_files: valid_path_matchers(vec!["**/.*".into()], "hidden_files"),
            scan_symlinks: ScanSymlinksSetting::Expanded,
            file_scan_depth: Some(5),
        }
    }
}

fn valid_path_matchers(mut values: Vec<String>, context: &'static str) -> PathMatcher {
    values.sort();
    PathMatcher::new_lenient(values, |error| {
        log::error!("Failed to compile patterns in `{context}`: {error}");
    })
}

fn file_scan_inclusion_matchers(mut values: Vec<String>) -> (PathMatcher, PathMatcher) {
    values.sort();
    let mut errors = Vec::new();
    let inclusions = PathMatcher::new_lenient(&values, |error| {
        errors.push(error);
    });
    let parent_inclusions = values
        .iter()
        .filter(|pattern| !errors.iter().any(|error| error.glob().is_none_or(|invalid| invalid == pattern.as_str())))
        .flat_map(|pattern| inclusion_parent_patterns(pattern))
        .collect();
    for error in errors {
        log::error!("Failed to compile patterns in `file_scan_inclusions`: {error}");
    }
    (inclusions, valid_path_matchers(parent_inclusions, "file_scan_inclusions"))
}

fn inclusion_parent_patterns(pattern: &str) -> Vec<String> {
    let mut parents = Vec::new();
    let mut open_braces = 0_usize;
    let mut class_start = None;
    let mut escaped = false;
    for (index, character) in pattern.char_indices() {
        let is_separator = character == '/';
        let mut split_index = index;
        if class_start.is_none() {
            if escaped {
                escaped = false;
                if !is_separator {
                    continue;
                }
                split_index -= 1;
            } else if character == '\\' {
                escaped = true;
                continue;
            }
        }
        if let Some(start) = class_start {
            let first_content = start + 1 + usize::from(matches!(pattern.as_bytes().get(start + 1), Some(b'!' | b'^')));
            if character == ']' && index > first_content {
                class_start = None;
            }
        } else {
            match character {
                '[' => class_start = Some(index),
                '{' => open_braces += 1,
                '}' => open_braces = open_braces.saturating_sub(1),
                _ => {}
            }
        }
        if is_separator {
            let closing_braces = "}".repeat(open_braces);
            if let Some(start) = class_start {
                parents.push(format!("{}*{closing_braces}", &pattern[..start]));
            } else if open_braces > 0 {
                parents.push(format!("{}{closing_braces}", &pattern[..split_index]));
            } else if split_index > 0 {
                parents.push(pattern[..split_index].to_string());
            }
        }
    }
    parents.sort();
    parents.dedup();
    parents
}

#[cfg(test)]
mod tests {
    use util::rel_path::RelPath;

    use super::*;

    #[test]
    fn built_in_worktree_settings() {
        let settings = WorktreeSettings::built_in();
        assert!(settings.is_path_excluded(&RelPath::new_test("src/.git/HEAD")));
        assert!(!settings.is_path_excluded(&RelPath::new_test("src/main.rs")));
        assert!(settings.is_path_always_included(&RelPath::new_test(".env.local"), false));
        assert!(settings.is_path_hidden(&RelPath::new_test("a/.cache/b")));
        assert!(!settings.is_path_hidden(&RelPath::new_test("a/b")));
        assert_eq!(settings.scan_symlinks, ScanSymlinksSetting::Expanded);
        assert_eq!(settings.file_scan_depth, Some(5));
    }
}
