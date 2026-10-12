//! Observation of the result of the most recent adapter save.

use reedline::HistoryItemId;
use std::sync::{Arc, Mutex};

/// The result of the most recent adapter save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistorySaveOutcome {
    Saved(HistoryItemId),
    Failed,
}

/// Shared receipt through which arf can observe an adapter save result.
#[derive(Clone)]
pub struct HistorySaveReceipt {
    inner: Arc<Mutex<Option<HistorySaveOutcome>>>,
}

impl HistorySaveReceipt {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    pub fn take(&self) -> Option<HistorySaveOutcome> {
        self.inner
            .lock()
            .ok()
            .and_then(|mut outcome| outcome.take())
    }

    pub(crate) fn record(&self, outcome: HistorySaveOutcome) {
        if let Ok(mut latest) = self.inner.lock() {
            *latest = Some(outcome);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taking_a_recorded_outcome_consumes_it() {
        let receipt = HistorySaveReceipt::new();
        let outcome = HistorySaveOutcome::Saved(reedline::HistoryItemId::new(42));
        receipt.record(outcome);

        assert_eq!(receipt.take(), Some(outcome));
        assert_eq!(receipt.take(), None);
    }
}
