---
name: Bug report
about: Report incorrect or unsafe compression, a panic, or an API problem
title: ""
labels: bug
---

**What happened**
A clear description of the bug.

**Input**
The exact prompt / input text, or a minimal reproduction. Note if it contains anything sensitive.

**Expected vs actual**
- Expected:
- Actual (compressed output / error / panic):

**Mode**
default (lossless fold) / `mode: "ir"` / other.

**Version**
`tokelang-core` version + `rustc --version`.

**Token counts (if relevant)**
Measured with `cl100k_base` (tiktoken): original vs compressed.
