//! Which documents have a details refresh or lookup in flight, so the library can show it.
//! A document counts once per job (file read, then Crossref and model lookup) and is listed
//! until the last one finishes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::domain::Id;

/// Sent whenever the list changes. `ids` is everything still being worked on.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct DetailsActivity {
    pub ids: Vec<Id>,
}

type Sink = Arc<dyn Fn(DetailsActivity) + Send + Sync>;

#[derive(Clone)]
pub struct Activity {
    jobs: Arc<Mutex<HashMap<Id, u32>>>,
    sink: Sink,
}

impl Activity {
    pub fn new(sink: impl Fn(DetailsActivity) + Send + Sync + 'static) -> Self {
        Self {
            jobs: Arc::default(),
            sink: Arc::new(sink),
        }
    }

    pub fn start(&self, id: &str) {
        self.change(|j| *j.entry(id.to_string()).or_default() += 1);
    }

    pub fn finish(&self, id: &str) {
        self.change(|j| {
            if let Some(n) = j.get_mut(id) {
                *n -= 1;
                if *n == 0 {
                    j.remove(id);
                }
            }
        });
    }

    fn change(&self, f: impl FnOnce(&mut HashMap<Id, u32>)) {
        let ids = {
            let Ok(mut jobs) = self.jobs.lock() else {
                return;
            };
            f(&mut jobs);
            jobs.keys().cloned().collect()
        };
        (self.sink)(DetailsActivity { ids });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_stays_listed_until_its_last_job_finishes() {
        let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
        let sink = seen.clone();
        let a = Activity::new(move |e| sink.lock().unwrap().push(e.ids.len()));
        a.start("x");
        a.start("x");
        a.finish("x");
        a.finish("x");
        a.finish("x");
        assert_eq!(*seen.lock().unwrap(), vec![1, 1, 1, 0, 0]);
    }
}
