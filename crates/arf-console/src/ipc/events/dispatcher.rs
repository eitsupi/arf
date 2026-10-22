//! The delivery side of the event bus.

use std::sync::Arc;

use super::bus::{DisconnectReason, Inner};
use super::queue::QueueReceiver;

/// Moves events from the shared publish queue to connection queues.
///
/// Created together with the [`EventBus`](super::EventBus). Run it as a task
/// on the IPC server's runtime; it never waits on a connection.
pub struct EventDispatcher {
    inner: Arc<Inner>,
    receiver: QueueReceiver,
}

impl EventDispatcher {
    pub(super) fn new(inner: Arc<Inner>, receiver: QueueReceiver) -> Self {
        Self { inner, receiver }
    }

    /// Deliver events until the bus is closed and its queue is drained.
    pub async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                event = self.receiver.recv() => {
                    let Some(event) = event else { break };
                    // Handle an overflow before delivering anything published
                    // after it, so no connection sees a gap without being cut off.
                    self.handle_overflow();
                    self.inner.deliver(&event);
                }
                () = self.inner.overflow_notified() => self.handle_overflow(),
            }
        }
        self.handle_overflow();
    }

    fn handle_overflow(&self) {
        if let Some(record) = self.inner.take_overflow() {
            self.inner.disconnect_affected(&record);
        }
    }
}

impl Drop for EventDispatcher {
    fn drop(&mut self) {
        self.inner.disconnect_all(DisconnectReason::Shutdown);
    }
}
