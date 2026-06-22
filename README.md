# tokelang-core

[![CI](https://github.com/tokelang/tokelang-core/actions/workflows/ci.yml/badge.svg)](https://github.com/tokelang/tokelang-core/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)

The compression engine behind [Tokelang](https://tokelang.com) — English-prompt compression
middleware over standard tokenizers, with a content-recall validator and safe passthrough fallback.

`tokelang-core` is the Rust library that powers the Tokelang Lite line. It compresses
natural-language prompts to fewer tokens while preserving meaning, and returns the original text
unchanged whenever compression would not be safe.

## Status

Pre-1.0 (`v0.9.x` line). The public API may change before `1.0.0`.

## Install

```toml
[dependencies]
tokelang-core = "0.9"
```

(Published to crates.io at launch.)

## Quickstart

```rust
use tokelang_core::{CompileMode, Engine};

let engine = Engine::new();
let compiled = engine
    .compile("Explain quantum entanglement in simple terms")
    .unwrap();

match compiled.mode {
    CompileMode::Tokelang => println!("compressed: {}", compiled.compact),
    CompileMode::Passthrough => println!("kept original (compression was not safe)"),
}
```

## How it works

- **Default mode** — a provably-lossless general-text fold (`v0.9.6`+): safe word-level compression
  gated by a content-recall validator. If compression would lose meaning, the engine falls back to
  passthrough and returns the original text.
- **`mode: "ir"`** *(opt-in)* — the structured instruction-IR path. Higher savings, but lossy on
  some multi-intent prompts; off by default.

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the full design.

## Token measurement

All token counts use `cl100k_base` (OpenAI `tiktoken`). Character counts are never reported as token
counts.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Contributions are accepted under Apache-2.0 with a DCO
sign-off (`git commit -s`).

## License

Licensed under the [Apache License 2.0](LICENSE). "Tokelang" is a trademark — see
[`TRADEMARKS.md`](TRADEMARKS.md).
