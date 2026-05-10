use anyhow::Result;
use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

pub struct PathWatcher {
    debouncer: notify_debouncer_full::Debouncer<
        notify::RecommendedWatcher,
        notify_debouncer_full::RecommendedCache,
    >,
    rx: mpsc::Receiver<Vec<PathBuf>>,
    watched: BTreeSet<PathBuf>,
}

impl PathWatcher {
    pub fn new(timeout: Duration) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let debouncer = new_debouncer(timeout, None, move |result: DebounceEventResult| {
            let Ok(events) = result else {
                return;
            };
            let mut deduped = BTreeSet::new();
            for event in events {
                for path in event.event.paths {
                    deduped.insert(path);
                }
            }
            if !deduped.is_empty() {
                let _ = tx.send(deduped.into_iter().collect());
            }
        })?;
        Ok(Self {
            debouncer,
            rx,
            watched: BTreeSet::new(),
        })
    }

    pub fn watch_paths(&mut self, paths: &[PathBuf]) -> Result<()> {
        for p in paths {
            if self.watched.insert(p.clone()) {
                self.debouncer.watch(p, RecursiveMode::Recursive)?;
            }
        }
        Ok(())
    }

    pub fn sync_paths(&mut self, paths: &[PathBuf]) -> Result<()> {
        let desired = paths.iter().cloned().collect::<BTreeSet<_>>();

        let to_unwatch = self
            .watched
            .difference(&desired)
            .cloned()
            .collect::<Vec<_>>();
        for path in &to_unwatch {
            let _ = self.debouncer.unwatch(path);
        }
        for path in to_unwatch {
            self.watched.remove(&path);
        }

        let to_watch = desired
            .difference(&self.watched)
            .cloned()
            .collect::<Vec<_>>();
        for path in &to_watch {
            self.debouncer.watch(path, RecursiveMode::Recursive)?;
        }
        for path in to_watch {
            self.watched.insert(path);
        }
        Ok(())
    }

    pub fn recv_changed_paths(&self, timeout: Duration) -> Option<Vec<PathBuf>> {
        self.rx.recv_timeout(timeout).ok()
    }
}
