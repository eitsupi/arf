use std::sync::RwLock;

/// Spinner configuration (frames and color to display).
struct SpinnerConfig {
    /// Animation frames as a string (each character is one frame).
    frames: String,
    /// ANSI color code for the spinner (e.g., "\x1b[36m" for cyan).
    color_code: String,
}

static SPINNER_CONFIG: RwLock<SpinnerConfig> = RwLock::new(SpinnerConfig {
    frames: String::new(),
    color_code: String::new(),
});

/// Spinner thread state for animated busy indicator.
/// Uses a separate thread to animate the spinner while R is evaluating code.
pub(super) struct SpinnerThread {
    /// Signal to stop the spinner thread.
    stop_signal: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Handle to the spinner thread.
    handle: Option<std::thread::JoinHandle<()>>,
}

pub(super) static SPINNER_THREAD: std::sync::Mutex<Option<SpinnerThread>> =
    std::sync::Mutex::new(None);

/// Configure the spinner animation frames.
///
/// The `frames` string contains characters to cycle through.
/// An empty string disables the spinner.
///
/// Example: `"⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"` for braille dots spinner.
pub fn set_spinner_frames(frames: &str) {
    if let Ok(mut config) = SPINNER_CONFIG.write() {
        config.frames = frames.to_string();
    }
}

/// Configure the spinner color.
///
/// The `color_code` should be an ANSI escape sequence for the color,
/// e.g., "\x1b[36m" for cyan.
pub fn set_spinner_color(color_code: &str) {
    if let Ok(mut config) = SPINNER_CONFIG.write() {
        config.color_code = color_code.to_string();
    }
}

/// Start the spinner (display the busy indicator).
///
/// Spawns a background thread that animates the spinner at ~12.5fps.
/// The spinner is stopped automatically when R output is produced or
/// when the next ReadConsole prompt is displayed.
pub fn start_spinner() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    // Get the frames and color from config
    let (frames, color_code) = match SPINNER_CONFIG.read() {
        Ok(config) => (config.frames.clone(), config.color_code.clone()),
        Err(_) => return,
    };

    if frames.is_empty() {
        return; // Spinner disabled
    }

    // Check if already running
    let mut spinner_guard = match SPINNER_THREAD.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    if spinner_guard.is_some() {
        return; // Already running
    }

    // Create stop signal
    let stop_signal = Arc::new(AtomicBool::new(false));
    let stop_signal_clone = stop_signal.clone();

    // ANSI reset code
    const ANSI_RESET: &str = "\x1b[0m";

    // Spawn the spinner thread
    let handle = thread::spawn(move || {
        let frames_chars: Vec<char> = frames.chars().collect();
        if frames_chars.is_empty() {
            return;
        }

        let mut frame_index = 0;
        let frame_duration = Duration::from_millis(80); // ~12.5 fps for smooth animation

        // Display the first frame with color
        if color_code.is_empty() {
            print!("{} ", frames_chars[frame_index]);
        } else {
            print!("{}{}{} ", color_code, frames_chars[frame_index], ANSI_RESET);
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());

        loop {
            // Check stop signal at loop start
            if stop_signal_clone.load(Ordering::Relaxed) {
                break;
            }

            thread::sleep(frame_duration);

            // Check again after sleep for faster response to stop signal
            // This avoids unnecessary frame display when stop was called during sleep
            if stop_signal_clone.load(Ordering::Relaxed) {
                break;
            }

            // Advance to next frame
            frame_index = (frame_index + 1) % frames_chars.len();

            // Update the display: move cursor back and print new frame with color
            // \r moves to start of line, then print frame + space
            if color_code.is_empty() {
                print!("\r{} ", frames_chars[frame_index]);
            } else {
                print!(
                    "\r{}{}{} ",
                    color_code, frames_chars[frame_index], ANSI_RESET
                );
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    });

    *spinner_guard = Some(SpinnerThread {
        stop_signal,
        handle: Some(handle),
    });
}

/// Stop the spinner and clear it from the display.
///
/// This is called automatically when R produces output or when
/// the next prompt is about to be displayed.
pub fn stop_spinner() {
    use std::sync::atomic::Ordering;

    let mut spinner_guard = match SPINNER_THREAD.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    if let Some(spinner) = spinner_guard.take() {
        // Signal the thread to stop
        spinner.stop_signal.store(true, Ordering::Relaxed);

        // Wait for the thread to finish
        if let Some(handle) = spinner.handle {
            let _ = handle.join();
        }

        // Clear the spinner from the display
        print!("\r\x1b[K");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

/// Check if the spinner is currently active.
pub fn is_spinner_active() -> bool {
    SPINNER_THREAD
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
}
