//! Registration of native call methods with embedded R.

use crate::error::{HarpError, HarpResult};
use arf_libr::{R_CallMethodDef, r_library};

/// Register a set of `.Call` methods immediately with embedded R.
///
/// The definitions are passed in a sentinel-terminated table to
/// `R_registerRoutines`, which records the routine metadata in the embedding
/// `DllInfo`. The table and names must remain valid until registration returns;
/// the registered function code must remain loaded while R can call it.
///
/// # Safety
/// Each name must point to a valid NUL-terminated C string for the duration of
/// registration. Each function pointer must match the `.Call` ABI, its code
/// must remain loaded while R can call it, and its arguments must match the
/// declared count.
pub unsafe fn register_call_methods(definitions: &[R_CallMethodDef]) -> HarpResult<()> {
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
