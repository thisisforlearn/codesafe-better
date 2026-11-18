//! Result cache keyed by content hash.
//!
//! The cache is a single JSON map under `.codesafe/cache/index.json`. Using one
//! document avoids the classic bug where an arbitrary key (`bench:rust:ab12:name`)
//! is written as a filename and either fails or escapes the cache directory.

use crate::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Cache {
    dir: PathBuf,
    path: PathBuf,
    map: HashMap<String, String>,
}

impl Cache {
    pub fn new(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| crate::Error::Cache(format!("{dir:?}: {e}")))?;
        let path = dir.join("index.json");
        let map = if path.is_file() {
            match std::fs::read_to_string(&path) {
                Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
                Err(_) => HashMap::new(),
            }
        } else {
            HashMap::new()
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            path,
            map,
        })
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.map.get(key).cloned()
    }

    pub fn put(&mut self, key: &str, value: &str) -> Result<()> {
        self.map.insert(key.to_string(), value.to_string());
        self.flush()
    }

    fn flush(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.map).map_err(crate::Error::Json)?;
        let tmp = self
            .dir
            .join(format!("index.json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, json).map_err(|e| crate::Error::Cache(format!("{tmp:?}: {e}")))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            crate::Error::Cache(format!("{:?}: {e}", self.path))
        })?;
        Ok(())
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            dir: PathBuf::new(),
            path: PathBuf::new(),
            map: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trip() {
        let dir = std::env::temp_dir().join(format!("codesafe-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = Cache::new(&dir).unwrap();
        c.put("a", "hello").unwrap();
        assert_eq!(c.get("a"), Some("hello".into()));
        assert_eq!(c.get("missing"), None);
    }

    #[test]
    fn cache_persists_and_handles_weird_keys() {
        let dir = std::env::temp_dir().join(format!("codesafe-cache2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = Cache::new(&dir).unwrap();
        let key = "bench:rust:abc/def:../evil name::x";
        c.put(key, "1.5").unwrap();
        let c2 = Cache::new(&dir).unwrap();
        assert_eq!(c2.get(key), Some("1.5".into()));
        assert!(dir.join("index.json").is_file());
    }

    #[test]
    fn corrupt_cache_falls_back_to_empty() {
        let dir = std::env::temp_dir().join(format!("codesafe-cache3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.json"), b"{not json").unwrap();
        let c = Cache::new(&dir).unwrap();
        assert_eq!(c.get("anything"), None);
    }
}
