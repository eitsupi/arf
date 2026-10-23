//! The event bus of the running IPC server, for producers that have no handle
//! to it (such as code running on the R thread).

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::bus::{EventBus, Limits, PublishError, check_event_size};
use super::event::{ExecutionId, Topic};

/// Identifies one installation of a bus in a [`CurrentBus`] slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation(u64);

struct Installed {
    generation: Generation,
    bus: EventBus,
}

/// A slot holding the bus of the server that is running now, if any.
///
/// Each server run installs its own bus and later clears the slot with the
/// [`Generation`] it received, so a server that is slow to stop can never
/// remove the bus of a server started after it.
pub(super) struct CurrentBus {
    slot: RwLock<Option<Installed>>,
    next_generation: AtomicU64,
}

impl CurrentBus {
    pub(super) const fn new() -> Self {
        Self {
            slot: RwLock::new(None),
            next_generation: AtomicU64::new(1),
        }
    }

    /// Make `bus` the current bus, replacing any other.
    pub(super) fn install(&self, bus: EventBus) -> Generation {
        let generation = Generation(self.next_generation.fetch_add(1, Ordering::Relaxed));
        *self
            .slot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Installed { generation, bus });
        generation
    }

    /// Empty the slot, unless a newer bus has replaced the one installed
    /// as `generation`. Returns whether the slot was emptied.
    pub(super) fn clear(&self, generation: Generation) -> bool {
        let mut slot = self
            .slot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot
            .as_ref()
            .is_some_and(|installed| installed.generation == generation)
        {
            *slot = None;
            true
        } else {
            false
        }
    }

    pub(super) fn publish(
        &self,
        topic: Topic,
        data: Value,
        execution: Option<ExecutionId>,
    ) -> Result<u64, PublishError> {
        let slot = self
            .slot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match slot.as_ref() {
            Some(installed) => installed.bus.publish(topic, data, execution),
            None => {
                // The limit belongs to the bus of a running server, which
                // every server creates with the default limits. Refusing an
                // event it would refuse keeps the outcome for the producer
                // independent of whether a server happens to run.
                check_event_size(&topic, &data, Limits::default().max_event_bytes)?;
                Err(PublishError::Closed)
            }
        }
    }
}

static CURRENT: CurrentBus = CurrentBus::new();

/// Make `bus` the process-wide current bus. Called by a starting server.
pub(crate) fn install_current_bus(bus: EventBus) -> Generation {
    CURRENT.install(bus)
}

/// Remove the current bus if it is still the one installed as `generation`.
/// Called by a stopping server.
pub(crate) fn clear_current_bus(generation: Generation) -> bool {
    CURRENT.clear(generation)
}

/// Publish an event on the bus of the running IPC server, without blocking.
///
/// Fails with [`PublishError::Closed`] when no server is running, unless the
/// event is too large for any server to accept; see [`EventBus::publish`] for
/// the other outcomes.
pub(crate) fn publish_event(
    topic: Topic,
    data: Value,
    execution: Option<ExecutionId>,
) -> Result<u64, PublishError> {
    CURRENT.publish(topic, data, execution)
}
