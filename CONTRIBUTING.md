# Contributing to Horcrux

Thank you for your interest in contributing to Horcrux! This document provides guidelines and instructions for contributing to the project.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Project Structure](#project-structure)
- [Development Workflow](#development-workflow)
- [Coding Standards](#coding-standards)
- [Testing](#testing)
- [Submitting Changes](#submitting-changes)
- [Documentation](#documentation)

## Code of Conduct

This project adheres to a code of conduct that all contributors are expected to follow:

- Be respectful and inclusive
- Welcome newcomers and help them learn
- Focus on constructive criticism
- Accept responsibility for mistakes
- Prioritize the community's best interests

## Getting Started

### Prerequisites

- **Rust 1.82+** - Install from [rustup.rs](https://rustup.rs/)
- **Git** - Version control
- **Linux** - Gentoo preferred, but any Linux distro works for development
- **QEMU/KVM** (optional) - For testing VM functionality
- **LXC/Docker** (optional) - For testing container functionality

### Quick Start

```bash
# Clone the repository
git clone https://github.com/CanuteTheGreat/horcrux.git
cd horcrux

# Build the project
cargo build

# Run tests
cargo test

# Run the API server (development mode)
cargo run --bin horcrux-api

# Run the CLI
cargo run --bin horcrux-cli -- --help
```

## Development Setup

### Install Dependencies

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install WASM target for UI development
rustup target add wasm32-unknown-unknown

# Install trunk for UI development
cargo install trunk

# Install development tools
cargo install cargo-watch    # Auto-rebuild on changes
cargo install cargo-edit      # Manage dependencies
cargo install cargo-audit     # Security audits
```

### IDE Setup

#### VS Code
Recommended extensions:
- `rust-analyzer` - Rust language support
- `CodeLLDB` - Debugging support
- `crates` - Dependency management
- `Even Better TOML` - TOML syntax highlighting

#### CLion/IntelliJ
- Install the Rust plugin
- Configure rust-analyzer

### Environment Configuration

Create a `.env` file in the project root (optional):

```env
RUST_LOG=debug
DATABASE_URL=sqlite://horcrux.db
API_HOST=127.0.0.1
API_PORT=8006
```

## Project Structure

```
horcrux/
├── horcrux-api/          # Main API server
│   ├── src/
│   │   ├── main.rs       # Server entry point
│   │   ├── vm/           # VM management
│   │   ├── container/    # Container management
│   │   ├── storage/      # Storage backends
│   │   ├── cluster/      # Clustering logic
│   │   ├── auth/         # Authentication
│   │   ├── middleware/   # Axum middleware
│   │   └── ...
│   ├── tests/            # Integration tests
│   ├── benches/          # Performance benchmarks
│   └── horcrux-ui/       # Web UI (Leptos/WASM)
├── horcrux-cli/          # Command-line interface
├── horcrux-common/       # Shared types and utilities
├── horcrux-mobile/       # Mobile app (future)
├── docs/                 # Documentation
├── deploy/               # Deployment configs
│   ├── systemd/          # systemd service files
│   └── openrc/           # OpenRC service files
└── gentoo/               # Gentoo ebuild files
```

## Development Workflow

### Branch Strategy

- `main` - Stable, production-ready code
- `develop` - Integration branch for features
- `feature/*` - New features
- `bugfix/*` - Bug fixes
- `hotfix/*` - Urgent fixes for production

### Creating a Feature Branch

```bash
# Update main
git checkout main
git pull origin main

# Create feature branch
git checkout -b feature/your-feature-name

# Make changes and commit
git add .
git commit -m "Add: Description of your changes"

# Push to remote
git push origin feature/your-feature-name
```

### Commit Message Format

Follow conventional commits format:

```
<type>: <description>

[optional body]

[optional footer]
```

**Types:**
- `feat` - New feature
- `fix` - Bug fix
- `docs` - Documentation only
- `style` - Code style changes (formatting, semicolons, etc.)
- `refactor` - Code restructuring without behavior changes
- `perf` - Performance improvements
- `test` - Adding or updating tests
- `chore` - Maintenance tasks

**Examples:**
```
feat: Add ZFS snapshot replication support

Implements incremental ZFS snapshot replication across cluster nodes
with bandwidth throttling and retention policy management.

Closes #123
```

```
fix: Resolve memory leak in VM migration

The migration process was not properly cleaning up QMP connections
after completion, leading to file descriptor exhaustion.
```

## Coding Standards

### Rust Style

- Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Use `rustfmt` for formatting: `cargo fmt`
- Use `clippy` for linting: `cargo clippy`
- Maximum line length: 100 characters
- Use descriptive variable names
- Add doc comments for public APIs

### Code Organization

```rust
// Module-level documentation
//! Brief description of the module
//!
//! Detailed explanation of what this module does
//! and how it fits into the larger system.

// Imports (grouped by: std, external crates, internal crates)
use std::collections::HashMap;
use tokio::process::Command;
use horcrux_common::Result;

// Type definitions
pub struct MyType {
    field: String,
}

// Implementations
impl MyType {
    /// Brief description
    ///
    /// # Arguments
    /// * `param` - Description
    ///
    /// # Returns
    /// Description of return value
    ///
    /// # Examples
    /// ```
    /// let instance = MyType::new("value");
    /// ```
    pub fn new(value: &str) -> Self {
        Self {
            field: value.to_string(),
        }
    }
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_my_type() {
        let instance = MyType::new("test");
        assert_eq!(instance.field, "test");
    }
}
```

### Error Handling

- Use `Result<T>` for fallible operations
- Provide context with error messages
- Use `?` operator for error propagation
- Log errors with `tracing` crate

```rust
use tracing::error;

pub async fn create_vm(config: &VmConfig) -> Result<String> {
    let vm_id = validate_config(config)?;

    let output = Command::new("qemu-system-x86_64")
        .args(&["--version"])
        .output()
        .await
        .map_err(|e| {
            error!("Failed to execute QEMU: {}", e);
            Error::System(format!("QEMU not available: {}", e))
        })?;

    if !output.status.success() {
        return Err(Error::System("QEMU check failed".to_string()));
    }

    Ok(vm_id)
}
```

### Async Code

- Use `tokio` runtime for async operations
- Prefer `async/await` over manual futures
- Use `tokio::spawn` for concurrent tasks
- Be mindful of blocking operations

```rust
// Good: Non-blocking async operations
pub async fn get_vm_stats(vm_id: &str) -> Result<VmStats> {
    let cpu = get_cpu_usage(vm_id).await?;
    let memory = get_memory_usage(vm_id).await?;

    Ok(VmStats { cpu, memory })
}

// Good: Spawn blocking for CPU-intensive work
pub async fn process_large_file(path: &str) -> Result<()> {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        // CPU-intensive work here
        std::fs::read_to_string(path)
    })
    .await
    .map_err(|e| Error::System(e.to_string()))?;

    Ok(())
}
```

## Testing

### Unit Tests

Place unit tests in the same file as the code being tested:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_config_validation() {
        let config = VmConfig {
            id: "100".to_string(),
            name: "test-vm".to_string(),
            // ... other fields
        };

        assert!(validate_config(&config).is_ok());
    }

    #[tokio::test]
    async fn test_async_operation() {
        let result = some_async_function().await;
        assert!(result.is_ok());
    }
}
```

### Integration Tests

Place integration tests in `tests/` directory:

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_vm_lifecycle

# Run tests with output
cargo test -- --nocapture

# Run tests in single thread
cargo test -- --test-threads=1
```

### Benchmarks

Add benchmarks to `benches/` directory using Criterion:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_my_function(c: &mut Criterion) {
    c.bench_function("my_function", |b| {
        b.iter(|| my_function(black_box(42)));
    });
}

criterion_group!(benches, benchmark_my_function);
criterion_main!(benches);
```

Run benchmarks:
```bash
cargo bench
```

## Submitting Changes

### Pull Request Process

1. **Fork the repository** and create your branch from `main`
2. **Make your changes** following the coding standards
3. **Add tests** for new functionality
4. **Update documentation** if needed
5. **Run tests**: `cargo test`
6. **Run linters**: `cargo clippy` and `cargo fmt`
7. **Commit your changes** with clear messages
8. **Push to your fork**
9. **Create a Pull Request**

### Pull Request Template

```markdown
## Description
Brief description of changes

## Type of Change
- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Documentation update

## Testing
- [ ] Unit tests added/updated
- [ ] Integration tests added/updated
- [ ] Manual testing performed

## Checklist
- [ ] Code follows project style guidelines
- [ ] Self-review completed
- [ ] Comments added for complex code
- [ ] Documentation updated
- [ ] No new warnings introduced
- [ ] Tests pass locally
```

### Code Review

All PRs require review before merging:

- At least one approval required
- All CI checks must pass
- No merge conflicts
- Up-to-date with target branch

## Documentation

### Code Documentation

```rust
/// Calculate VM resource allocation
///
/// This function determines optimal resource allocation based on
/// available node capacity and VM requirements.
///
/// # Arguments
/// * `vm_config` - VM configuration requirements
/// * `node_capacity` - Available node resources
///
/// # Returns
/// * `Ok(Allocation)` - Successful allocation
/// * `Err(Error)` - If resources are insufficient
///
/// # Examples
/// ```
/// let allocation = calculate_allocation(&vm_config, &node_capacity)?;
/// ```
pub fn calculate_allocation(
    vm_config: &VmConfig,
    node_capacity: &NodeCapacity,
) -> Result<Allocation> {
    // Implementation
}
```

### API Documentation

Update `docs/API.md` when adding/changing API endpoints:

```markdown
### Create VM
`POST /api/vms`

Creates a new virtual machine.

**Request Body:**
```json
{
  "name": "web-server",
  "cpus": 4,
  "memory": 8192,
  "disk_size": 50
}
```

**Response:**
```json
{
  "id": "vm-100",
  "status": "created"
}
```

**Errors:**
- `400` - Invalid configuration
- `409` - VM already exists
- `500` - Creation failed
```

### User Documentation

Add user-facing docs to `docs/` directory:
- Feature guides
- Configuration examples
- Troubleshooting tips
- Architecture diagrams

## Getting Help

- **Issues** - [GitHub Issues](https://github.com/CanuteTheGreat/horcrux/issues)
- **Discussions** - [GitHub Discussions](https://github.com/CanuteTheGreat/horcrux/discussions)
- **Documentation** - [docs/](./docs/)

## Recognition

Contributors will be recognized in:
- `CONTRIBUTORS.md` file
- Release notes
- Project README

## License

By contributing, you agree that your contributions will be licensed under the GNU General Public License v3.0.

---

**Thank you for contributing to Horcrux!** 🎉
