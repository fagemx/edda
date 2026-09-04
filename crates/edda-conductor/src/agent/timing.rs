//! Process lifetime evidence. A failed spawn or unconfirmed reap stays unknown.
use std::sync::Mutex;
use std::time::Instant;

#[derive(Default)]
pub(crate) struct ProcessTiming(Mutex<Option<u64>>);

impl ProcessTiming {
    pub(crate) fn reset(&self) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = None;
        }
    }

    pub(crate) fn record(&self, started: Instant) {
        if let Ok(mut slot) = self.0.lock() {
            let elapsed = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            *slot = Some(slot.unwrap_or(0).saturating_add(elapsed));
        }
    }

    pub(crate) fn elapsed_ms(&self) -> Option<u64> {
        self.0.lock().ok().and_then(|slot| *slot)
    }
}
