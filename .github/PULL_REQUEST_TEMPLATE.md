## Summary

<!-- What does this change and why? -->

## Linked issue

<!-- Closes #<n> -->

## Checklist

- [ ] Targets the `development` branch (one concern per pull request)
- [ ] Tests added or updated; no assertion weakened
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` pass
- [ ] `pnpm lint`, `pnpm typecheck` and `pnpm test` pass
- [ ] Generated TypeScript bindings regenerated if IPC changed (`src/ipc/bindings.ts`)
- [ ] Business logic stays in Rust; the UI change only renders or holds UI state
- [ ] Any behaviour change has an entry in `CHANGELOG.md` and a test
- [ ] No secrets, tokens, DSNs, local paths or real personal data in code, fixtures or logs
- [ ] Documentation and `CHANGELOG.md` updated for user-visible changes
- [ ] One concern per commit; commit messages carry no tool or assistant trailers
