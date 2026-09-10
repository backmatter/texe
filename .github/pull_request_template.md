## What changed

<!-- Describe the user-visible behavior and why this is the smallest useful change. -->

## Compatibility and trust boundary

<!-- Note effects on manifests, locks, schemas, platforms, networking, command execution, or reproducibility. Write "None" when there are none. -->

## Verification

<!-- List the commands and acceptance journeys you ran. Explain any relevant environment limitation. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `RUSTDOCFLAGS="-D warnings -D missing-docs" cargo doc --workspace --no-deps --locked`
- [ ] `node --test tests/*.test.js`
- [ ] `cargo deny check`
- [ ] Relevant documentation, schemas, and fixtures are updated
