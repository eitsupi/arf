use super::*;

impl Repl {
    /// Initialize both owned runtimes before IPC becomes reachable.
    pub(crate) fn prepare_history(&mut self) {
        if self.prepared_r_history.is_some() {
            return;
        }
        let (r_runtime, shell_runtime) = self.initialize_history_runtimes();
        Self::report_history_runtime("R", &r_runtime);
        Self::report_history_runtime("Shell", &shell_runtime);
        if let Some(warning) = self.history_location.legacy_default_warning() {
            eprintln!("Warning: {warning}");
            log::warn!("{warning}");
        }
        if let Some(store) = r_runtime.store() {
            crate::ipc::set_history_store(store);
        }
        if !r_runtime.is_available() {
            crate::ipc::clear_history_session_id();
        }
        self.prepared_r_history = Some(r_runtime);
        self.prepared_shell_history = Some(shell_runtime);
    }

    /// Build the independent R and shell owners without registering either
    /// one globally. This keeps construction testable and makes global IPC
    /// registration an explicit responsibility of `prepare_history`.
    pub(super) fn initialize_history_runtimes(&self) -> (HistoryRuntime, HistoryRuntime) {
        use crate::history::artifact::HistoryKind;

        let r_runtime = HistoryRuntime::initialize_for_location(
            &self.config.history.mode,
            &self.history_location,
            "r.db",
            HistoryKind::R,
            self.session_id,
            Some(chrono::Utc::now()),
        );
        let shell_runtime = HistoryRuntime::initialize_for_location(
            &self.config.history.mode,
            &self.history_location,
            "shell.db",
            HistoryKind::Shell,
            self.session_id,
            Some(chrono::Utc::now()),
        );
        (r_runtime, shell_runtime)
    }

    pub(super) fn report_history_runtime(label: &str, runtime: &HistoryRuntime) {
        if let Some(diagnostic) = runtime.startup_warning() {
            eprintln!("Warning: {label} history: {diagnostic}");
            log::warn!("{label} history: {diagnostic}");
        }
    }

    pub(super) fn prepared_r_history(&self) -> HistoryRuntime {
        self.prepared_r_history
            .clone()
            .expect("history runtimes must be prepared before the REPL starts")
    }

    pub(super) fn prepared_shell_history(&self) -> HistoryRuntime {
        self.prepared_shell_history
            .clone()
            .expect("history runtimes must be prepared before the REPL starts")
    }

    /// Get the history session ID as an i64 (for IPC).
    pub(crate) fn history_session_id_raw(&self) -> Option<i64> {
        self.prepared_r_history()
            .store()
            .and_then(|store| store.session())
            .map(i64::from)
    }
}

#[cfg(test)]
mod tests;
