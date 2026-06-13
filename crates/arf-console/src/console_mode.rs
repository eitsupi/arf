//! Console mode restoration helpers.

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::sync::atomic::AtomicPtr;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::Console::{
            ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        },
    };

    static ORIGINAL_INPUT_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
    static ORIGINAL_INPUT_MODE: AtomicU32 = AtomicU32::new(0);
    static HAS_ORIGINAL_INPUT_MODE: AtomicBool = AtomicBool::new(false);
    static ATEXIT_REGISTERED: AtomicBool = AtomicBool::new(false);

    /// Restores the original console input mode when dropped.
    ///
    /// `install()` disables input echo immediately so bracketed-paste payloads
    /// sent before the first reedline `read_line()` are not visibly echoed.
    ///
    /// R's `quit()` can terminate the process before Rust destructors run, so
    /// `install()` also registers an `atexit` handler using the same restore
    /// path. Calling the restore path more than once is harmless.
    pub(crate) struct ConsoleModeGuard;

    impl ConsoleModeGuard {
        pub(crate) fn install() -> Self {
            if !HAS_ORIGINAL_INPUT_MODE.load(Ordering::Acquire)
                && let Some(handle) = stdin_handle()
            {
                let mut mode = 0;
                if unsafe { GetConsoleMode(handle, &mut mode) } != 0 {
                    ORIGINAL_INPUT_HANDLE.store(handle, Ordering::Release);
                    ORIGINAL_INPUT_MODE.store(mode, Ordering::Release);
                    HAS_ORIGINAL_INPUT_MODE.store(true, Ordering::Release);
                    disable_echo_input(handle, mode);
                }
            }

            if HAS_ORIGINAL_INPUT_MODE.load(Ordering::Acquire) {
                register_atexit_restore();
            }

            Self
        }
    }

    impl Drop for ConsoleModeGuard {
        fn drop(&mut self) {
            restore_original_input_mode();
        }
    }

    fn stdin_handle() -> Option<HANDLE> {
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(handle)
        }
    }

    fn register_atexit_restore() {
        if ATEXIT_REGISTERED.swap(true, Ordering::AcqRel) {
            return;
        }

        let result = unsafe { libc::atexit(restore_original_input_mode_at_exit) };
        if result != 0 {
            ATEXIT_REGISTERED.store(false, Ordering::Release);
            log::warn!("Failed to register console mode restore with atexit");
        }
    }

    fn disable_echo_input(handle: HANDLE, mode: u32) {
        let new_mode = mode & !ENABLE_ECHO_INPUT;
        if new_mode != mode && unsafe { SetConsoleMode(handle, new_mode) } == 0 {
            log::warn!("Failed to disable console input echo");
        }
    }

    extern "C" fn restore_original_input_mode_at_exit() {
        restore_original_input_mode();
    }

    pub(crate) fn restore_original_input_mode() {
        if !HAS_ORIGINAL_INPUT_MODE.load(Ordering::Acquire) {
            return;
        }

        let handle = ORIGINAL_INPUT_HANDLE.load(Ordering::Acquire);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return;
        }

        let mode = ORIGINAL_INPUT_MODE.load(Ordering::Acquire);
        if unsafe { SetConsoleMode(handle, mode) } == 0 {
            log::warn!("Failed to restore original console input mode");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn echo_input_is_cleared() {
            assert_eq!(0, ENABLE_ECHO_INPUT & !ENABLE_ECHO_INPUT);
            assert_eq!(
                0,
                (ENABLE_ECHO_INPUT | 0x100) & !ENABLE_ECHO_INPUT & ENABLE_ECHO_INPUT
            );
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ORIGINAL_TERMIOS: Mutex<Option<libc::termios>> = Mutex::new(None);
    static ATEXIT_REGISTERED: AtomicBool = AtomicBool::new(false);

    /// Restores the original terminal mode when dropped.
    ///
    /// `install()` disables input echo immediately so bracketed-paste payloads
    /// sent before the first reedline `read_line()` are not visibly echoed.
    pub(crate) struct ConsoleModeGuard;

    impl ConsoleModeGuard {
        pub(crate) fn install() -> Self {
            let mut original = match ORIGINAL_TERMIOS.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };

            if original.is_none() {
                let mut mode: libc::termios = unsafe { std::mem::zeroed() };
                if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut mode) } == 0 {
                    *original = Some(mode);
                    disable_echo_input(&mode);
                }
            }

            if original.is_some() {
                register_atexit_restore();
            }

            Self
        }
    }

    impl Drop for ConsoleModeGuard {
        fn drop(&mut self) {
            restore_original_input_mode();
        }
    }

    fn register_atexit_restore() {
        if ATEXIT_REGISTERED.swap(true, Ordering::AcqRel) {
            return;
        }

        let result = unsafe { libc::atexit(restore_original_input_mode_at_exit) };
        if result != 0 {
            ATEXIT_REGISTERED.store(false, Ordering::Release);
            log::warn!("Failed to register terminal mode restore with atexit");
        }
    }

    fn disable_echo_input(mode: &libc::termios) {
        let mut new_mode = *mode;
        new_mode.c_lflag &= !libc::ECHO;
        if new_mode.c_lflag != mode.c_lflag
            && unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &new_mode) } != 0
        {
            log::warn!(
                "Failed to disable terminal input echo: {}",
                std::io::Error::last_os_error()
            );
        }
    }

    extern "C" fn restore_original_input_mode_at_exit() {
        restore_original_input_mode();
    }

    pub(crate) fn restore_original_input_mode() {
        let original = match ORIGINAL_TERMIOS.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let Some(mode) = original.as_ref() else {
            return;
        };

        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, mode) } != 0 {
            log::warn!(
                "Failed to restore terminal input mode: {}",
                std::io::Error::last_os_error()
            );
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn echo_flag_is_cleared() {
            let flags = libc::ECHO | libc::ICANON | libc::ISIG;
            let cleared = flags & !libc::ECHO;
            assert_eq!(0, cleared & libc::ECHO);
            assert_ne!(0, cleared & libc::ICANON);
            assert_ne!(0, cleared & libc::ISIG);
        }
    }
}

pub(crate) use imp::{ConsoleModeGuard, restore_original_input_mode};
