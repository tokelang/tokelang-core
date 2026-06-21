# Changelog

All notable changes to `tokelang-core` are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project follows semantic versioning;
while pre-1.0, minor versions may include breaking API changes.

## [Unreleased]

### Added

- Open-source release scaffolding: `LICENSE` (Apache-2.0), `NOTICE`, `CONTRIBUTING`, `SECURITY`,
  `CODE_OF_CONDUCT`, `TRADEMARKS`, CI workflow, and issue/PR templates.
- Crate metadata for crates.io publishing (`license`, `description`, `repository`, `keywords`,
  `categories`).

## [0.9.6]

### Changed

- Default `compile` mode is now the provably-lossless general-text fold. The structured
  instruction-IR path is demoted to opt-in `mode: "ir"`. Default mode no longer drops instructions,
  negations, or paths on multi-intent prompts.
