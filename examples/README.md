# Axon Examples

This directory contains ready-to-run sample configurations and scripts for common scenarios. Each config can be validated and served by Axon, and each script provides a quick smoke test to verify behavior.

## Structure

- configs/: Configuration files for each scenario
- scripts/: Shell scripts to run and test each scenario

## How to use

- Validate config:
  axon validate --config examples/configs/SCENARIO.yaml
- Run server (foreground):
  axon serve --config examples/configs/SCENARIO.yaml
- Run scripted smoke test:
  examples/scripts/SCENARIO.sh

Note: Replace `axon` with `cargo run --` if you haven’t installed the binary.
