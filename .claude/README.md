# Claude Code configuration

Project-level configuration, committed so every contributor gets the same
behaviour. Personal overrides go in `.claude/settings.local.json`, which is
git-ignored.

## Agents

| Agent | Use it for |
|---|---|
| `mep-reviewer` | Anything under `parts/`, or code modelling routes, fittings, ports, sizing or take-off. Catches what a programmer without site experience gets wrong: bend radii, stocked sizes, missing connections, wrong plan symbols, invented IFC property names. |
| `interop-guard` | Changes to `crates/od-io-*`, or to entity structures in `od-core`. Guards the lossless round trip — angle conventions, coordinate precision, multi-byte text splitting, and the writer arm that gets forgotten. |
| `layering-guard` | Any change that adds a dependency, module or type to `od-core`. Enforces the boundaries from `docs/02-architecture.md`. |

Invoke with the Agent tool, e.g. *"have mep-reviewer check the new valve
definitions"*.

## Skills

| Skill | Use it for |
|---|---|
| `add-part` | Adding or fixing a part in the catalogue. Covers the local-frame conventions, the expression language, and what the loader does and does not check. |
| `dxf-entity` | Adding support for a DXF entity type, or fixing a read/write bug. Walks the four places that must change, in order — the fourth is the one that gets missed. |

## Commands

| Command | What it does |
|---|---|
| `/verify` | Runs everything CI runs: Rust tests and clippy, TypeScript lint/typecheck/tests, and the round-trip and catalogue gates. |
| `/drawing <path.dxf>` | Inspects a drawing: contents, what cannot be modelled, round-trip result, standards findings. |

## Hooks

`format-touched.sh` runs after every Edit or Write and formats just that file
(`rustfmt` or `prettier`). Formatting the whole workspace on every edit is slow
enough that people turn it off, and a formatter that is off is not a convention.

## Permissions

Read-only and build commands are allowed without prompting. Anything that
publishes — `git push`, `gh pr create`, `cargo publish` — asks first. Reading
`.env` files and private keys is denied outright.
