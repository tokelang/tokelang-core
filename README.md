# tokelang-core

`tokelang-core` is the production compiler library for Tokelang v0.7.0.

It owns:

- span-aware clause segmentation
- instruction / modifier / entity / relation extraction
- typed semantic IR construction
- compact Tokelang emission
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

- `TokelangIR` no longer stores a flat `subjects: Vec<String>`.
- Semantic content is represented through `SemanticFrame`.
- Compact parsing is part of the crate and must stay in sync with emission.
- Reserved symbol escaping is driven from the shared symbol registry in `symbols`.
