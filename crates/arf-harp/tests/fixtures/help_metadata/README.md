# Help metadata fixtures

These fixtures were independently authored or generated upstream and copied
verbatim from the `v0.5.0-rc.2` tag of
[r-documentation-rs](https://github.com/eitsupi/r-documentation-rs/tree/v0.5.0-rc.2).
The source files are under `crates/rd-helpdb/tests/fixtures/data/` in that
repository. The upstream MIT notice is included in
[`LICENSE.r-documentation-rs`](LICENSE.r-documentation-rs). These local copies
keep the `arf-harp` tests self-contained.

- `help_topics_metadata_v3.rds`: topic rows covering alias groups, missing
  values, and titles.
- `help_topics_aliases_only_v3.rds`: topic metadata with omitted Name and
  Title columns.
- `demo_valid_v3.rds`: a valid two-column demo index.
- `vignette_reordered_v3.rds`: a valid vignette data frame with reordered
  columns.
