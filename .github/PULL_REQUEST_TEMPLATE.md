<!-- Changing the release process, the C ABI, the shared binding surface or a
     venue's signing? There is a longer template that asks what such a change
     has to answer: reopen this pull request with ?template=detailed.md
     appended to the URL. GitHub offers no picker for a second template, so it
     is only reachable that way. -->

<!-- Keep it short. One logical change per PR. -->

## What

<!-- What does this change and why? -->

## Checklist

- [ ] `cargo fmt --all` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` are clean
- [ ] `cargo test --workspace --all-features` passes
- [ ] Tests added/updated (prefer hand-computed expectations for engine changes)
- [ ] No look-ahead bias introduced into the fill model
- [ ] `CHANGELOG.md` updated under `[Unreleased]`
