//! Shared subprocess helper for arf-harp integration tests.

#[path = "../../src/test_support.rs"]
mod test_support;

pub(crate) use test_support::with_r_in_subprocess;

/// Give the subprocess an exact test name without duplicating module paths.
macro_rules! with_r {
    ($name:ident, $body:block) => {
        $crate::common::with_r_in_subprocess(
            concat!(module_path!(), "::", stringify!($name)),
            || $body,
        )
    };
}

pub(crate) use with_r;
