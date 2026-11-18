# Contributing

Bug reports and pull requests are welcome.

## Setup

You need Rust 1.89 or newer, plus bash and fish, because some tests run them.

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --no-default-features -- -D warnings
cargo test --locked
```

Tests that need bash or fish are ignored when that shell is missing. CI installs both
and runs them with `cargo test --locked -- --include-ignored`. If you install fish after
your first build, run `cargo clean -p rc-sort` so the fish tests stop being ignored.

## Guidelines

- Never lose a line. Any change that rewrites a user's file must back it up first.
- Every behavior change needs a test. Tests follow Arrange / Act / Assert.
- Commit messages follow Conventional Commits (`feat:`, `fix:`, `chore:`, ...).
