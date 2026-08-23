---
description: Run every check the CI runs — Rust tests and clippy, TypeScript lint, typecheck and tests, and the catalogue and round-trip gates.
---

Run the full verification suite and report what fails.

```bash
# Rust
cargo fmt --all --check
cargo clippy --workspace --all-targets   # warnings are failures here
cargo test --workspace

# The two gates that protect the product's promises
cargo test -p od-io-dxf --test roundtrip
cargo test -p od-parts

# TypeScript
bun run format:check
bun run lint
bun run typecheck
OD_BIN="$PWD/target/debug/od" bun test apps/api
```

Run them all before reporting, rather than stopping at the first failure — a
partial answer sends the reader back for a second round.

For each failure, give the command, the essential output (not the whole log),
and what it means. Then fix what is clearly a mistake in the change under
review. Leave anything that looks like a deliberate decision to the user, and
say why you left it.

If everything passes, say so in one line with the test counts.
