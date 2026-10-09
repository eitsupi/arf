use super::*;
use arf_harp::help::HelpTopic;
use std::sync::Arc;

fn topic(package: &str, name: &str, aliases: &[&str], title: &str) -> HelpTopic {
    HelpTopic {
        package: package.to_owned(),
        package_dir: std::path::PathBuf::from(format!("/{package}")),
        topic: name.to_owned(),
        aliases: aliases.iter().map(|value| (*value).to_owned()).collect(),
        help_key: None,
        title: title.to_owned(),
        entry_type: "help".to_owned(),
    }
}

#[test]
fn cancelled_generation_is_not_published_and_shutdown_joins_worker() {
    use std::sync::{Barrier, mpsc};
    let (started_tx, started_rx) = mpsc::channel();
    let gate = Arc::new(Barrier::new(2));
    let hook_gate = Arc::clone(&gate);
    let hook = Arc::new(move |generation| {
        let _ = started_tx.send(generation);
        hook_gate.wait();
    });
    let topics: Arc<[HelpTopic]> =
        vec![topic("base", "mean", &["average"], "Arithmetic Mean")].into();
    let mut worker = SearchWorker::spawn_with_hook(topics, hook).unwrap();
    let old = worker.submit("mean".to_owned()).unwrap();
    assert_eq!(started_rx.recv().unwrap(), old);
    let current = worker.cancel().unwrap();
    assert_ne!(current, old);
    gate.wait();
    worker.shutdown_and_join().unwrap();
    assert!(worker.take_result(old).unwrap().is_none());
    assert!(worker.take_result(current).unwrap().is_none());
}

#[test]
fn newer_request_replaces_queued_request() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Barrier, mpsc};
    let (seen_tx, seen_rx) = mpsc::channel();
    let gate = Arc::new(Barrier::new(2));
    let hook_gate = Arc::clone(&gate);
    let first_call = Arc::new(AtomicBool::new(true));
    let hook_first_call = Arc::clone(&first_call);
    let hook = Arc::new(move |generation| {
        let _ = seen_tx.send(generation);
        if hook_first_call.swap(false, Ordering::SeqCst) {
            hook_gate.wait();
        }
    });
    let mut worker = SearchWorker::spawn_with_hook(Vec::<HelpTopic>::new().into(), hook).unwrap();
    let active = worker.submit("first".to_owned()).unwrap();
    assert_eq!(seen_rx.recv().unwrap(), active);
    let superseded = worker.submit("second".to_owned()).unwrap();
    let latest = worker.submit("third".to_owned()).unwrap();
    gate.wait();
    assert_eq!(seen_rx.recv().unwrap(), latest);
    assert_ne!(superseded, latest);
    let mut state = worker.shared.state.lock().unwrap();
    while state.result.is_none() && state.failure.is_none() {
        state = worker.shared.changed.wait(state).unwrap();
    }
    assert_eq!(state.result.as_ref().unwrap().generation, latest);
    drop(state);
    worker.shutdown_and_join().unwrap();
    assert!(worker.take_result(active).unwrap().is_none());
    assert!(worker.take_result(latest).unwrap().is_some());
}

#[test]
fn recoverable_worker_panic_is_reported_as_an_io_error() {
    use std::sync::{Barrier, mpsc};
    let (entered_tx, entered_rx) = mpsc::channel();
    let gate = Arc::new(Barrier::new(2));
    let hook_gate = Arc::clone(&gate);
    let hook = Arc::new(move |_generation| {
        let _ = entered_tx.send(());
        hook_gate.wait();
        panic!("injected search panic");
    });
    let mut worker = SearchWorker::spawn_with_hook(Vec::<HelpTopic>::new().into(), hook).unwrap();
    let generation = worker.submit("term".to_owned()).unwrap();
    entered_rx.recv().unwrap();
    gate.wait();
    let mut state = worker.shared.state.lock().unwrap();
    while state.failure.is_none() {
        state = worker.shared.changed.wait(state).unwrap();
    }
    drop(state);
    assert!(worker.take_result(generation).is_err());
    worker.shutdown_and_join().unwrap();
}
