# tokelang-core architecture

This document orients a contributor opening `tokelang-core` for the first time. It explains
*what the crate does*, *how a prompt flows through it*, and *why some of the seemingly redundant
machinery exists*. For the public API reference, run `cargo doc --no-deps --open`.

## What this crate is

`tokelang-core` is the compression engine behind **Tokelang Lite** — pragmatic English-compression
middleware that sits in front of a standard LLM tokenizer. Given a natural-language prompt it
returns either a shorter **compact** form that preserves the instruction's meaning, or — when it
cannot do that safely — the **original prompt unchanged**. It never silently returns something
lossy in preference to the original: when in doubt, it passes through.

It is *not* a custom tokenizer or a new language to learn (that is the separate long-term
"Tokelang" research track). Everything here operates over ordinary English and emits text that a
normal model reads natively.

## The one invariant that matters

**Safety beats savings.** Every routing and validation decision in this crate exists to answer a
single question: *is the compact form safe to return instead of the original?* If the answer is
ever uncertain, the engine returns the original prompt. A compression that drops a negation, a
file path, a number, or an instruction is a correctness bug — not an aggressive optimization.
This is why the code has more guard rails than a naive "fold synonyms" pass would need.

## Top-level flow

`Engine::compile` dispatches on the input **mode** ([`InputMode`]). Since **v0.9.6** the **default**
path is a provably-lossless lexical fold; the instruction-IR is opt-in (`mode:"ir"`).

```
   input: &str ─► Engine::compile_with_options ─► match mode:

   default            ─► general_text::candidate   (lossless fold: drop only stopwords / request
   (the lossless         wrappers it can prove are safe; keep every critical token + hard zone)
    fold)             ─► validate_or_recover       (recall floor + critical-token + protected-span;
                          unsafe → Passthrough)     — never invokes the IR, so it cannot drop a span

   mode:"ir"          ─► compiler::Compiler ─► TokelangProgram (typed IR + source spans)
   (opt-in,           ─► serialize IR ─► compact ─► RoutingSignals (savings / anchor gate)
    aggressive)       ─► validate_or_recover ─► Tokelang or Passthrough

   mode:"context_file"─► literal-island route (higher recall floor + hard-zone / protected-span
   (reused / system      preservation) ─► Tokelang or Passthrough
    prompts)
                                          │
                                          ▼
                              CompileResult { program, compact, mode }
```

`mode` is a [`CompileMode`]: `Tokelang` means the `compact` string is the compressed form to send
onward; `Passthrough` means `compact` is the original input verbatim and no compression was applied.

**Why the IR is opt-in (v0.9.6).** The IR re-serializes a prompt from typed blocks, so any input span
not assigned to a block is silently dropped — which deleted whole instructions, negations, and file
paths on multi-intent / pasted / delegation-contract prompts (the NB#29 bug class). The default fold
has no clause-segmentation step and therefore *cannot* drop a span: it removes only function words it
can prove are safe, validates against the recall floor, and passes through on any doubt. The IR is
preserved behind `mode:"ir"` for callers who explicitly want aggressive restructuring.

## Modules

| Module | Role |
|---|---|
| `engine` | Public facade. `Engine` owns the compiler, tokenizer, and compile cache. Entry points: `compile`, `compile_with_options`, `parse_compact`. Decides Tokelang-vs-Passthrough. |
| `compiler` | Natural-language → IR pipeline (the opt-in `mode:"ir"` path since v0.9.6): `segment` → `normalize` → `pipeline` (parse into typed blocks with `source_span`s) → emit compact. `coverage` tracks which input spans the IR accounts for. Frozen — not the default path. |
| `ir` | The typed semantic IR (`TokelangProgram`, `TokelangBlock`, `SemanticFrame`, `Entity`, `Relation`, …) and the parser that reads a compact string back into a program. Reached via `mode:"ir"`. |
| `symbols` | Single source of truth for the compact vocabulary — instruction keywords, modifiers, output formats, subject abbreviations, and synonym resolution. |
| `general_text` | **The default compile path (v0.9.6+).** Lossless general-text fold — conservative lexical compression that provably preserves content (every critical token + hard zone survives, or it passes through). |
| `validator` | Content-recall safety net. Compares compact against the original and forces passthrough when meaning-bearing tokens are lost. |
| `token_metrics` | Token counting. Uses `cl100k_base` via a `tiktoken` Python worker; falls back to a labelled proxy count when the worker is unavailable. |
| `options` | `CompileOptions`, `InputMode`, and `ProtectedRange` — caller-supplied compilation inputs. |

## Input modes (`InputMode`)

- **`Default`** (wire: `"default"`) — per-call user prompts. The **lossless `general_text` fold**:
  optimizes for savings under the safety invariant; never invokes the IR.
- **`ContextFile`** (wire: `"context_file"`) — system prompts, agent personas, RAG headers. The
  literal-island route with a higher recall floor (`CONTEXT_FILE_RECALL_FLOOR = 0.85`) because these
  texts are reused across many calls and are less tolerant of any loss.
- **`Ir`** (wire: `"ir"`, also accepts `"structured"`) — opt-in instruction-IR restructuring (the
  pre-v0.9.6 default). Aggressive clause/entity restructuring that can raise savings on long
  multi-step instructions but may drop spans on multi-intent prompts; provided for callers who
  explicitly accept that trade-off.

## Why the layered passthrough predicates exist

`engine.rs` carries several stacked heuristics (`RoutingSignals`: token-savings gate, workflow
scaffold detection, tuple-row counts, anchor/contract/locality hits, inline-equation detection,
…). To a newcomer this looks redundant — *why not just one threshold?*

They are **defensive sediment from real failure modes.** Early versions routed on a *proxy*
tokenizer count rather than true `cl100k`; the layered predicates were added to stop specific
classes of prompt (code-heavy text, dense specs, math, structured tuples) from being compressed
when compression would corrupt them. Each predicate maps to a category of prompt we observed
getting damaged. They are intentionally conservative: a false "pass through" only costs savings,
while a false "compress" costs correctness. Unifying or pruning them is tracked tech debt — it is a
*RISKY* change (it moves which prompts route which way) and must go through its own iteration with
the byte-identical regression gate, never bundled opportunistically.

## The token-savings gate

A compact form is only worth returning if it is meaningfully shorter. The engine requires at least
`MIN_TOKEN_SAVINGS_PCT_FOR_TOKELANG` (5%) token reduction — or `LOW_RISK_WORKFLOW_MIN_…` (3%) for
low-risk workflow-shaped prompts — measured in real `cl100k` tokens. Below the gate, it passes
through: a 1% "saving" is not worth any risk of meaning drift.

## The compile cache

`Engine` memoizes results keyed on `(schema, profile, input, protected_ranges, mode)`, but only for
inputs ≥ `MIN_CHARS_FOR_COMPILE_CACHE` (256 chars) — short prompts recompute cheaply. Recompilation
is deterministic, so the cache is purely a performance aid; `compile_cache_stats()` exposes hit/miss
counts. (The cache is currently unbounded — adding LRU eviction is tracked tech debt.)

## Protected ranges

Callers can pin byte ranges via `ProtectedRange` (e.g. quoted literals, code spans). These are
normalized (sorted, merged, UTF-8-boundary- and overlap-checked) and excluded from compression so
their bytes survive verbatim.

## Direction (forward-looking)

**v0.9.6 made the `general_text` fold the default and demoted the instruction-IR to opt-in
`mode:"ir"`** — removing the multi-intent span-drop failure class (NB#29) by construction while
keeping the IR available for callers that want richer structure. The IR (`compiler/`, `ir/`) is now
*frozen*: no further IR development. The next direction is a classify-then-route front gate ("MEC")
that dispatches each prompt to the cheapest safe route; it ships only once it beats the fold on
token-weighted savings without adding lossy cases. Expect `engine.rs` routing to keep evolving there.

## Known limitations & tracked tech debt

- Routing predicates in `engine.rs` are conservative and overlapping (see above) — consolidation
  is deferred to a dedicated iteration.
- The compile cache has no eviction/size cap yet.
- `compiler/pipeline.rs` is large and slated for splitting into phase modules.
- Some per-compile computations (lowercasing, candidate generation) are recomputed rather than
  memoized.

These are intentional, known, and tracked — not undiscovered bugs. The guiding principle for any
change here: **justify it by a general rule that holds across prompt families, never by making one
example look better.**
