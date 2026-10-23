//! Bounded event queue limited by both item count and total bytes.
//!
//! Sending never waits: a full queue is reported to the caller immediately.
//! Used for the shared publisher-to-dispatcher queue and for each
//! connection's outgoing queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::{TryRecvError, TrySendError};

use super::event::Event;

/// Size limits of one queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueLimits {
    pub max_events: usize,
    pub max_bytes: usize,
}

/// Why an event could not be queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueueError {
    /// The count or byte limit would be exceeded.
    Full,
    /// The receiving side has gone away.
    Closed,
}

pub(super) struct QueueSender {
    tx: mpsc::Sender<Arc<Event>>,
    queued_bytes: Arc<AtomicUsize>,
    max_bytes: usize,
}

pub(super) struct QueueReceiver {
    rx: mpsc::Receiver<Arc<Event>>,
    queued_bytes: Arc<AtomicUsize>,
}

pub(super) fn queue(limits: QueueLimits) -> (QueueSender, QueueReceiver) {
    let (tx, rx) = mpsc::channel(limits.max_events.max(1));
    let queued_bytes = Arc::new(AtomicUsize::new(0));
    (
        QueueSender {
            tx,
            queued_bytes: Arc::clone(&queued_bytes),
            max_bytes: limits.max_bytes,
        },
        QueueReceiver { rx, queued_bytes },
    )
}

impl QueueSender {
    /// Queue an event without waiting.
    ///
    /// Byte accounting assumes a single sending context at a time, which both
    /// users of this type guarantee (the publisher lock and the dispatcher).
    pub(super) fn try_send(&self, event: &Arc<Event>) -> Result<(), QueueError> {
        let size = event.size();
        let used = self.queued_bytes.load(Ordering::Acquire);
        if used.saturating_add(size) > self.max_bytes {
            return Err(QueueError::Full);
        }
        self.queued_bytes.fetch_add(size, Ordering::AcqRel);
        match self.tx.try_send(Arc::clone(event)) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.queued_bytes.fetch_sub(size, Ordering::AcqRel);
                Err(match err {
                    TrySendError::Full(_) => QueueError::Full,
                    TrySendError::Closed(_) => QueueError::Closed,
                })
            }
        }
    }
}

impl QueueReceiver {
    /// Wait for the next event; `None` once the queue is closed and drained.
    pub(super) async fn recv(&mut self) -> Option<Arc<Event>> {
        let event = self.rx.recv().await?;
        self.release(&event);
        Some(event)
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "part of the bus API that the IPC server does not use"
        )
    )]
    pub(super) fn try_recv(&mut self) -> Result<Arc<Event>, TryRecvError> {
        let event = self.rx.try_recv()?;
        self.release(&event);
        Ok(event)
    }

    fn release(&self, event: &Event) {
        self.queued_bytes.fetch_sub(event.size(), Ordering::AcqRel);
    }
}
