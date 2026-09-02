use std::collections::HashMap;
use std::sync::Arc;

use crate::{BenchmarkAdapter, BenchmarkError, BenchmarkId, Result};

#[derive(Default)]
pub struct BenchmarkRegistry {
    adapters: HashMap<String, Arc<dyn BenchmarkAdapter>>,
}

impl BenchmarkRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, adapter: Arc<dyn BenchmarkAdapter>) -> Result<()> {
        let id = adapter.descriptor().id().as_str().to_owned();
        if self.adapters.contains_key(&id) {
            return Err(BenchmarkError::coded("duplicate_benchmark"));
        }
        self.adapters.insert(id, adapter);
        Ok(())
    }

    pub fn get(&self, id: &BenchmarkId) -> Result<Arc<dyn BenchmarkAdapter>> {
        self.adapters
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| BenchmarkError::coded("unknown_benchmark"))
    }

    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }
}
