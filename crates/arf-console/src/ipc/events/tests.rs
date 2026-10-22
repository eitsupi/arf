use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::task::JoinHandle;

use super::bus::SeqSource;
use super::*;

const WAIT: Duration = Duration::from_secs(5);

fn internal(name: &str) -> Topic {
    Topic::internal(name).unwrap()
}

fn pattern(text: &str) -> SubscriptionPattern {
    SubscriptionPattern::parse(text).unwrap()
}

fn spawn_bus(limits: Limits) -> (EventBus, JoinHandle<()>) {
    let (bus, dispatcher) = EventBus::new(limits);
    (bus, tokio::spawn(dispatcher.run()))
}

fn subscribed(bus: &EventBus, patterns: &[&str]) -> EventStream {
    let stream = bus.register_connection();
    for text in patterns {
        bus.subscribe(stream.id(), pattern(text)).unwrap();
    }
    stream
}

fn publish(bus: &EventBus, topic: &str, data: Value) -> u64 {
    bus.publish(internal(topic), data, None).unwrap()
}

async fn next(stream: &mut EventStream) -> Arc<Event> {
    tokio::time::timeout(WAIT, stream.recv())
        .await
        .expect("timed out waiting for an event")
        .expect("connection was disconnected")
}

async fn assert_disconnected(stream: &mut EventStream, reason: DisconnectReason) {
    let received = tokio::time::timeout(WAIT, stream.recv())
        .await
        .expect("timed out waiting for disconnect");
    assert!(received.is_none());
    assert_eq!(stream.disconnect_reason(), Some(reason));
}

/// Limits with room for large events but small connection queues.
fn limits_with_connection_events(max_events: usize) -> Limits {
    Limits {
        connection_queue: QueueLimits {
            max_events,
            max_bytes: 1024 * 1024,
        },
        ..Limits::default()
    }
}

#[test]
fn topic_validation() {
    for good in [
        "a",
        "foo.bar",
        "arf.execution.finished",
        "my_pkg.sub-topic.1",
    ] {
        assert!(Topic::internal(good).is_ok(), "{good}");
    }
    assert_eq!(Topic::internal(""), Err(TopicError::Empty));
    assert_eq!(Topic::internal("a..b"), Err(TopicError::EmptySegment));
    assert_eq!(Topic::internal(".a"), Err(TopicError::EmptySegment));
    assert_eq!(Topic::internal("a."), Err(TopicError::EmptySegment));
    assert_eq!(
        Topic::internal("a b"),
        Err(TopicError::InvalidCharacter(' '))
    );
    assert_eq!(
        Topic::internal("a.*"),
        Err(TopicError::InvalidCharacter('*'))
    );
    assert!(matches!(
        Topic::internal(&"x".repeat(129)),
        Err(TopicError::TooLong { len: 129, max: 128 })
    ));
}

#[test]
fn reserved_namespace_is_distinguished_by_constructor() {
    assert_eq!(Topic::external("arf.secret"), Err(TopicError::Reserved));
    assert_eq!(Topic::external("arf.a.b"), Err(TopicError::Reserved));
    assert!(Topic::external("arfx.thing").is_ok());
    assert!(Topic::external("user.arf.thing").is_ok());

    let reserved = Topic::internal("arf.execution.finished").unwrap();
    assert!(reserved.is_reserved());
    assert!(!Topic::external("user.event").unwrap().is_reserved());
}

#[test]
fn pattern_parsing() {
    assert_eq!(
        SubscriptionPattern::parse("foo.bar"),
        Ok(SubscriptionPattern::Exact("foo.bar".into()))
    );
    assert_eq!(
        SubscriptionPattern::parse("foo.*"),
        Ok(SubscriptionPattern::Prefix("foo.".into()))
    );
    assert_eq!(pattern("foo.*").to_string(), "foo.*");
    for bad in ["", "*", ".*", "foo.*.bar", "foo*", "foo..*", "a b"] {
        assert!(SubscriptionPattern::parse(bad).is_err(), "{bad}");
    }
    // Subscribers may name reserved topics.
    assert!(SubscriptionPattern::parse("arf.*").is_ok());
}

#[test]
fn pattern_matching() {
    let exact = pattern("foo.bar");
    assert!(exact.matches(&internal("foo.bar")));
    assert!(!exact.matches(&internal("foo.bar.baz")));
    assert!(!exact.matches(&internal("foo")));

    let prefix = pattern("foo.*");
    assert!(prefix.matches(&internal("foo.bar")));
    assert!(prefix.matches(&internal("foo.bar.baz")));
    assert!(!prefix.matches(&internal("foo")));
    assert!(!prefix.matches(&internal("foobar")));
    assert!(!prefix.matches(&internal("bar.foo.x")));
}

#[test]
fn event_serializes_with_all_envelope_fields() {
    let (bus, _dispatcher) = EventBus::new(Limits::default());
    let stream = subscribed(&bus, &["x.y"]);
    drop(stream);
    let event = Event::new(7, internal("x.y"), json!({"k": [1, 2]}), None, 0);
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({"seq": 7, "topic": "x.y", "data": {"k": [1, 2]}, "execution": null})
    );
    let event = Event::new(8, internal("x.y"), Value::Null, Some(ExecutionId(42)), 0);
    assert_eq!(
        serde_json::to_value(&event).unwrap()["execution"],
        json!(42)
    );
}

#[tokio::test]
async fn subscription_matching_decides_delivery() {
    let (bus, _task) = spawn_bus(Limits::default());
    let mut exact = subscribed(&bus, &["job.done"]);
    let mut prefix = subscribed(&bus, &["job.*"]);
    let mut other = subscribed(&bus, &["unrelated", "other.*"]);

    publish(&bus, "job.done", json!(1));
    publish(&bus, "job.step.2", json!(2));
    publish(&bus, "job", json!(3));
    publish(&bus, "unrelated", json!(4));

    assert_eq!(next(&mut exact).await.data, json!(1));
    assert_eq!(next(&mut prefix).await.data, json!(1));
    assert_eq!(next(&mut prefix).await.data, json!(2));
    // `exact` saw nothing else and `other` only its own topic.
    assert_eq!(next(&mut other).await.data, json!(4));
    publish(&bus, "job.done", json!(5));
    assert_eq!(next(&mut exact).await.data, json!(5));
}

#[tokio::test]
async fn overlapping_patterns_deliver_once() {
    let (bus, _task) = spawn_bus(Limits::default());
    let mut stream = subscribed(&bus, &["a.b", "a.*", "a.b"]);
    publish(&bus, "a.b", json!(1));
    publish(&bus, "a.sentinel", json!(2));
    assert_eq!(next(&mut stream).await.data, json!(1));
    assert_eq!(next(&mut stream).await.data, json!(2));
}

#[tokio::test]
async fn events_arrive_in_publication_order_with_increasing_seq() {
    let (bus, _task) = spawn_bus(Limits::default());
    let mut stream = subscribed(&bus, &["a.*", "b.*"]);

    let mut assigned = Vec::new();
    for i in 0..50 {
        let topic = if i % 2 == 0 { "a.x" } else { "b.y" };
        assigned.push(publish(&bus, topic, json!(i)));
    }
    assert!(assigned.windows(2).all(|w| w[0] < w[1]));

    for (i, seq) in assigned.iter().enumerate() {
        let event = next(&mut stream).await;
        assert_eq!(event.data, json!(i));
        assert_eq!(event.seq, *seq);
    }
}

#[tokio::test]
async fn every_subscribed_connection_gets_each_event_in_order() {
    let (bus, _task) = spawn_bus(Limits::default());
    let mut streams: Vec<_> = (0..3).map(|_| subscribed(&bus, &["t.*"])).collect();

    for i in 0..20 {
        publish(&bus, "t.n", json!(i));
    }
    for stream in &mut streams {
        let mut last_seq = 0;
        for i in 0..20 {
            let event = next(stream).await;
            assert_eq!(event.data, json!(i));
            assert!(event.seq > last_seq);
            last_seq = event.seq;
        }
    }
}

#[tokio::test]
async fn events_without_subscribers_are_discarded() {
    let (bus, _task) = spawn_bus(Limits::default());
    let mut watcher = subscribed(&bus, &["marker"]);
    let first = publish(&bus, "nobody.listens", json!("lost"));
    publish(&bus, "marker", json!("routed"));
    // Once the watcher has the marker, the earlier event has been routed too.
    next(&mut watcher).await;

    let mut late = subscribed(&bus, &["nobody.*"]);
    let second = publish(&bus, "nobody.listens", json!("kept"));

    let event = next(&mut late).await;
    assert_eq!(event.data, json!("kept"));
    assert_eq!(event.seq, second);
    assert!(second > first);
}

#[tokio::test]
async fn unsubscribe_stops_delivery() {
    let (bus, _task) = spawn_bus(Limits::default());
    let mut stream = subscribed(&bus, &["a", "b"]);
    assert!(bus.unsubscribe(stream.id(), &pattern("a")));
    assert!(!bus.unsubscribe(stream.id(), &pattern("a")));

    publish(&bus, "a", json!(1));
    publish(&bus, "b", json!(2));
    assert_eq!(next(&mut stream).await.data, json!(2));
}

#[tokio::test]
async fn slow_connection_is_disconnected_without_affecting_others() {
    let (bus, _task) = spawn_bus(limits_with_connection_events(4));
    let mut slow = subscribed(&bus, &["load.*"]);
    let mut fast = subscribed(&bus, &["load.*"]);

    for i in 0..10 {
        publish(&bus, "load.tick", json!(i));
        // `fast` keeps up; `slow` never reads.
        assert_eq!(next(&mut fast).await.data, json!(i));
    }

    assert_disconnected(&mut slow, DisconnectReason::SlowConsumer).await;
    assert_eq!(fast.disconnect_reason(), None);
    assert_eq!(bus.connection_count(), 1);
}

#[tokio::test]
async fn connection_byte_limit_disconnects_slow_connection() {
    let limits = Limits {
        max_event_bytes: 1024,
        connection_queue: QueueLimits {
            max_events: 100,
            max_bytes: 1024,
        },
        ..Limits::default()
    };
    let (bus, _task) = spawn_bus(limits);
    let mut slow = subscribed(&bus, &["big"]);
    let mut fast = subscribed(&bus, &["big"]);

    // Each event accounts for roughly 500 bytes, so three exceed 1024.
    let payload = "x".repeat(400);
    for _ in 0..3 {
        publish(&bus, "big", json!(payload));
        next(&mut fast).await;
    }
    assert_disconnected(&mut slow, DisconnectReason::SlowConsumer).await;
    assert_eq!(fast.disconnect_reason(), None);
}

#[tokio::test]
async fn publish_queue_overflow_disconnects_affected_connections() {
    let limits = Limits {
        publish_queue: QueueLimits {
            max_events: 4,
            max_bytes: 1024 * 1024,
        },
        ..Limits::default()
    };
    let (bus, dispatcher) = EventBus::new(limits);
    let mut affected = subscribed(&bus, &["a.*"]);
    let mut also_affected = subscribed(&bus, &["a.over"]);
    let mut unaffected = subscribed(&bus, &["b.only"]);
    let mut idle = bus.register_connection();

    // The dispatcher is not running, so the shared queue fills up.
    for _ in 0..4 {
        bus.publish(internal("a.fill"), Value::Null, None).unwrap();
    }
    assert_eq!(
        bus.publish(internal("a.over"), Value::Null, None),
        Err(PublishError::QueueOverflow)
    );

    let _task = tokio::spawn(dispatcher.run());
    assert_disconnected(&mut affected, DisconnectReason::PublishQueueOverflow).await;
    assert_disconnected(&mut also_affected, DisconnectReason::PublishQueueOverflow).await;

    publish(&bus, "b.only", json!("fine"));
    assert_eq!(next(&mut unaffected).await.data, json!("fine"));
    assert_eq!(unaffected.disconnect_reason(), None);
    // No subscriptions means nothing could have been missed.
    assert_eq!(idle.disconnect_reason(), None);
    assert!(idle.try_recv().is_err());
}

#[tokio::test]
async fn publish_queue_overflow_with_many_topics_disconnects_all_subscribers() {
    let limits = Limits {
        publish_queue: QueueLimits {
            max_events: 1,
            max_bytes: 1024 * 1024,
        },
        ..Limits::default()
    };
    let (bus, dispatcher) = EventBus::new(limits);
    let mut subscriber = subscribed(&bus, &["zzz.never.published"]);

    bus.publish(internal("fill"), Value::Null, None).unwrap();
    for i in 0..100 {
        assert_eq!(
            bus.publish(internal(&format!("dropped.t{i}")), Value::Null, None),
            Err(PublishError::QueueOverflow)
        );
    }

    let _task = tokio::spawn(dispatcher.run());
    assert_disconnected(&mut subscriber, DisconnectReason::PublishQueueOverflow).await;
}

#[tokio::test]
async fn publish_queue_byte_limit_counts_as_overflow() {
    let limits = Limits {
        max_event_bytes: 1024,
        publish_queue: QueueLimits {
            max_events: 100,
            max_bytes: 1024,
        },
        ..Limits::default()
    };
    let (bus, dispatcher) = EventBus::new(limits);
    let mut subscriber = subscribed(&bus, &["big"]);
    let payload = "x".repeat(400);

    bus.publish(internal("big"), json!(payload), None).unwrap();
    bus.publish(internal("big"), json!(payload), None).unwrap();
    assert_eq!(
        bus.publish(internal("big"), json!(payload), None),
        Err(PublishError::QueueOverflow)
    );

    let _task = tokio::spawn(dispatcher.run());
    assert_disconnected(&mut subscriber, DisconnectReason::PublishQueueOverflow).await;
}

#[tokio::test]
async fn event_size_limit_is_enforced_at_publish() {
    let limits = Limits {
        max_event_bytes: 1024,
        ..Limits::default()
    };
    // A private sequence source makes the exact numbers deterministic.
    let (bus, dispatcher) =
        EventBus::with_seq_source(limits, Box::leak(Box::new(SeqSource::new())));
    let _task = tokio::spawn(dispatcher.run());
    let mut stream = subscribed(&bus, &["t"]);

    // 96 bytes of envelope allowance + 1 byte topic + (n + 2) bytes of JSON
    // string leaves room for exactly 925 characters.
    assert_eq!(
        bus.publish(internal("t"), json!("x".repeat(925)), None),
        Ok(1)
    );
    assert!(matches!(
        bus.publish(internal("t"), json!("x".repeat(926)), None),
        Err(PublishError::TooLarge { max: 1024, .. })
    ));
    // A huge value is rejected too, and the failed attempts used no seq.
    assert!(matches!(
        bus.publish(internal("t"), json!(vec![0u8; 100_000]), None),
        Err(PublishError::TooLarge { .. })
    ));
    assert_eq!(bus.publish(internal("t"), json!("ok"), None), Ok(2));

    assert_eq!(next(&mut stream).await.seq, 1);
    assert_eq!(next(&mut stream).await.seq, 2);
}

#[test]
fn publishing_never_blocks_when_nobody_reads() {
    // The dispatcher runs on its own thread but the subscriber never reads,
    // and the publisher far exceeds every queue limit.
    let limits = Limits {
        publish_queue: QueueLimits {
            max_events: 8,
            max_bytes: 1024 * 1024,
        },
        connection_queue: QueueLimits {
            max_events: 8,
            max_bytes: 1024 * 1024,
        },
        ..Limits::default()
    };
    let (bus, dispatcher) = EventBus::new(limits);
    let stream = subscribed(&bus, &["flood.*"]);

    let runtime = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(dispatcher.run());
    });

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let publisher_bus = bus.clone();
    std::thread::spawn(move || {
        let mut ok = 0usize;
        let mut overflowed = 0usize;
        for i in 0..50_000 {
            match publisher_bus.publish(internal("flood.tick"), json!(i), None) {
                Ok(_) => ok += 1,
                Err(PublishError::QueueOverflow) => overflowed += 1,
                Err(other) => panic!("unexpected error: {other}"),
            }
        }
        done_tx.send((ok, overflowed)).unwrap();
    });

    let (ok, overflowed) = done_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("publisher blocked");
    assert_eq!(ok + overflowed, 50_000);
    assert!(ok > 0);

    bus.close();
    runtime.join().unwrap();
    drop(stream);
}

#[tokio::test]
async fn concurrent_publishers_preserve_seq_order_on_delivery() {
    const THREADS: usize = 4;
    const PER_THREAD: usize = 500;
    let limits = Limits {
        publish_queue: QueueLimits {
            max_events: THREADS * PER_THREAD,
            max_bytes: 64 * 1024 * 1024,
        },
        connection_queue: QueueLimits {
            max_events: THREADS * PER_THREAD,
            max_bytes: 64 * 1024 * 1024,
        },
        ..Limits::default()
    };
    let (bus, _task) = spawn_bus(limits);
    let mut stream = subscribed(&bus, &["c.*"]);

    let handles: Vec<_> = (0..THREADS)
        .map(|thread| {
            let bus = bus.clone();
            std::thread::spawn(move || {
                for i in 0..PER_THREAD {
                    bus.publish(internal("c.n"), json!([thread, i]), None)
                        .unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let mut last_seq = 0;
    let mut per_thread = [0usize; THREADS];
    for _ in 0..THREADS * PER_THREAD {
        let event = next(&mut stream).await;
        assert!(event.seq > last_seq);
        last_seq = event.seq;
        let thread = event.data[0].as_u64().unwrap() as usize;
        let index = event.data[1].as_u64().unwrap() as usize;
        assert_eq!(index, per_thread[thread]);
        per_thread[thread] += 1;
    }
}

#[tokio::test]
async fn disconnect_discards_subscriptions_and_queue() {
    let (bus, dispatcher) = EventBus::new(Limits::default());
    let mut stream = subscribed(&bus, &["a", "b.*"]);
    let id = stream.id();
    assert_eq!(bus.subscription_count(id), Some(2));

    // Queue events for the connection, then disconnect before reading them.
    // `witness` receives both events from the same routing step, so once it
    // has them they are in `stream`'s queue too.
    let task = tokio::spawn(dispatcher.run());
    let mut witness = subscribed(&bus, &["a"]);
    publish(&bus, "a", json!(1));
    publish(&bus, "a", json!(2));
    next(&mut witness).await;
    next(&mut witness).await;

    assert!(bus.disconnect(id, DisconnectReason::Requested));
    assert!(!bus.disconnect(id, DisconnectReason::Requested));
    assert_eq!(bus.connection_count(), 1); // only the witness remains
    assert_eq!(bus.subscription_count(id), None);
    assert_disconnected(&mut stream, DisconnectReason::Requested).await;
    assert_eq!(stream.try_recv(), Ok(None));

    assert_eq!(
        bus.subscribe(id, pattern("c")),
        Err(SubscribeError::UnknownConnection)
    );
    assert!(!bus.unsubscribe(id, &pattern("a")));
    task.abort();
}

#[tokio::test]
async fn stream_can_disconnect_itself_and_drop_cleans_up() {
    let (bus, _task) = spawn_bus(Limits::default());
    let mut first = subscribed(&bus, &["a"]);
    assert!(first.disconnect());
    assert_disconnected(&mut first, DisconnectReason::Requested).await;

    let second = subscribed(&bus, &["a"]);
    assert_eq!(bus.connection_count(), 1);
    drop(second);
    assert_eq!(bus.connection_count(), 0);
}

#[tokio::test]
async fn connection_ids_are_never_reused() {
    let (bus, _task) = spawn_bus(Limits::default());
    let first = bus.register_connection();
    let first_id = first.id();
    drop(first);
    let second = bus.register_connection();
    let third = bus.register_connection();
    assert!(second.id() > first_id);
    assert!(third.id() > second.id());
}

#[tokio::test]
async fn subscription_count_is_limited_per_connection() {
    let limits = Limits {
        max_subscriptions_per_connection: 2,
        ..Limits::default()
    };
    let (bus, _task) = spawn_bus(limits);
    let stream = bus.register_connection();
    assert_eq!(bus.subscribe(stream.id(), pattern("a")), Ok(true));
    assert_eq!(bus.subscribe(stream.id(), pattern("b.*")), Ok(true));
    assert_eq!(bus.subscribe(stream.id(), pattern("a")), Ok(false));
    assert_eq!(
        bus.subscribe(stream.id(), pattern("c")),
        Err(SubscribeError::TooManySubscriptions { max: 2 })
    );
    assert!(bus.unsubscribe(stream.id(), &pattern("a")));
    assert_eq!(bus.subscribe(stream.id(), pattern("c")), Ok(true));
}

#[tokio::test]
async fn close_stops_dispatcher_and_disconnects_everyone() {
    let (bus, task) = spawn_bus(Limits::default());
    let mut stream = subscribed(&bus, &["a"]);
    publish(&bus, "a", json!(1));
    assert_eq!(next(&mut stream).await.data, json!(1));

    bus.close();
    assert_eq!(
        bus.publish(internal("a"), Value::Null, None),
        Err(PublishError::Closed)
    );
    tokio::time::timeout(WAIT, task)
        .await
        .expect("dispatcher did not stop")
        .unwrap();
    assert_disconnected(&mut stream, DisconnectReason::Shutdown).await;
}

#[tokio::test]
async fn dropping_the_dispatcher_disconnects_connections_and_closes_publishing() {
    let (bus, dispatcher) = EventBus::new(Limits::default());
    let mut stream = subscribed(&bus, &["a"]);
    drop(dispatcher);

    assert_disconnected(&mut stream, DisconnectReason::Shutdown).await;
    assert_eq!(
        bus.publish(internal("a"), Value::Null, None),
        Err(PublishError::Closed)
    );
}

#[tokio::test]
async fn registering_after_shutdown_yields_a_disconnected_stream() {
    let (bus, dispatcher) = EventBus::new(Limits::default());
    drop(dispatcher);

    let mut stream = bus.register_connection();
    assert_eq!(stream.disconnect_reason(), Some(DisconnectReason::Shutdown));
    assert_disconnected(&mut stream, DisconnectReason::Shutdown).await;
    assert_eq!(bus.connection_count(), 0);
    assert_eq!(
        bus.subscribe(stream.id(), pattern("a")),
        Err(SubscribeError::UnknownConnection)
    );
}

#[test]
fn registration_racing_with_shutdown_never_leaves_a_live_connection() {
    for _ in 0..200 {
        let (bus, dispatcher) = EventBus::new(Limits::default());
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let registrar = {
            let bus = bus.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                (0..20)
                    .map(|_| bus.register_connection())
                    .collect::<Vec<_>>()
            })
        };
        barrier.wait();
        drop(dispatcher);
        let streams = registrar.join().unwrap();

        assert_eq!(bus.connection_count(), 0);
        for stream in &streams {
            assert_eq!(stream.disconnect_reason(), Some(DisconnectReason::Shutdown));
        }
    }
}

#[test]
fn connection_ids_are_unique_across_buses() {
    let (first_bus, _first_dispatcher) = EventBus::new(Limits::default());
    let (second_bus, _second_dispatcher) = EventBus::new(Limits::default());
    let ids = [
        first_bus.register_connection().id(),
        second_bus.register_connection().id(),
        first_bus.register_connection().id(),
        second_bus.register_connection().id(),
    ];
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len());
}

#[test]
fn seq_keeps_increasing_across_buses() {
    let (first_bus, _first_dispatcher) = EventBus::new(Limits::default());
    let seq_a = first_bus.publish(internal("t"), Value::Null, None).unwrap();
    drop(first_bus);

    let (second_bus, _second_dispatcher) = EventBus::new(Limits::default());
    let seq_b = second_bus
        .publish(internal("t"), Value::Null, None)
        .unwrap();
    let (third_bus, _third_dispatcher) = EventBus::new(Limits::default());
    let seq_c = third_bus.publish(internal("t"), Value::Null, None).unwrap();
    let seq_d = second_bus
        .publish(internal("t"), Value::Null, None)
        .unwrap();

    assert!(seq_a < seq_b && seq_b < seq_c && seq_c < seq_d);
}
