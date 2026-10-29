//! Bounded caches for reloadable directory/project data, never learned history.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAX_ENTRIES: usize = 64;

pub(super) struct Cache<V> {
    entries: HashMap<PathBuf, (Instant, V)>,
}

impl<V> Default for Cache<V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<V> Cache<V> {
    pub fn get(&mut self, path: &Path) -> Option<&V> {
        let (used, value) = self.entries.get_mut(path)?;
        *used = Instant::now();
        Some(value)
    }

    pub fn insert(&mut self, path: PathBuf, value: V) {
        if self.entries.len() >= MAX_ENTRIES && !self.entries.contains_key(&path) {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (used, _))| used)
                .map(|(path, _)| path.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(path, (Instant::now(), value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_reloadable_data_and_keeps_recently_used_entries() {
        let mut cache = Cache::default();
        for i in 0..MAX_ENTRIES {
            cache.insert(PathBuf::from(i.to_string()), i);
        }
        assert_eq!(cache.get(Path::new("0")), Some(&0));
        cache.insert(PathBuf::from("extra"), 100);
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert_eq!(cache.get(Path::new("0")), Some(&0));
        assert_eq!(cache.get(Path::new("1")), None);
        cache.insert(PathBuf::from("0"), 200);
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert_eq!(cache.get(Path::new("0")), Some(&200));
    }
}
