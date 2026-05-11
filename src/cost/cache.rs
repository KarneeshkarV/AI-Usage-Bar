// Per-file incremental parse cache. Phase 2+ may flesh this out; for now the
// scanners re-read each file in full, which is fine for the JSONL volumes we
// see in practice (a few MB total).

use serde::{Deserialize, Serialize};

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileCache {
    pub mtime: u64,
    pub size: u64,
    pub parsed_bytes: u64,
}
