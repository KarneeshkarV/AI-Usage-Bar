use std::path::PathBuf;

/// Resolve a CLI binary by name. Honors an explicit override, then PATH.
pub fn resolve_binary(name: &str, override_path: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = override_path {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    which::which(name).ok()
}
