fn main() {
    hide_build_tree_from_spotlight();
    tauri_build::build()
}

/// Spotlight indexes `target/release/bundle/macos/UsageBar.app` as a second
/// app, labeled "macos" because that is the folder name. An empty
/// `.metadata_never_index` tells Spotlight to skip the whole build tree so
/// only `/Applications/UsageBar.app` shows up.
fn hide_build_tree_from_spotlight() {
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let mut dir = std::path::PathBuf::from(out_dir);
    for _ in 0..8 {
        if dir.file_name().is_some_and(|name| name == "target") {
            let _ = std::fs::write(dir.join(".metadata_never_index"), []);
            return;
        }
        if !dir.pop() {
            return;
        }
    }
}
