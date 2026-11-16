use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::domain::Id;
use crate::persistence::Db;
use crate::services::ai::Providers;
use crate::services::embedding::engine::Embedder;
use crate::services::ingestion::enrich::Enricher;
use crate::services::ingestion::ocr::OcrModel;
use crate::services::ingestion::queue::Ingestor;

/// Running generations, keyed by the assistant message id.
#[derive(Clone, Default)]
pub struct Generations(Arc<Mutex<HashMap<Id, CancellationToken>>>);

impl Generations {
    pub fn register(&self, message_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        if let Ok(mut map) = self.0.lock() {
            map.insert(message_id.to_string(), token.clone());
        }
        token
    }

    pub fn remove(&self, message_id: &str) {
        if let Ok(mut map) = self.0.lock() {
            map.remove(message_id);
        }
    }

    /// True if a generation was running.
    pub fn cancel(&self, message_id: &str) -> bool {
        match self.0.lock().ok().and_then(|m| m.get(message_id).cloned()) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub providers: Providers,
    pub generations: Generations,
    pub ingest: Ingestor,
    pub embedder: Embedder,
    pub ocr: OcrModel,
    pub layout: OcrModel,
    pub enricher: Enricher,
    /// Project that dropped files are added to, set by the UI.
    pub import_target: Arc<Mutex<Option<Id>>>,
}
