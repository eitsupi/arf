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
//!
//! ## The current bus
//!
//! Each run of the IPC server owns one bus. Producers without a handle to it
//! publish through `publish_event`, which reaches the bus of the server that
//! is running, or fails if there is none. A stopping server removes only the
//! bus it installed itself (identified by a generation number), so it can
//! never remove the bus of a server started after it.

mod bus;
mod current;
mod dispatcher;
mod event;
mod pattern;
mod queue;
mod r_publisher;
#[cfg(test)]
mod tests;

pub(crate) use bus::allocate_connection_id;
pub use bus::{ConnectionId, EventBus, EventStream, Limits, SubscribeError};
pub(crate) use current::publish_event;
pub(crate) use current::{clear_current_bus, install_current_bus};
pub use event::Event;
pub use pattern::SubscriptionPattern;
pub(crate) use r_publisher::install_r_event_publisher;

// Used by event producers, which are added separately from the server.
#[cfg_attr(
    not(test),
    expect(
        unused_imports,
        reason = "only used by producers outside the server, which are added separately"
    )
)]
pub use {
    bus::{DisconnectReason, PublishError},
    event::{ExecutionId, Topic, TopicError},
    queue::QueueLimits,
};

#[expect(
    unused_imports,
    reason = "the server names the dispatcher only through the bus constructor"
)]
pub use dispatcher::EventDispatcher;
