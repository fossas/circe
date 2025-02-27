# Circe Project Guidelines

## Build/Test Commands
```bash
# Build project
cargo build

# Lint code
cargo fmt --all -- --check
cargo clippy --all-features --all --tests -- -D clippy::correctness

# Run all tests
cargo nextest run --all-targets
cargo test --doc

# Run single test
cargo nextest run test_name
cargo nextest run --package circe_lib path::to::module
```

## Code Style Guidelines
- **Formatting**: Use rustfmt, consistent with surrounding code
- **Naming**: snake_case for functions/variables, CamelCase for types
- **Imports**: Group std lib, external crates, internal modules (alphabetically)
- **Error Handling**: Use color-eyre with context(), ensure!(), bail!()
- **Types**: Prefer Builder pattern, derive common traits, use strong types
- **Documentation**: Comments explain "why" not "what", use proper sentences
- **Organization**: Modular approach, named module files (not mod.rs)
- **Testing**: Add integration tests in tests/it/, use test_case macro
- **Functional Style**: Avoid mutation, prefer functional patterns when possible
- **Cargo**: Never edit Cargo.toml directly, use cargo edit commands
- **Conversions**: Use Type::from(value) not let x: Type = value.into()

Set `RUST_LOG=debug` or `RUST_LOG=trace` for detailed logs during development.