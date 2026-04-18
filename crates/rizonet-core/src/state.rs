//! Type-indexed shared state for command handlers.
//!
//! ```ignore
//! use rizonet_core::{AppBuilder, RizonetConfig, handle};
//! use std::sync::Mutex;
//!
//! struct Db(Mutex<Vec<String>>);
//!
//! AppBuilder::new(cfg)
//!     .manage(Db(Mutex::new(vec![])))
//!     .run()?;
//!
//! // Later, from any IPC handler:
//! let db = handle().state::<Db>().unwrap();
//! db.0.lock().unwrap().push("hi".into());
//! ```

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Shared, type-indexed container. Values are cloned out as `Arc<T>` so that
/// handlers can keep a cheap reference without holding a lock.
#[derive(Default, Clone)]
pub struct StateMap {
    inner: Arc<RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>>,
}

impl StateMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a value keyed by its concrete type. Replaces any previous value
    /// of the same type.
    pub fn insert<T: Send + Sync + 'static>(&self, value: T) {
        let mut guard = self.inner.write().expect("state poisoned");
        guard.insert(TypeId::of::<T>(), Arc::new(value));
    }

    /// Retrieve a previously inserted value.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let guard = self.inner.read().expect("state poisoned");
        guard
            .get(&TypeId::of::<T>())
            .and_then(|v| v.clone().downcast::<T>().ok())
    }
}
