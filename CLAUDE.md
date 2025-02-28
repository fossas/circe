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

## Code Style Guidelines and Patterns
- **Formatting**: Use rustfmt, consistent with surrounding code
- **Naming**: 
  - snake_case for functions/variables, CamelCase for types
  - Prefer terse function names (e.g., use `image_source` instead of `create_image_source`)
  - Avoid Hungarian notation (e.g., use `MultiImageSource` instead of `ImageSourceEnum`)
  - Name types based on their purpose, not their implementation
- **Imports**: Group std lib, external crates, internal modules (alphabetically)
- **Error Handling**: Use color-eyre with context(), ensure!(), bail!()
- **Types**:
  - Prefer Builder pattern, derive common traits, use strong types
  - Avoid dynamic dispatch with trait objects (`dyn TraitName`). Instead, prefer enum-based approaches
  - Use concrete types when possible
  - Avoid using `Box<dyn Trait>` in public APIs
- **Documentation**: 
  - Comments explain "why" not "what", use proper sentences
  - Avoid useless comments that just restate the code (e.g., "This function checks if the image exists")
- **Organization**: Modular approach, named module files (not mod.rs)
- **Testing**: 
  - Add integration tests in tests/it/, use test_case macro
  - Ensure tests are resilient to different environments (e.g., handle missing Docker daemon gracefully)
- **Functional Style**: Avoid mutation, prefer functional patterns when possible
- **Cargo**: Never edit Cargo.toml directly, use cargo edit commands
- **Conversions**: Use Type::from(value) not let x: Type = value.into()

Set `RUST_LOG=debug` or `RUST_LOG=trace` for detailed logs during development.

## Examples

### Using an enum instead of dynamic dispatch (Box<dyn Trait>)

```rust
// BAD: Uses dynamic dispatch with trait objects
pub async fn some_factory() -> Result<Box<dyn SomeTrait>> {
    if condition {
        Ok(Box::new(ConcreteTypeA::new()))
    } else {
        Ok(Box::new(ConcreteTypeB::new()))
    }
}

// GOOD: Uses an enum-based approach
pub enum SomeTraitImpl {
    TypeA(ConcreteTypeA),
    TypeB(ConcreteTypeB), 
}

#[async_trait::async_trait]
impl SomeTrait for SomeTraitImpl {
    async fn method(&self) -> Result<()> {
        match self {
            Self::TypeA(a) => a.method().await,
            Self::TypeB(b) => b.method().await,
        }
    }
}

pub async fn some_factory() -> Result<SomeTraitImpl> {
    if condition {
        Ok(SomeTraitImpl::TypeA(ConcreteTypeA::new()))
    } else {
        Ok(SomeTraitImpl::TypeB(ConcreteTypeB::new()))
    }
}
```

### Meaningful function naming

```rust
// BAD: Redundant prefix
pub fn create_registry() -> Registry { ... }

// GOOD: Terse, meaningful name
pub fn registry() -> Registry { ... }
```

### Comments that add value

```rust
// BAD: Comment that just repeats what the code does
// Check if the image exists
if daemon.image_exists().await? { ... }

// GOOD: Comment that explains why or context
// Try to use the daemon first for better performance
if daemon.image_exists().await? { ... }
```