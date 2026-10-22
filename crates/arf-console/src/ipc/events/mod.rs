//! Event delivery from the R session to subscribed connections.
//!
//! Producers publish events through [`EventBus::publish`], which is safe to
//! call from synchronous R-thread code: it never waits for queue space or for
//! the network. An [`EventDispatcher`] task, running on the IPC server's
//! runtime, routes each event to the connections whose subscriptions match
//! and places it on that connection's bounded queue. The connection's writer
//! drains its [`EventStream`].
//!
//! ## Back-pressure
//!
//! * Every queue is limited by event count and by bytes ([`Limits`]); a single
//!   event larger than `max_event_bytes` is rejected when published.
//! * A connection whose own queue overflows is disconnected; other
//!   connections are unaffected.
//! * If the shared publish queue overflows, the dropped event is not lost
//!   silently: the dispatcher is told and disconnects every connection whose
//!   subscriptions may match what was dropped.
//!
//! Disconnected connections can observe why through
//! [`EventStream::disconnect_reason`].
//!
//! ## Topics and subscriptions
//!
//! A topic is a dot-separated name. Each segment is non-empty and made of
//! ASCII letters, digits, `_` and `-`; the whole name is at most 128 bytes.
//! Names starting with `arf.` are reserved for arf itself (see
//! [`Topic::external`]). A subscription pattern is an exact topic or
//! `prefix.*`, which matches every topic starting with `prefix.`.
//!
//! ## Ordering
//!
//! `seq` is assigned in publication order and the order is preserved through
//! both queues, so each connection receives its events in increasing `seq`.
//! Subscriptions apply from the moment the dispatcher routes an event, not
//! from when it was published. Connection ids and `seq` are shared across
//! the whole process, so they keep increasing even if the bus is recreated.

// Nothing outside this module and its tests uses the event bus yet; the
// expectation fails (and must be removed) once a caller exists.
#![expect(
    dead_code,
    unused_imports,
    reason = "the bus is not yet wired into the IPC server"
)]

mod bus;
mod dispatcher;
mod event;
mod pattern;
mod queue;
#[cfg(test)]
mod tests;

pub use bus::{
    ConnectionId, DisconnectReason, EventBus, EventStream, Limits, PublishError, SubscribeError,
};
pub use dispatcher::EventDispatcher;
pub use event::{Event, ExecutionId, Topic, TopicError};
pub use pattern::SubscriptionPattern;
pub use queue::QueueLimits;
