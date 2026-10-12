//! Unit tests for the sponge history queue.

use super::SpongeQueue;
use reedline::HistoryItemId;

/// Helper to create a HistoryItemId for testing.
fn make_id(id: i64) -> HistoryItemId {
    HistoryItemId::new(id)
}

/// Helper to run a sequence of commands through SpongeQueue and collect deleted IDs.
fn run_commands(commands: &[(bool, i64)], delay: usize) -> Vec<HistoryItemId> {
    let mut queue = SpongeQueue::new();
    let mut deleted = Vec::new();

    for &(is_failure, id) in commands {
        let history_id = Some(make_id(id));
        if let Some(id_to_delete) = queue.record_command(is_failure, history_id, delay) {
            deleted.push(id_to_delete);
        }
    }

    deleted
}

#[test]
fn test_sponge_delay_keeps_recent_failures() {
    // With delay=2, failed commands should remain accessible for 2 more commands
    // Command sequence: fail(1), success, success -> fail(1) should be deleted
    let commands = vec![
        (true, 1),  // Failed command with ID 1
        (false, 2), // Success
        (false, 3), // Success -> now queue has 3 entries, purge oldest
    ];

    let deleted = run_commands(&commands, 2);
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0], make_id(1));
}

#[test]
fn test_sponge_delay_zero_deletes_immediately() {
    // With delay=0, failed commands are deleted immediately
    let commands = vec![
        (true, 1), // Failed -> immediately deleted (queue > 0)
    ];

    let deleted = run_commands(&commands, 0);
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0], make_id(1));
}

#[test]
fn test_sponge_success_does_not_delete() {
    // Successful commands never cause deletions directly
    let commands = vec![
        (false, 1), // Success
        (false, 2), // Success
        (false, 3), // Success -> purges None entries
    ];

    let deleted = run_commands(&commands, 2);
    assert!(deleted.is_empty(), "No failures to delete");
}

#[test]
fn test_sponge_multiple_failures_fifo() {
    // Multiple failures should be deleted in FIFO order
    // delay=2: keep 2 entries, delete older ones
    let commands = vec![
        (true, 1),  // fail(1) -> queue: [Some(1)]
        (true, 2),  // fail(2) -> queue: [Some(2), Some(1)]
        (true, 3),  // fail(3) -> queue: [Some(3), Some(2), Some(1)] -> delete 1
        (false, 4), // success -> queue: [None, Some(3), Some(2)] -> delete 2
    ];

    let deleted = run_commands(&commands, 2);
    assert_eq!(deleted.len(), 2);
    assert_eq!(deleted[0], make_id(1)); // First to be deleted
    assert_eq!(deleted[1], make_id(2)); // Second to be deleted
}

#[test]
fn test_sponge_interleaved_success_failure() {
    // Interleaved success/failure pattern
    // delay=3
    let commands = vec![
        (true, 1),  // fail -> [Some(1)]
        (false, 2), // ok   -> [None, Some(1)]
        (true, 3),  // fail -> [Some(3), None, Some(1)]
        (false, 4), // ok   -> [None, Some(3), None, Some(1)] -> delete 1
        (false, 5), // ok   -> [None, None, Some(3), None] -> delete None (no-op)
    ];

    let deleted = run_commands(&commands, 3);
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0], make_id(1));
}

#[test]
fn test_sponge_large_delay_no_deletion() {
    // With a large delay, nothing gets deleted during the session
    let commands = vec![(true, 1), (true, 2), (true, 3), (false, 4), (false, 5)];

    let deleted = run_commands(&commands, 100);
    assert!(deleted.is_empty(), "Delay is larger than command count");
}

#[test]
fn test_sponge_delay_one_keeps_one_command() {
    // delay=1 means keep only the most recent command in queue
    let commands = vec![
        (true, 1),  // fail -> [Some(1)]
        (false, 2), // ok   -> [None, Some(1)] -> delete 1
        (true, 3),  // fail -> [Some(3), None] -> delete None (no-op)
        (true, 4),  // fail -> [Some(4), Some(3)] -> delete 3
    ];

    let deleted = run_commands(&commands, 1);
    assert_eq!(deleted.len(), 2);
    assert_eq!(deleted[0], make_id(1));
    assert_eq!(deleted[1], make_id(3));
}

#[test]
fn test_sponge_realistic_scenario() {
    // Realistic scenario: user typos a command, then fixes it
    // With delay=2, they can use up-arrow to see the failed command for 2 commands
    let commands = vec![
        (true, 100),  // typo: "gti status" -> [Some(100)]
        (false, 101), // fix: "git status" -> [None, Some(100)]
        (false, 102), // continue: "git add ." -> [None, None, Some(100)] -> delete 100
    ];

    let deleted = run_commands(&commands, 2);
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0], make_id(100));
}

#[test]
fn test_sponge_drain_failed_ids() {
    // Test drain_failed_ids for exit cleanup
    let mut queue = SpongeQueue::new();

    // Add some commands with a large delay (no immediate deletion)
    queue.record_command(true, Some(make_id(1)), 100);
    queue.record_command(false, Some(make_id(2)), 100);
    queue.record_command(true, Some(make_id(3)), 100);
    queue.record_command(false, Some(make_id(4)), 100);

    // Drain should return only the failed command IDs
    let drained: Vec<_> = queue.drain_failed_ids().collect();
    assert_eq!(drained.len(), 2);
    // drain_failed_ids returns in FIFO order (oldest first)
    assert_eq!(drained[0], make_id(1));
    assert_eq!(drained[1], make_id(3));

    // Queue should be empty after drain
    assert!(queue.is_empty());
}
