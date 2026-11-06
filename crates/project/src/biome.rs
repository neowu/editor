//! Built-in formatting via the Biome CLI (`biome format --stdin-file-path <path>`).
//!
//! Each format spawns a short-lived process; Biome is a native binary, so there is
//! no long-running server to manage.

use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

use language::Language;

/// Languages that `formatter: "auto"` formats with Biome.
const LANGUAGES: &[&str] = &["CSS", "GraphQL", "JavaScript", "JSON", "JSONC", "TSX", "TypeScript"];

const NPX_PACKAGE: &str = "@biomejs/biome";

pub(crate) fn supports(language: Option<&Language>) -> bool {
    language.is_some_and(|language| LANGUAGES.contains(&language.name().as_ref()))
}

/// The program and leading arguments used to invoke Biome.
pub(crate) struct BiomeCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Resolves Biome, in order: the nearest `node_modules/.bin/biome` above `start_dir`,
/// `biome` on `PATH`, then `npx --yes @biomejs/biome`.
pub(crate) fn resolve_command(start_dir: Option<&Path>, path_env: Option<&str>) -> BiomeCommand {
    let local = start_dir.into_iter().flat_map(Path::ancestors).find_map(|dir| {
        let candidate = dir.join("node_modules").join(".bin").join("biome");
        candidate.is_file().then_some(candidate)
    });
    if let Some(program) = local {
        return BiomeCommand { program, args: Vec::new() };
    }

    let cwd = start_dir.unwrap_or_else(|| Path::new("/"));
    if let Ok(program) = which::which_in("biome", path_env, cwd) {
        return BiomeCommand { program, args: Vec::new() };
    }

    BiomeCommand {
        program: which::which_in("npx", path_env, cwd).unwrap_or_else(|_| "npx".into()),
        args: vec!["--yes".into(), NPX_PACKAGE.into()],
    }
}

/// The path passed as `--stdin-file-path`; Biome picks the language and config from it.
/// Unsaved buffers get a synthetic name from the language's first path suffix.
pub(crate) fn stdin_file_path(abs_path: Option<&Path>, language: Option<&Language>) -> PathBuf {
    match abs_path {
        Some(path) => path.to_path_buf(),
        None => {
            let suffix = language
                .and_then(|language| language.config().matcher.path_suffixes.first().cloned())
                .unwrap_or_default();
            PathBuf::from(format!("untitled.{suffix}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::fs::Permissions;
    use std::process;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("biome-{name}-{}", process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prefers_nearest_node_modules_bin() {
        let root = temp_dir("local");
        let bin = root.join("node_modules/.bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("biome"), "").unwrap();
        let nested = root.join("packages/app/src");
        fs::create_dir_all(&nested).unwrap();

        let command = resolve_command(Some(&nested), Some(""));
        assert_eq!(command.program, bin.join("biome"));
        assert!(command.args.is_empty());
    }

    #[test]
    fn falls_back_to_path_then_npx() {
        let root = temp_dir("fallback");
        let path_dir = root.join("path_bin");
        fs::create_dir_all(&path_dir).unwrap();
        let path_env = path_dir.to_string_lossy().to_string();

        let command = resolve_command(Some(&root), Some(&path_env));
        assert_eq!(command.program, PathBuf::from("npx"));
        assert_eq!(command.args, vec![OsString::from("--yes"), NPX_PACKAGE.into()]);

        let biome = path_dir.join("biome");
        fs::write(&biome, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&biome, Permissions::from_mode(0o755)).unwrap();
        }
        let command = resolve_command(Some(&root), Some(&path_env));
        assert_eq!(command.program, biome);
        assert!(command.args.is_empty());
    }
}
