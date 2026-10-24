//! The event bus: publishing, connection registry and delivery.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::sync::mpsc::error::TryRecvError;
use tokio_util::sync::CancellationToken;

use super::dispatcher::EventDispatcher;
use super::event::{Event, ExecutionId, Topic, json_len_within};
use super::pattern::{SubscriptionPattern, SubscriptionSet};
use super::queue::{QueueError, QueueLimits, QueueReceiver, QueueSender, queue};
use crate::sync::LockRecover;

/// Distinct dropped topics remembered after the publish queue overflows.
/// Past this many, every subscribed connection is treated as affected.
const MAX_RECORDED_OVERFLOW_TOPICS: usize = 64;

/// Size and count limits for the whole event pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest accepted event (see [`Event::size`]); larger ones are rejected
    /// at publish time.
    pub max_event_bytes: usize,
    /// Queue between publishers and the dispatcher.
    pub publish_queue: QueueLimits,
    /// Outgoing queue of each connection.
    pub connection_queue: QueueLimits,
    /// Patterns one connection may hold at the same time.
    pub max_subscriptions_per_connection: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_event_bytes: 1024 * 1024,
            publish_queue: QueueLimits {
                max_events: 1024,
                max_bytes: 8 * 1024 * 1024,
            },
            connection_queue: QueueLimits {
                max_events: 256,
                max_bytes: 4 * 1024 * 1024,
            },
            max_subscriptions_per_connection: 64,
        }
    }
}

impl Limits {
    fn assert_consistent(&self) {
        assert!(
            self.max_event_bytes <= self.publish_queue.max_bytes
                && self.max_event_bytes <= self.connection_queue.max_bytes,
            "max_event_bytes must fit in every queue"
        );
    }
}

/// Identifier of a registered connection. Never reused within a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ConnectionId(u64);

impl ConnectionId {
    pub(crate) fn value(self) -> u64 {
        self.0
    }
}

pub(crate) fn allocate_connection_id() -> ConnectionId {
    ConnectionId(NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed))
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a connection was removed from the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    /// The connection's outgoing queue overflowed.
    SlowConsumer,
    /// The shared publish queue overflowed and events that may have matched
    /// this connection's subscriptions were dropped.
    PublishQueueOverflow,
    /// The owner of the connection asked for it to be closed.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "part of the bus API that the IPC server does not use"
        )
    )]
    Requested,
    /// The receiving end of the connection was dropped.
    ReceiverDropped,
    /// The bus is shutting down.
    Shutdown,
}

/// Why an event could not be published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishError {
    TooLarge {
        /// Smallest size the event is known to have reached (at least `max + 1`).
        size_at_least: usize,
        max: usize,
    },
    /// The publish queue was full; the event was dropped and subscribed
    /// connections are being disconnected.
    QueueOverflow,
    /// The bus is closed or its dispatcher is gone, or, when publishing
    /// through the current bus, no server is running.
    Closed,
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { max, .. } => {
                write!(f, "event exceeds the maximum size of {max} bytes")
            }
            Self::QueueOverflow => write!(f, "event queue overflow"),
            Self::Closed => write!(f, "event bus is closed"),
        }
    }
}

impl std::error::Error for PublishError {}

/// Encoded size of `data` if an event with this topic and data fits in
/// `max` bytes, or [`PublishError::TooLarge`].
pub(super) fn check_event_size(
    topic: &Topic,
    data: &Value,
    max: usize,
) -> Result<usize, PublishError> {
    let overhead = super::event::event_size(topic.as_str().len(), 0);
    max.checked_sub(overhead)
        .and_then(|room| json_len_within(data, room))
        .ok_or(PublishError::TooLarge {
            size_at_least: max.saturating_add(1),
            max,
        })
}

/// Why a subscription change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeError {
    UnknownConnection,
    TooManySubscriptions { max: usize },
}

impl fmt::Display for SubscribeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownConnection => write!(f, "connection is not registered"),
            Self::TooManySubscriptions { max } => {
                write!(f, "a connection may hold at most {max} subscriptions")
            }
        }
    }
}

impl std::error::Error for SubscribeError {}

/// State shared between the registry and the connection's [`EventStream`].
struct ConnectionShared {
    reason: OnceLock<DisconnectReason>,
    /// Cancelled when `reason` is set, so the owner can wait for a disconnect.
    disconnected: CancellationToken,
}

struct ConnectionEntry {
    subscriptions: SubscriptionSet,
    sender: QueueSender,
    shared: Arc<ConnectionShared>,
}

#[derive(Default)]
struct Registry {
    connections: HashMap<ConnectionId, ConnectionEntry>,
    /// Set once the dispatcher is gone. Registration checks it under the same
    /// lock that `disconnect_all` takes, so a connection is either removed by
    /// that sweep or never registered.
    shut_down: bool,
}

impl Registry {
    /// Remove a connection, dropping its subscriptions and queue.
    fn remove(&mut self, id: ConnectionId, reason: DisconnectReason) -> bool {
        match self.connections.remove(&id) {
            Some(entry) => {
                let _ = entry.shared.reason.set(reason);
                entry.shared.disconnected.cancel();
                log::debug!("event connection {id} disconnected: {reason:?}");
                true
            }
            None => false,
        }
    }
}

/// Topics dropped because the publish queue overflowed, not yet handled by
/// the dispatcher.
#[derive(Default)]
pub(super) struct OverflowRecord {
    topics: HashSet<Topic>,
    /// Too many distinct topics to list; assume every subscription is affected.
    everything: bool,
}

impl OverflowRecord {
    fn note(&mut self, topic: &Topic) {
        if self.everything {
            return;
        }
        if self.topics.len() >= MAX_RECORDED_OVERFLOW_TOPICS && !self.topics.contains(topic) {
            self.everything = true;
            self.topics.clear();
        } else {
            self.topics.insert(topic.clone());
        }
    }

    fn affects(&self, subscriptions: &SubscriptionSet) -> bool {
        if subscriptions.is_empty() {
            return false;
        }
        self.everything || self.topics.iter().any(|t| subscriptions.matches(t))
    }
}

/// Source of event sequence numbers.
///
/// The production source is process-wide, so `seq` keeps increasing when the
/// bus is recreated within one process (for example by stopping and
/// restarting the IPC server).
pub(super) struct SeqSource(Mutex<u64>);

impl SeqSource {
    pub(super) const fn new() -> Self {
        Self(Mutex::new(1))
    }
}

static PROCESS_SEQ: SeqSource = SeqSource::new();

struct PublishState {
    /// `None` once the bus has been closed.
    sender: Option<QueueSender>,
}

pub(super) struct Inner {
    limits: Limits,
    /// Sequence assignment and enqueueing happen under this lock so that the
    /// queue order always equals the `seq` order.
    publish: Mutex<PublishState>,
    seq: &'static SeqSource,
    overflow: Mutex<OverflowRecord>,
    /// Hint that `overflow` holds something; checked without locking.
    overflow_pending: AtomicBool,
    overflow_notify: Notify,
    registry: Mutex<Registry>,
}

/// Connection ids come from one process-wide counter, so ids stay unique
/// even when the bus is recreated (for example by stopping and restarting
/// the IPC server).
static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

impl Inner {
    /// Take the recorded overflow, if any.
    pub(super) fn take_overflow(&self) -> Option<OverflowRecord> {
        if !self.overflow_pending.load(Ordering::Acquire) {
            return None;
        }
        let mut record = self.overflow.lock_recover();
        self.overflow_pending.store(false, Ordering::Release);
        Some(std::mem::take(&mut *record))
    }

    /// Disconnect every connection whose subscriptions may have matched a
    /// dropped event.
    pub(super) fn disconnect_affected(&self, record: &OverflowRecord) {
        let mut registry = self.registry.lock_recover();
        let affected: Vec<ConnectionId> = registry
            .connections
            .iter()
            .filter(|(_, entry)| record.affects(&entry.subscriptions))
            .map(|(id, _)| *id)
            .collect();
        for id in affected {
            registry.remove(id, DisconnectReason::PublishQueueOverflow);
        }
    }

    /// Queue `event` on every connection subscribed to its topic.
    pub(super) fn deliver(&self, event: &Arc<Event>) {
        let mut registry = self.registry.lock_recover();
        let mut failed: Vec<(ConnectionId, DisconnectReason)> = Vec::new();
        for (id, entry) in &registry.connections {
            if !entry.subscriptions.matches(&event.topic) {
                continue;
            }
            match entry.sender.try_send(event) {
                Ok(()) => {}
                Err(QueueError::Full) => failed.push((*id, DisconnectReason::SlowConsumer)),
                Err(QueueError::Closed) => failed.push((*id, DisconnectReason::ReceiverDropped)),
            }
        }
        for (id, reason) in failed {
            registry.remove(id, reason);
        }
    }

    /// Stop accepting events; later publishes fail with [`PublishError::Closed`].
    pub(super) fn close_publishing(&self) {
        self.publish.lock_recover().sender = None;
    }

    pub(super) fn disconnect_all(&self, reason: DisconnectReason) {
        let mut registry = self.registry.lock_recover();
        registry.shut_down = true;
        let ids: Vec<ConnectionId> = registry.connections.keys().copied().collect();
        for id in ids {
            registry.remove(id, reason);
        }
    }

    pub(super) fn overflow_notified(&self) -> impl Future<Output = ()> + '_ {
        self.overflow_notify.notified()
    }
}

/// Handle for publishing events and managing connections.
///
/// Cheap to clone; all clones share one bus.
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Inner>,
}

impl EventBus {
    /// Create a bus and the dispatcher that must be run (see
    /// [`EventDispatcher::run`]) for events to reach connections.
    pub fn new(limits: Limits) -> (Self, EventDispatcher) {
        Self::with_seq_source(limits, &PROCESS_SEQ)
    }

    pub(super) fn with_seq_source(
        limits: Limits,
        seq: &'static SeqSource,
    ) -> (Self, EventDispatcher) {
        limits.assert_consistent();
        let (sender, receiver) = queue(limits.publish_queue);
        let inner = Arc::new(Inner {
            limits,
            publish: Mutex::new(PublishState {
                sender: Some(sender),
            }),
            seq,
            overflow: Mutex::new(OverflowRecord::default()),
            overflow_pending: AtomicBool::new(false),
            overflow_notify: Notify::new(),
            registry: Mutex::new(Registry::default()),
        });
        let dispatcher = EventDispatcher::new(Arc::clone(&inner), receiver);
        (Self { inner }, dispatcher)
    }

    /// Publish an event. Never blocks and never waits for delivery.
    ///
    /// Events are delivered in the order of successful `publish` calls.
    /// Returns the assigned `seq`. Sequence numbers are shared by all buses
    /// of the process, so they keep increasing across bus instances; a failed
    /// publish does not consume one. Events nobody is subscribed to are
    /// discarded by the dispatcher.
    ///
    /// If the publish queue is full the event is dropped, the dispatcher is
    /// told, and every connection whose subscriptions may match is
    /// disconnected with [`DisconnectReason::PublishQueueOverflow`].
    pub fn publish(
        &self,
        topic: Topic,
        data: Value,
        execution: Option<ExecutionId>,
    ) -> Result<u64, PublishError> {
        let data_len = check_event_size(&topic, &data, self.inner.limits.max_event_bytes)?;

        let state = self.inner.publish.lock_recover();
        // Lock order is always bus publish lock, then the sequence source.
        // The sequence number is only consumed once the event is queued.
        let mut next_seq = self.inner.seq.0.lock_recover();
        let seq = *next_seq;
        let event = Arc::new(Event::new(seq, topic, data, execution, data_len));
        let Some(sender) = state.sender.as_ref() else {
            return Err(PublishError::Closed);
        };
        match sender.try_send(&event) {
            Ok(()) => {
                *next_seq += 1;
                Ok(seq)
            }
            Err(QueueError::Full) => {
                // Still holding the publish lock: the overflow is recorded
                // before any later event can enter the queue, so the
                // dispatcher always sees it before such an event.
                self.inner.overflow.lock_recover().note(&event.topic);
                self.inner.overflow_pending.store(true, Ordering::Release);
                self.inner.overflow_notify.notify_one();
                log::warn!(
                    "event publish queue overflow, dropped topic {}",
                    event.topic
                );
                Err(PublishError::QueueOverflow)
            }
            Err(QueueError::Closed) => Err(PublishError::Closed),
        }
    }

    /// Stop accepting events. The dispatcher delivers what is already queued,
    /// disconnects all connections with [`DisconnectReason::Shutdown`] and exits.
    pub fn close(&self) {
        self.inner.close_publishing();
    }

    /// Register a new connection with no subscriptions.
    ///
    /// If the dispatcher has already shut down, the returned stream is not
    /// registered and is already disconnected with
    /// [`DisconnectReason::Shutdown`], so `recv` returns `None` right away.
    pub fn register_connection(&self) -> EventStream {
        let id = allocate_connection_id();
        let (sender, receiver) = queue(self.inner.limits.connection_queue);
        let shared = Arc::new(ConnectionShared {
            reason: OnceLock::new(),
            disconnected: CancellationToken::new(),
        });
        {
            let mut registry = self.inner.registry.lock_recover();
            if registry.shut_down {
                let _ = shared.reason.set(DisconnectReason::Shutdown);
                shared.disconnected.cancel();
            } else {
                registry.connections.insert(
                    id,
                    ConnectionEntry {
                        subscriptions: SubscriptionSet::default(),
                        sender,
                        shared: Arc::clone(&shared),
                    },
                );
            }
        }
        EventStream {
            id,
            receiver,
            shared,
            bus: self.clone(),
        }
    }

    /// Add a subscription. Returns `false` if the pattern was already present.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "part of the bus API that the IPC server does not use"
        )
    )]
    pub fn subscribe(
        &self,
        id: ConnectionId,
        pattern: SubscriptionPattern,
    ) -> Result<bool, SubscribeError> {
        let max = self.inner.limits.max_subscriptions_per_connection;
        let mut registry = self.inner.registry.lock_recover();
        let entry = registry
            .connections
            .get_mut(&id)
            .ok_or(SubscribeError::UnknownConnection)?;
        if entry.subscriptions.len() >= max && !entry.subscriptions.contains(&pattern) {
            return Err(SubscribeError::TooManySubscriptions { max });
        }
        Ok(entry.subscriptions.insert(pattern))
    }

    /// Add several subscriptions at once, all or nothing: if the connection
    /// would exceed its limit, none is added. Returns how many patterns were
    /// newly added; duplicates, in the request or already held, count once.
    pub fn subscribe_many(
        &self,
        id: ConnectionId,
        patterns: Vec<SubscriptionPattern>,
    ) -> Result<usize, SubscribeError> {
        let max = self.inner.limits.max_subscriptions_per_connection;
        let mut registry = self.inner.registry.lock_recover();
        let entry = registry
            .connections
            .get_mut(&id)
            .ok_or(SubscribeError::UnknownConnection)?;
        let fresh: HashSet<SubscriptionPattern> = patterns
            .into_iter()
            .filter(|pattern| !entry.subscriptions.contains(pattern))
            .collect();
        if entry.subscriptions.len().saturating_add(fresh.len()) > max {
            return Err(SubscribeError::TooManySubscriptions { max });
        }
        let added = fresh.len();
        for pattern in fresh {
            entry.subscriptions.insert(pattern);
        }
        Ok(added)
    }

    /// Remove a subscription. Returns whether it was present.
    pub fn unsubscribe(&self, id: ConnectionId, pattern: &SubscriptionPattern) -> bool {
        self.inner
            .registry
            .lock_recover()
            .connections
            .get_mut(&id)
            .is_some_and(|entry| entry.subscriptions.remove(pattern))
    }

    /// Remove a connection, discarding its subscriptions and queued events.
    /// Returns `false` if it was not registered (for example already
    /// disconnected by the bus).
    pub fn disconnect(&self, id: ConnectionId, reason: DisconnectReason) -> bool {
        self.inner.registry.lock_recover().remove(id, reason)
    }

    #[cfg(test)]
    pub(in crate::ipc) fn connection_count(&self) -> usize {
        self.inner.registry.lock_recover().connections.len()
    }

    #[cfg(test)]
    pub(in crate::ipc) fn subscription_count(&self, id: ConnectionId) -> Option<usize> {
        self.inner
            .registry
            .lock_recover()
            .connections
            .get(&id)
            .map(|entry| entry.subscriptions.len())
    }
}

/// The receiving end of one connection: yields the events to send, in order.
///
/// Dropping it disconnects the connection from the bus.
pub struct EventStream {
    id: ConnectionId,
    receiver: QueueReceiver,
    shared: Arc<ConnectionShared>,
    bus: EventBus,
}

impl EventStream {
    pub fn id(&self) -> ConnectionId {
        self.id
    }

    /// Wait for the next event to send. Returns `None` once the connection
    /// has been disconnected; queued events are then discarded (at most one
    /// that was already in flight may still be returned first).
    pub async fn recv(&mut self) -> Option<Arc<Event>> {
        if self.shared.reason.get().is_some() {
            return None;
        }
        self.receiver.recv().await
    }

    /// Non-waiting variant of [`recv`](Self::recv). `Ok(None)` means the
    /// connection is disconnected, `Err(())` that nothing is queued right now.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "part of the bus API that the IPC server does not use"
        )
    )]
    pub fn try_recv(&mut self) -> Result<Option<Arc<Event>>, ()> {
        if self.shared.reason.get().is_some() {
            return Ok(None);
        }
        match self.receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Err(()),
            Err(TryRecvError::Disconnected) => Ok(None),
        }
    }

    /// A token that is cancelled as soon as the bus disconnects this
    /// connection, for waiting on a disconnect while the stream itself is
    /// busy elsewhere.
    pub fn disconnected_token(&self) -> CancellationToken {
        self.shared.disconnected.clone()
    }

    /// Why the connection was disconnected, once it has been.
    pub fn disconnect_reason(&self) -> Option<DisconnectReason> {
        self.shared.reason.get().copied()
    }

    /// Disconnect this connection from the bus.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "part of the bus API that the IPC server does not use"
        )
    )]
    pub fn disconnect(&self) -> bool {
        self.bus.disconnect(self.id, DisconnectReason::Requested)
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.bus
            .disconnect(self.id, DisconnectReason::ReceiverDropped);
    }
}
