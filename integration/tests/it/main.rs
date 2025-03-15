use std::path::PathBuf;

mod extract;
mod list;
mod reexport;

/// The root directory of the workspace.
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
