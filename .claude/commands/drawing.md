---
description: Inspect a DXF file — what is in it, what this build cannot model, whether it survives a round trip, and what a standards check says. Pass the path as an argument.
argument-hint: <path to .dxf>
---

Analyse the drawing at `$1` and report what a user would want to know before
working on it.

```bash
cargo run -q -p od-cli -- inspect "$1"
cargo run -q -p od-cli -- check "$1" --rules jp
cargo run -q -p od-cli -- roundtrip "$1"
```

Then write a short report covering:

- **What it is**: entity count, extents in metres, layer count. Say whether the
  extents look like a plan at building scale, a detail, or something with stray
  geometry a long way from the rest.
- **What we cannot model**: anything listed as preserved verbatim. State plainly
  that it survives a save but cannot be edited, and name the types — that list
  is the honest measure of how far this build is from handling the drawing.
- **Round trip**: whether it came back identical. If not, that is a bug in this
  project, not in the file; investigate before anything else.
- **Standards**: the findings, in the order they would matter to someone
  submitting the drawing.

Do not modify the file.
