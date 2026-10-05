# Help resolution fixtures

These fixtures are independently authored for arf. `generate.R` uses R to
generate two compiled Rd topics, alias mappings (including operators, a
duplicate alias, and an alias/key collision), and matching Rd metadata with
and without database keys. Tests copy them into minimal package directories
under temporary library roots and read them using Rust alone.

The separate `homepkg` database contains only `home`, so alias/key collision
tests reach external-package discovery rather than matching the current DB.

Regenerate with:

```sh
Rscript crates/arf-harp/tests/fixtures/help_resolution/generate.R crates/arf-harp/tests/fixtures/help_resolution
```
