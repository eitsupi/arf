use super::protocol;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

/// Session-only switch for interactive `send` approval. It intentionally is
/// not part of Config or startup CLI options.
static SEND_POLICY_ALLOW: OnceLock<AtomicBool> = OnceLock::new();

fn send_policy_allow() -> &'static AtomicBool {
    SEND_POLICY_ALLOW.get_or_init(|| AtomicBool::new(false))
}

pub fn set_send_policy_allow(allow: bool) {
    send_policy_allow().store(allow, Ordering::Release);
}

pub fn send_policy_is_allow() -> bool {
    send_policy_allow().load(Ordering::Acquire)
}

pub struct UserInputApproval {
    pub approved: bool,
    pub wrote_newline: bool,
}

/// Ask for approval immediately before an interactive `user_input` executes.
/// The sender check prevents a timed-out request from being executed after its
/// client has gone away. A half-close cannot be distinguished from the normal
/// request framing, so this is necessarily best effort until the server timeout.
pub fn approve_user_input(
    code: &str,
    reply: &tokio::sync::oneshot::Sender<protocol::IpcResponse>,
) -> UserInputApproval {
    if send_policy_is_allow() {
        return UserInputApproval {
            approved: !reply.is_closed(),
            wrote_newline: false,
        };
    }
    if reply.is_closed() {
        return UserInputApproval {
            approved: false,
            wrote_newline: false,
        };
    }
    let escaped = user_input_display(code);
    let prompt = format!(
        "# [arf] IPC send request: [{escaped}]\r\n# [arf] Press y to approve, any other key declines: "
    );
    use std::io::Write;
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let finish = |approved: bool| {
        // Raw terminals do not translate LF to CRLF; return to column zero
        // explicitly so the agent echo is rendered on the next line.
        if approved {
            print!("\r\n");
        } else {
            print!("\r\n# [arf] IPC send declined.\r\n");
        }
        let _ = std::io::stdout().flush();
        UserInputApproval {
            approved,
            wrote_newline: true,
        }
    };

    // Reedline normally owns raw mode, but the fast path can reach this
    // helper before it starts reading. Preserve whichever state we inherit.
    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw && crossterm::terminal::enable_raw_mode().is_err() {
        return finish(false);
    }
    struct RawModeGuard {
        was_raw: bool,
    }
    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            if !self.was_raw {
                let _ = crossterm::terminal::disable_raw_mode();
            }
        }
    }
    let _raw_mode_guard = RawModeGuard { was_raw };

    loop {
        if reply.is_closed() {
            return finish(false);
        }
        if let Err(error) = crossterm::event::poll(std::time::Duration::from_millis(50)) {
            log::debug!("IPC send approval input failed: {error}");
            return finish(false);
        }
        arf_libr::process_r_events();
        // Keep the IPC transport responsive while waiting. R is marked busy
        // by the caller, so later mutating requests receive R_BUSY.
        super::poll_ipc_requests();
        if !crossterm::event::poll(std::time::Duration::from_millis(0)).unwrap_or(false) {
            continue;
        }
        match crossterm::event::read() {
            Ok(crossterm::event::Event::Key(key)) => {
                use crossterm::event::{KeyCode, KeyModifiers};
                let approved = matches!(key.code, KeyCode::Char('y' | 'Y'))
                    && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
                    && !reply.is_closed();

                // Consume the rest of the answer before raw mode is released;
                // otherwise characters such as Enter can reach reedline.
                while crossterm::event::poll(std::time::Duration::from_millis(0)).unwrap_or(false) {
                    if crossterm::event::read().is_err() {
                        break;
                    }
                }
                return finish(approved);
            }
            Ok(_) => continue,
            Err(error) => {
                log::debug!("IPC send approval input failed: {error}");
                return finish(false);
            }
        }
    }
}

pub(crate) fn user_input_display(code: &str) -> String {
    code.chars()
        .map(|character| {
            if character.is_ascii_graphic() || character == ' ' {
                character.to_string()
            } else {
                character.escape_default().to_string()
            }
        })
        .collect()
}

pub fn reject_user_input_not_approved(reply: tokio::sync::oneshot::Sender<protocol::IpcResponse>) {
    let _ = reply.send(protocol::IpcResponse::error(
        protocol::INPUT_NOT_APPROVED,
        "IPC send was not approved".to_string(),
    ));
}

#[cfg(test)]
mod approval_display_tests {
    #[test]
    fn display_includes_content_after_old_preview_limit() {
        let source = format!(
            "{}{}",
            "0123456789".repeat(24),
            r#"system("SHOULD_BE_VISIBLE")"#
        );
        let display = super::user_input_display(&source);

        insta::assert_snapshot!(display, @r###"012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789system("SHOULD_BE_VISIBLE")"###);
    }

    #[test]
    fn display_escapes_control_characters() {
        let source = "line\n\tline\r\0\x1b[31m";
        let display = super::user_input_display(source);

        insta::assert_snapshot!(display, @r###"line\n\tline\r\u{0}\u{1b}[31m"###);
        assert!(!display.contains('\n'));
        assert!(!display.contains('\r'));
        assert!(!display.contains('\0'));
        assert!(!display.contains('\x1b'));
    }
}
