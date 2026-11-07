use std::io;
use std::path::Path;

use smol::fs;
use smol::fs::unix::PermissionsExt;

/// Set the permissions for the given path so that the file becomes executable.
/// This is a noop for non-unix platforms.
pub async fn make_file_executable(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, <fs::Permissions as PermissionsExt>::from_mode(0o755)).await
}
