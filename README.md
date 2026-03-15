# tokelang-core

Public API facade for the Tokelang engine. Wires the compiler, parser,
compression, and runtime subsystems into a single `Engine` type that exposes
the full pipeline: natural language in, compressed IR + expanded prompt out.

> **Security Notice:** This repository contains proprietary infrastructure for
> Tokelang. Do not distribute externally. Access is restricted to authorized
> team members.

## Usage

```rust
use tokelang_core::Engine;

let engine = Engine::new();

// Compile: natural language -> IR + compressed form
let result = engine.compile("Explain quantum entanglement in simple terms").unwrap();
assert_eq!(result.ir.to_compact(), "EXP:QENT:SIMPLE");

// Expand: IR -> optimized prompt
let prompt = engine.expand(&result.ir).unwrap();
assert!(prompt.contains("Explain"));
assert!(prompt.contains("quantum entanglement"));
```

## Types

- `Engine` — main entry point; holds the compiler, runtime, and prefix code tables
- `CompileResult` — contains the IR, compact string, and compressed compact string
- `EngineError` — unified error type wrapping compile, parse, compression, and runtime errors

## Re-exports

For convenience, `tokelang-core` re-exports key types from downstream crates:

- `Instruction`, `Modifier` from `tokelang-symbols`
- `TokelangIR` from `tokelang-parser`
- `CompressedIR`, `HuffmanTable`, `PrefixCodeTable` from `tokelang-compression`

## Dependencies

| Crate | Used for |
|---|---|
| `tokelang-symbols` | Token vocabulary |
| `tokelang-parser` | IR type and parsing |
| `tokelang-compiler` | NL-to-IR compilation |
| `tokelang-compression` | Prefix coding and deduplication |
| `tokelang-runtime` | IR-to-prompt expansion |
| `serde` | Serialization |
| `thiserror` | Error types |

## Part of the Tokelang engine

This crate is consumed as a git submodule of [`tokelang/tokelang`](https://github.com/tokelang/tokelang).
Clone the main repository with `--recursive` to get all crates:

```sh
git clone --recursive git@github.com:tokelang/tokelang.git
```

## License

MIT
