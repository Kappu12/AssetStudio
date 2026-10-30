use std::sync::{Arc, Mutex};
use std::time::Instant;

use assetstudio_core::{ProgressObserver, ProgressUpdate};

#[derive(Clone)]
pub struct Progress {
    state: Arc<Mutex<ProgressUpdate>>,
    started: Instant,
}

impl Default for Progress {
    fn default() -> Self {
        Self::new("Working")
    }
}

impl Progress {
    pub fn new(stage: &'static str) -> Self {
        Self {
            state: Arc::new(Mutex::new(ProgressUpdate {
                stage,
                completed: 0,
                total: None,
            })),
            started: Instant::now(),
        }
    }

    pub fn update(&self, stage: &'static str, completed: usize, total: Option<usize>) {
        if let Ok(mut state) = self.state.lock() {
            *state = ProgressUpdate {
                stage,
                completed,
                total,
            };
        }
    }

    pub fn observer(&self) -> ProgressObserver {
        let progress = self.clone();
        Arc::new(move |update| progress.update(update.stage, update.completed, update.total))
    }

    pub fn snapshot(&self) -> ProgressUpdate {
        self.state
            .lock()
            .map(|state| *state)
            .unwrap_or(ProgressUpdate {
                stage: "Working",
                completed: 0,
                total: None,
            })
    }

    pub fn elapsed(&self) -> String {
        let seconds = self.started.elapsed().as_secs();
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}
