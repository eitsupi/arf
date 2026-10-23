//! Events published by R code (`arf_publish_event`) on the current bus.

use serde_json::Value;

use super::bus::PublishError;
use super::current::publish_event;
use super::event::{ExecutionId, Topic};
use arf_harp::embedding_api::{PublishOutcome, set_event_publisher};

/// Let R code publish events on the bus of the running IPC server.
///
/// The publisher looks up the current bus on every call, so it keeps working
/// across server restarts. Call this before the embedding routines are
/// registered with R.
pub(crate) fn install_r_event_publisher() {
    if !set_event_publisher(publish_from_r) {
        log::debug!("An R event publisher is already installed");
    }
}

fn publish_from_r(topic: &str, data: Value) -> PublishOutcome {
    publish_with(topic, data, publish_event)
}

/// Validate `topic` as one chosen by R code, publish with `publish` and map
/// the result for R.
///
/// The `execution` is always `None`: events from R are not tied to an
/// execution yet.
fn publish_with(
    name: &str,
    data: Value,
    publish: impl FnOnce(Topic, Value, Option<ExecutionId>) -> Result<u64, PublishError>,
) -> PublishOutcome {
    let topic = match Topic::external(name) {
        Ok(topic) => topic,
        Err(error) => {
            return PublishOutcome::Rejected(format!("invalid event topic {name:?}: {error}"));
        }
    };
    match publish(topic, data, None) {
        Ok(_) => PublishOutcome::Published,
        Err(PublishError::Closed) => PublishOutcome::NoTarget,
        Err(error @ (PublishError::TooLarge { .. } | PublishError::QueueOverflow)) => {
            PublishOutcome::Rejected(format!("event {name} was not published: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::bus::{EventBus, Limits};
    use super::super::current::CurrentBus;
    use super::super::pattern::SubscriptionPattern;
    use super::super::queue::QueueLimits;
    use super::super::{clear_current_bus, install_current_bus};
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);

    fn pattern(text: &str) -> SubscriptionPattern {
        SubscriptionPattern::parse(text).unwrap()
    }

    fn rejected(outcome: PublishOutcome) -> String {
        match outcome {
            PublishOutcome::Rejected(message) => message,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn topics_from_r_are_validated_and_must_not_be_reserved() {
        let never = |_: Topic, _: Value, _: Option<ExecutionId>| -> Result<u64, PublishError> {
            panic!("an invalid topic must not reach the bus")
        };
        for (topic, expected) in [
            ("", "topic is empty"),
            ("a..b", "empty segment"),
            ("a b", "invalid character"),
            ("arf.execution.finished", "reserved"),
            ("arf.anything", "reserved"),
        ] {
            let message = rejected(publish_with(topic, Value::Null, never));
            assert!(message.contains(expected), "{topic:?}: {message}");
            assert!(
                message.contains(&format!("{topic:?}")),
                "{topic:?}: {message}"
            );
        }
    }

    #[test]
    fn publish_errors_map_to_the_outcome_for_r() {
        let publish_result = |result: Result<u64, PublishError>| {
            publish_with(
                "demo.topic",
                json!({"k": 1}),
                move |topic, data, execution| {
                    assert_eq!(topic.as_str(), "demo.topic");
                    assert_eq!(data, json!({"k": 1}));
                    assert_eq!(execution, None);
                    result
                },
            )
        };

        assert_eq!(publish_result(Ok(7)), PublishOutcome::Published);
        assert_eq!(
            publish_result(Err(PublishError::Closed)),
            PublishOutcome::NoTarget
        );
        let message = rejected(publish_result(Err(PublishError::QueueOverflow)));
        assert!(
            message.contains("demo.topic") && message.contains("overflow"),
            "{message}"
        );
        let message = rejected(publish_result(Err(PublishError::TooLarge {
            size_at_least: 11,
            max: 10,
        })));
        assert!(
            message.contains("demo.topic") && message.contains("maximum size of 10 bytes"),
            "{message}"
        );
    }

    #[test]
    fn events_too_large_for_any_server_are_rejected_even_without_one() {
        let current = CurrentBus::new();
        let too_large = json!("x".repeat(Limits::default().max_event_bytes));
        let outcome = publish_with("demo.big", too_large, |t, d, e| current.publish(t, d, e));
        assert!(rejected(outcome).contains("maximum size"));

        let outcome = publish_with("demo.small", json!("x"), |t, d, e| current.publish(t, d, e));
        assert_eq!(outcome, PublishOutcome::NoTarget);
    }

    #[test]
    fn a_full_publish_queue_is_a_rejection() {
        let limits = Limits {
            publish_queue: QueueLimits {
                max_events: 2,
                max_bytes: 1024 * 1024,
            },
            ..Limits::default()
        };
        // The dispatcher is never run, so the shared queue fills up.
        let (bus, _dispatcher) = EventBus::new(limits);
        let current = CurrentBus::new();
        current.install(bus);
        let publish = || publish_with("demo.fill", Value::Null, |t, d, e| current.publish(t, d, e));
        assert_eq!(publish(), PublishOutcome::Published);
        assert_eq!(publish(), PublishOutcome::Published);
        assert!(rejected(publish()).contains("overflow"));
    }

    /// The only test that touches the process-wide current bus; the unit
    /// tests of this crate never run a server, so nothing else competes for it.
    #[tokio::test]
    async fn the_installed_publisher_follows_the_current_bus() {
        assert_eq!(
            publish_from_r("demo.topic", json!(1)),
            PublishOutcome::NoTarget
        );

        let (first, first_dispatcher) = EventBus::new(Limits::default());
        let first_task = tokio::spawn(first_dispatcher.run());
        let mut first_stream = first.register_connection();
        first
            .subscribe(first_stream.id(), pattern("demo.*"))
            .unwrap();
        let first_generation = install_current_bus(first.clone());

        assert_eq!(
            publish_from_r("demo.topic", json!({"n": 1})),
            PublishOutcome::Published
        );
        let event = tokio::time::timeout(WAIT, first_stream.recv())
            .await
            .expect("timed out waiting for the event")
            .expect("connection was disconnected");
        assert_eq!(event.topic.as_str(), "demo.topic");
        assert_eq!(event.data, json!({"n": 1}));
        assert_eq!(event.execution, None);

        // A bus installed later receives the next event: the bus is looked up
        // on every call, not remembered from the first one.
        let (second, second_dispatcher) = EventBus::new(Limits::default());
        let second_task = tokio::spawn(second_dispatcher.run());
        let mut second_stream = second.register_connection();
        second
            .subscribe(second_stream.id(), pattern("demo.*"))
            .unwrap();
        let second_generation = install_current_bus(second.clone());
        assert_eq!(
            publish_from_r("demo.topic", json!({"n": 2})),
            PublishOutcome::Published
        );
        let event = tokio::time::timeout(WAIT, second_stream.recv())
            .await
            .expect("timed out waiting for the event")
            .expect("connection was disconnected");
        assert_eq!(event.data, json!({"n": 2}));

        assert!(clear_current_bus(second_generation));
        assert_eq!(
            publish_from_r("demo.topic", json!(3)),
            PublishOutcome::NoTarget
        );
        assert!(!clear_current_bus(first_generation));

        first.close();
        second.close();
        first_task.await.unwrap();
        second_task.await.unwrap();
    }
}
