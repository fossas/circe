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
- **Variable Shadowing**: Prefer shadowing variables rather than using Hungarian notation (e.g., use `let path = path.to_string_lossy()` instead of `let path_str = path.to_string_lossy()`)
- **Imports**: Group std lib, external crates, internal modules (alphabetically)
- **Error Handling**: Use color-eyre with context(), ensure!(), bail!()
- **Types**: Prefer Builder pattern, derive common traits, use strong types
- **Documentation**: Comments explain "why" not "what", use proper sentences. Avoid redundant comments that merely describe what code does - good code should be self-explanatory
- **Organization**: Modular approach, named module files (not mod.rs)
- **Testing**: Add integration tests in tests/it/, use test_case macro
- **Functional Style**: Avoid mutation, prefer functional patterns when possible
- **Cargo**: Never edit Cargo.toml directly, use cargo edit commands
- **Conversions**: Use Type::from(value) not let x: Type = value.into()
- **String Formatting**: 
  - For simple variables, use direct interpolation: `"Value: {variable}"` instead of `"Value: {}", variable`
  - For expressions (method calls, etc.), use traditional formatting: `"Value: {}", expression.method()`
  - This project enforces `clippy::uninlined_format_args` for simple variables

Set `RUST_LOG=debug` or `RUST_LOG=trace` for detailed logs during development.