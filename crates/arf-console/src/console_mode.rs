//! Console mode restoration helpers.

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::sync::atomic::AtomicPtr;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::Console::{GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode},
    };

    static ORIGINAL_INPUT_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
    static ORIGINAL_INPUT_MODE: AtomicU32 = AtomicU32::new(0);
    static HAS_ORIGINAL_INPUT_MODE: AtomicBool = AtomicBool::new(false);
    static ATEXIT_REGISTERED: AtomicBool = AtomicBool::new(false);

    /// Restores the original console input mode when dropped.
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
        unsafe {
            let _ = SetConsoleMode(handle, mode);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(crate) struct ConsoleModeGuard;

    impl ConsoleModeGuard {
        pub(crate) fn install() -> Self {
            Self
        }
    }
}

pub(crate) use imp::ConsoleModeGuard;
