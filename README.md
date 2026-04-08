# tokelang-core

`tokelang-core` is the compiler library for the `v0.8.0` Tokelang Lite line.

It owns:

- span-aware clause segmentation
- instruction / modifier / entity / relation extraction
- typed semantic program construction
- compact word-based Tokelang emission
- compact Tokelang parsing
- compile-mode selection between Tokelang and passthrough

## Public API

```rust
use tokelang_core::{CompileMode, Engine};

let engine = Engine::new();
let compiled = engine
    .compile("Explain quantum entanglement in simple terms")
    .unwrap();

match compiled.mode {
    CompileMode::Tokelang => {
        let reparsed = engine.parse_compact(&compiled.compact).unwrap();
        assert_eq!(compiled.program.to_compact(), reparsed.to_compact());
    }
    CompileMode::Passthrough => panic!("expected tokelang mode for this prompt"),
}
```

## Design Notes

- `TokelangProgram` keeps the typed internal structure; only the public surface syntax changed.
- Compact parsing is part of the crate and must stay in sync with emission.
- `v0.8.0` removes symbol-driven escaping from the public format and favors a smaller word inventory instead.
