//! Which documents have a details refresh or lookup in flight, for the job list.
//! A document counts once per job (file read, then Crossref and model lookup) and is listed
//! until the last one finishes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::domain::Id;

type Sink = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Activity {
    jobs: Arc<Mutex<HashMap<Id, u32>>>,
    sink: Sink,
}

impl Activity {
    /// `changed` runs after every change to the list.
    pub fn new(changed: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            jobs: Arc::default(),
            sink: Arc::new(changed),
        }
    }

    /// Documents with a job queued or running.
    pub fn ids(&self) -> Vec<Id> {
        self.jobs
            .lock()
            .map(|j| j.keys().cloned().collect())
            .unwrap_or_default()
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
        {
            let Ok(mut jobs) = self.jobs.lock() else {
                return;
            };
            f(&mut jobs);
        }
        (self.sink)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_stays_listed_until_its_last_job_finishes() {
        let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
        let sink = seen.clone();
        let list = Arc::new(Mutex::new(None::<Activity>));
        let handle = list.clone();
        let a = Activity::new(move || {
            let n = handle.lock().unwrap().as_ref().map_or(0, |a| a.ids().len());
            sink.lock().unwrap().push(n);
        });
        *list.lock().unwrap() = Some(a.clone());
        a.start("x");
        a.start("x");
        a.finish("x");
        a.finish("x");
        a.finish("x");
        assert_eq!(*seen.lock().unwrap(), vec![1, 1, 1, 0, 0]);
    }
}
