These small, independently authored compiled help databases mirror the resolver
fixtures in `arf-harp`. They are kept inside this crate so packaged console tests
are self-contained. They exercise real Rust-only resolution from viewer state
tests without initializing R.

`homepkg` contains only `home`. `resolverpkg` contains `first-topic` and
`second-topic`; its aliases and Rd metadata include `mean`, `lm`, `shared`,
operator aliases, and an alias/key collision. To regenerate, run `generate.R`
in `crates/arf-harp/tests/fixtures/help_resolution`, then copy the seven files
used here. The generation script is the source of these fixtures.
