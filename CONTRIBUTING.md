# Contributing to tokelang-core

Thanks for your interest in improving Tokelang.

## Ground rules

- Be respectful — see [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
- Open an issue before large changes so we can align on the approach.
- Keep one logical change per pull request.

## Developer Certificate of Origin (DCO)

We use the [DCO](https://developercertificate.org/) instead of a CLA. Sign off every commit:

```bash
git commit -s -m "your message"
```

The `-s` flag adds a `Signed-off-by:` line certifying that you wrote the patch or otherwise have the
right to submit it under Apache-2.0. Pull requests without sign-off cannot be merged.

## Development

```bash
cargo test      # run the test suite
cargo clippy    # lints
cargo fmt       # format
```

CI runs `fmt`, `clippy`, and `test` on every pull request — please make sure they pass locally first.

## Token measurements

Tokelang savings are measured in real `cl100k_base` tokens (OpenAI `tiktoken`). If a change affects
compression, report token counts — never character counts.

## Changing default-mode behavior

Default-mode output is a stability contract. A PR that changes default-mode output must include
before/after token **and** content-recall evidence, and should be flagged clearly in the PR
description.

## License

By contributing, you agree that your contributions are licensed under the Apache License 2.0
(inbound = outbound).
