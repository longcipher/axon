format:
  taplo fmt
  cargo +nightly fmt --all
lint:
  taplo fmt --check
  cargo +nightly fmt --all -- --check
  cargo +nightly clippy --all -- -D warnings -A clippy::derive_partial_eq_without_eq -D clippy::unwrap_used -D clippy::uninlined_format_args
  cargo machete
test:
  cargo test

# Run a specific example script by name (without .sh)
example-run name:
  examples/scripts/{{name}}.sh

# Run all example scripts sequentially
examples-run-all:
  examples/scripts/run_all.sh

# Validate all example configs
examples-validate:
  for f in examples/configs/*.toml; do \
    echo "Validating $f"; \
    cargo run -- validate --config "$f" || exit 1; \
  done