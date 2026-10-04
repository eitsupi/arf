//! Registration of native call methods with embedded R.

use crate::error::{HarpError, HarpResult};
use arf_libr::{R_CallMethodDef, r_library};
use once_cell::sync::OnceCell;

static EMBEDDING_ROUTINES_REGISTERED: OnceCell<()> = OnceCell::new();

/// Register arf's complete `.Call` table once with initialized embedded R.
///
/// Call this on the R thread. Successful registration lasts for the process's
/// single R runtime; subsequent calls are no-ops. Failed attempts may be retried.
/// Add future embedding callbacks to this table rather than registering them
/// separately from their feature modules.
pub fn register_embedding_routines() -> HarpResult<()> {
    EMBEDDING_ROUTINES_REGISTERED
        .get_or_try_init(|| {
            let definitions = [crate::help_bridge::help_submit_definition()];
            // SAFETY: All definitions belong to arf, have static names and code,
            // and declare the arity of their `.Call` callbacks.
            unsafe { register_complete_call_table(&definitions) }
        })
        .copied()
}

/// Register a complete `.Call` table immediately with embedded R.
///
/// The definitions are passed in a sentinel-terminated table to
/// `R_registerRoutines`, which records the routine metadata in the embedding
/// `DllInfo`. The table and names must remain valid until registration returns;
/// the registered function code must remain loaded while R can call it.
/// `R_registerRoutines` replaces the table for each non-null routine category
/// without freeing the old table. Never use it for incremental registration;
/// the production caller above must register the full table only once.
///
/// # Safety
/// Each name must point to a valid NUL-terminated C string for the duration of
/// registration. Each function pointer must match the `.Call` ABI, its code
/// must remain loaded while R can call it, and its arguments must match the
/// declared count.
unsafe fn register_complete_call_table(definitions: &[R_CallMethodDef]) -> HarpResult<()> {
    if definitions.iter().any(|definition| {
        definition.name.is_null() || definition.fun.is_none() || definition.num_args < 0
    }) {
        return Err(HarpError::RoutineRegistration(
            "definitions require non-null names and functions and non-negative arities".to_string(),
        ));
    }

    let lib = r_library()?;
    let get_info = lib.r_get_embedding_dll_info.ok_or_else(|| {
        HarpError::RoutineRegistration(
            "R_getEmbeddingDllInfo is unavailable in this R library".to_string(),
        )
    })?;
    let register = lib.r_register_routines.ok_or_else(|| {
        HarpError::RoutineRegistration("R_registerRoutines is unavailable".to_string())
    })?;

    let info = unsafe { get_info() };
    if info.is_null() {
        return Err(HarpError::RoutineRegistration(
            "R_getEmbeddingDllInfo returned no embedding DllInfo".to_string(),
        ));
    }

    let mut table = Vec::with_capacity(definitions.len() + 1);
    table.extend_from_slice(definitions);
    table.push(R_CallMethodDef {
        name: std::ptr::null(),
        fun: None,
        num_args: 0,
    });

    // Count actual native registrations in unit tests, since R's symbol-query
    // APIs return fresh metadata allocations even when the table is unchanged.
    #[cfg(test)]
    tests::REGISTRATION_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let status = unsafe {
        register(
            info,
            std::ptr::null(),
            table.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if status != 1 {
        return Err(HarpError::RoutineRegistration(format!(
            "R_registerRoutines returned status {status}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
