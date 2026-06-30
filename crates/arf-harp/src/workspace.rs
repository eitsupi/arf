//! Workspace snapshot — read .GlobalEnv bindings via R's C API.
//!
//! Active bindings are detected *before* value access to avoid forcing them.
//! Unforced promises are labelled without being evaluated.

use std::ffi::CString;

use arf_libr::{R_FALSE, R_TRUE, r_class_symbol, r_library, r_nil_value, r_unbound_value};

use crate::error::HarpResult;
use crate::protect::RProtect;

/// A single binding in .GlobalEnv.
#[derive(Debug, Clone)]
pub struct EnvEntry {
    /// Binding name.
    pub name: String,
    /// R typeof string (e.g. "integer", "closure").
    pub type_label: String,
    /// First class from class() or implied class.
    pub class_label: String,
    /// Rf_xlength of the object; None for active bindings and promises.
    pub size: Option<i64>,
    /// True for list/env/pairlist/S4 — drillable in Phase 2.
    pub has_children: bool,
    pub is_active_binding: bool,
    pub is_promise: bool,
}

/// Snapshot the current .GlobalEnv bindings.
///
/// Must be called from the R main thread.
pub fn workspace_snapshot() -> HarpResult<Vec<EnvEntry>> {
    let lib = r_library()?;
    let nil = r_nil_value()?;
    let unbound = r_unbound_value()?;
    let class_sym = r_class_symbol()?;

    let mut protect = RProtect::new();

    unsafe {
        let global_env = *lib.r_globalenv;

        // R_lsInternal3(env, all_names=TRUE, sorted=TRUE) — returns a STRSXP
        let names_vec = protect.protect((lib.r_lsinternal3)(global_env, R_TRUE, R_TRUE));

        let n = (lib.rf_length)(names_vec) as isize;
        let mut entries = Vec::with_capacity(n as usize);

        for i in 0..n {
            let name_sexp = (lib.string_elt)(names_vec, i);
            let name_cstr = (lib.r_charsxp)(name_sexp);
            if name_cstr.is_null() {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(name_cstr)
                .to_string_lossy()
                .into_owned();

            let name_bytes = match CString::new(name.as_bytes()) {
                Ok(s) => s,
                Err(_) => continue,
            };
            // Rf_install interns the symbol permanently — no GC triggered
            let sym = (lib.rf_install)(name_bytes.as_ptr());

            // Existence check (sanity — symbol came from R_lsInternal3)
            if (lib.r_existsvarinframe)(global_env, sym) == R_FALSE {
                continue;
            }

            // Check active binding BEFORE fetching the value.
            // R_BindingIsActive(sym, rho): sym is the first arg, env is second.
            let is_active = (lib.r_bindingisactive)(sym, global_env) != R_FALSE;

            if is_active {
                entries.push(EnvEntry {
                    name,
                    type_label: "function".to_string(),
                    class_label: "(active binding)".to_string(),
                    size: None,
                    has_children: false,
                    is_active_binding: true,
                    is_promise: false,
                });
                continue;
            }

            // Fetch value — safe now that we confirmed it is not an active binding
            let value = (lib.rf_findvarinframe)(global_env, sym);

            if value == unbound {
                continue;
            }

            // PROMSXP = 5: unforced lazy value — don't force it
            let type_int = (lib.rf_typeof)(value) as u32;

            if type_int == 5 {
                entries.push(EnvEntry {
                    name,
                    type_label: "promise".to_string(),
                    class_label: "(promise)".to_string(),
                    size: None,
                    has_children: false,
                    is_active_binding: false,
                    is_promise: true,
                });
                continue;
            }

            // Get explicit class attribute.
            // value is rooted through global_env; no alloc between here and string read.
            let class_sexp = (lib.rf_getattrib)(value, class_sym);
            let explicit_class: Option<String> = if class_sexp != nil {
                // class attribute must be a STRSXP (16)
                if (lib.rf_typeof)(class_sexp) as u32 == 16 {
                    let cc = (lib.r_charsxp)((lib.string_elt)(class_sexp, 0));
                    if cc.is_null() {
                        None
                    } else {
                        Some(std::ffi::CStr::from_ptr(cc).to_string_lossy().into_owned())
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let is_s4 = (lib.rf_iss4)(value) != R_FALSE;
            let type_label = typeof_to_type_label(type_int).to_string();
            let class_label = explicit_class
                .unwrap_or_else(|| typeof_to_implicit_class(type_int, is_s4).to_string());
            let size = Some((lib.rf_xlength)(value) as i64);
            // LISTSXP=2, ENVSXP=4, VECSXP=19 have drillable children
            let has_children = matches!(type_int, 2 | 4 | 19) || is_s4;

            entries.push(EnvEntry {
                name,
                type_label,
                class_label,
                size,
                has_children,
                is_active_binding: false,
                is_promise: false,
            });
        }

        Ok(entries)
    }
}

fn typeof_to_type_label(type_int: u32) -> &'static str {
    match type_int {
        0 => "NULL",
        1 => "symbol",
        2 => "pairlist",
        3 => "closure",
        4 => "environment",
        5 => "promise",
        6 => "language",
        7 => "special",
        8 => "builtin",
        10 => "logical",
        13 => "integer",
        14 => "double",
        15 => "complex",
        16 => "character",
        17 => "...",
        19 => "list",
        20 => "expression",
        24 => "raw",
        25 => "S4",
        _ => "unknown",
    }
}

fn typeof_to_implicit_class(type_int: u32, is_s4: bool) -> &'static str {
    if is_s4 {
        return "S4";
    }
    match type_int {
        0 => "NULL",
        1 => "name",
        2 => "pairlist",
        3 | 7 | 8 => "function",
        4 => "environment",
        6 => "call",
        10 => "logical",
        13 => "integer",
        14 => "numeric",
        15 => "complex",
        16 => "character",
        19 => "list",
        20 => "expression",
        24 => "raw",
        _ => "unknown",
    }
}
