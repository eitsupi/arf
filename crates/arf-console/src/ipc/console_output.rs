//! Bounded batching for independently observed R console output.
//!
//! Lock order is OUTPUT → CURRENT slot → event bus publish/sequence. No R
//! calls occur while these locks are held. The 32 KiB text and 256-segment
//! caps keep JSON below 1 MiB even if every text byte expands to a six-byte
//! JSON escape; actual event size is checked by the event bus before publish.

use std::sync::Mutex;

use serde::Serialize;
use serde_json::{Value, json};

use super::events::{ExecutionId, PublishError, Topic};

const BUFFER_LIMIT: usize = 32 * 1024;
#[cfg(test)]
const EVENT_LIMIT: usize = 1024 * 1024;
const SEGMENT_LIMIT: usize = 256;

#[derive(Clone, Serialize)]
struct Segment {
    stream: &'static str,
    text: String,
}

#[derive(Default)]
struct Buffer {
    execution: Option<ExecutionId>,
    origin: Option<Value>,
    visible: bool,
    segments: Vec<Segment>,
    bytes: usize,
}

static OUTPUT: Mutex<Buffer> = Mutex::new(Buffer {
    execution: None,
    origin: None,
    visible: false,
    segments: Vec::new(),
    bytes: 0,
});

/// Install the observer once, before R can execute user code.
pub(crate) fn install_observer() {
    arf_libr::set_console_output_observer(observe);
}

pub(crate) fn install_bus(bus: super::events::EventBus) -> super::events::Generation {
    let mut output = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
    flush_locked(&mut output);
    reset(&mut output);
    super::events::install_current_bus(bus)
}

pub(crate) fn clear_bus(generation: super::events::Generation) -> bool {
    let mut output = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
    if !super::events::is_current_generation(generation) {
        return false;
    }
    flush_locked(&mut output);
    let cleared = super::events::clear_current_bus(generation);
    if cleared {
        reset(&mut output);
    }
    cleared
}

fn reset(output: &mut Buffer) {
    output.execution = None;
    output.origin = None;
    output.visible = false;
    output.segments.clear();
    output.bytes = 0;
}

fn observe(text: &str, is_error: bool) {
    if !super::events::has_current_bus() {
        return;
    }
    let Some(context) = crate::ipc::execution::console_output_context() else {
        return;
    };
    let stream = if is_error { "stderr" } else { "stdout" };
    let mut output = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
    if !super::events::has_current_bus() {
        return;
    }
    if output.execution != Some(context.0) {
        flush_locked(&mut output);
        output.execution = Some(context.0);
        output.origin = Some(context.1);
        output.visible = context.2;
    }
    append(&mut output, stream, text, flush_locked);
}

fn append(
    output: &mut Buffer,
    stream: &'static str,
    text: &str,
    mut flush: impl FnMut(&mut Buffer),
) {
    let mut remaining = text;
    while !remaining.is_empty() {
        let new_segment = output
            .segments
            .last()
            .is_none_or(|last| last.stream != stream);
        if output.bytes == BUFFER_LIMIT || (new_segment && output.segments.len() == SEGMENT_LIMIT) {
            flush(output);
            continue;
        }
        let available = BUFFER_LIMIT - output.bytes;
        let mut end = remaining.len().min(available);
        while !remaining.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            flush(output);
            continue;
        }
        let (chunk, rest) = remaining.split_at(end);
        if new_segment {
            output.segments.push(Segment {
                stream,
                text: chunk.to_owned(),
            });
        } else {
            output.segments.last_mut().unwrap().text.push_str(chunk);
        }
        output.bytes += chunk.len();
        remaining = rest;
    }
    if output.bytes == BUFFER_LIMIT || output.segments.len() == SEGMENT_LIMIT {
        flush(output);
    }
}

fn event_data(output: &Buffer, segments: &[Segment]) -> Value {
    json!({"origin": output.origin, "visible": output.visible, "segments": segments})
}

fn flush_locked(output: &mut Buffer) {
    if output.segments.is_empty() {
        return;
    }
    let topic = Topic::internal("arf.console_output").expect("built-in topic is valid");
    publish(output, &topic, &output.segments);
    output.segments.clear();
    output.bytes = 0;
}

fn publish(output: &Buffer, topic: &Topic, segments: &[Segment]) {
    let data = event_data(output, segments);
    if let Err(error) = super::events::publish_event_raw(topic.clone(), data, output.execution)
        && !matches!(error, PublishError::Closed)
    {
        log::debug!("Could not publish console output: {error}");
    }
}

/// Flush buffered output. The lock stays held through publication to keep
/// publication sequence aligned with observed R callback order.
pub(crate) fn flush() {
    let mut output = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
    flush_locked(&mut output);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_text_and_segment_count_fit_serialized_event_limit() {
        let mut buffer = Buffer::default();
        let topic = Topic::internal("arf.console_output").unwrap();
        let mut batches: Vec<Vec<Segment>> = Vec::new();
        let mut take_batch = |buffer: &mut Buffer| {
            batches.push(std::mem::take(&mut buffer.segments));
            buffer.bytes = 0;
        };
        for index in 0..600 {
            append(
                &mut buffer,
                if index % 2 == 0 { "stdout" } else { "stderr" },
                "\u{1}語",
                &mut take_batch,
            );
        }
        append(
            &mut buffer,
            "stdout",
            &format!("{}{}", "\u{1}".repeat(20_000), "界".repeat(20_000)),
            &mut take_batch,
        );
        take_batch(&mut buffer);

        let mut reconstructed = Vec::new();
        for batch in batches {
            let byte_count: usize = batch.iter().map(|segment| segment.text.len()).sum();
            assert!(byte_count <= BUFFER_LIMIT);
            assert!(batch.len() <= SEGMENT_LIMIT);
            let data = event_data(&Buffer::default(), &batch);
            let payload_size = serde_json::to_vec(&data).unwrap().len();
            assert!(payload_size + topic.as_str().len() + 128 < EVENT_LIMIT);
            reconstructed.extend(
                batch
                    .into_iter()
                    .map(|segment| (segment.stream, segment.text)),
            );
        }
        let mut expected = Vec::new();
        for index in 0..600 {
            expected.push((
                if index % 2 == 0 { "stdout" } else { "stderr" },
                "\u{1}語".to_owned(),
            ));
        }
        expected.push((
            "stdout",
            format!("{}{}", "\u{1}".repeat(20_000), "界".repeat(20_000)),
        ));
        let mut merged: Vec<(&str, String)> = Vec::new();
        for (stream, text) in reconstructed {
            if let Some((last_stream, last_text)) = merged.last_mut()
                && *last_stream == stream
            {
                last_text.push_str(&text);
            } else {
                merged.push((stream, text));
            }
        }
        assert_eq!(merged.len(), expected.len());
        for ((stream, text), (expected_stream, expected_text)) in merged.iter().zip(expected) {
            assert_eq!(*stream, expected_stream);
            assert_eq!(text, &expected_text);
        }
    }
}
