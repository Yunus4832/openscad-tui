<!-- OPENSPEC:START -->
# OpenSpec Instructions

These instructions are for AI assistants working in this project.

Always open `@/openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan)
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work
- Sounds ambiguous and you need the authoritative spec before coding

Use `@/openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

Keep this managed block so 'openspec update' can refresh the instructions.

<!-- OPENSPEC:END -->

# Agent Instructions for OpenSCAD TUI

This document provides essential information for AI agents working on the OpenSCAD TUI project, a Rust workspace for building a terminal user interface for OpenSCAD.

## Project Overview

OpenSCAD TUI is a terminal-based interface for creating OpenSCAD code with Vim-like interactions. The project is organized as a Cargo workspace with three crates:

- `openscad-core`: Core AST definitions and operations
- `openscad-library`: Library management and module definitions
- `openscad-ui`: TUI application and command handling

## Build Commands

```bash
# Build all crates in debug mode
cargo build

# Build specific crate
cargo build -p openscad-core
cargo build -p openscad-library
cargo build -p openscad-ui

# Build release binary
cargo build --release

# Run the main TUI application
cargo run --bin openscad-tui

# Build all workspace members
cargo build --workspace
```

## Lint and Format Commands

```bash
# Format all code
cargo fmt --all

# Check formatting without applying
cargo fmt --all -- --check

# Run clippy on all crates
cargo clippy --workspace -- -D warnings

# Check compilation without building
cargo check --workspace
```

## Test Commands

```bash
# Run all tests for all crates
cargo test --workspace

# Run tests for specific crate
cargo test -p openscad-core

# Run a specific test
cargo test -p openscad-core test_expr_parse_integer

# Run tests with detailed output
cargo test -- --nocapture
```

Tests are located in `#[cfg(test)]` modules within each source file. Use descriptive test names like `test_function_name_scenario`.

## Code Style Guidelines

### Import Conventions
- Use `use` statements grouped by external crates, standard library, internal modules
- Keep imports at top of file, after module comment
- Avoid wildcard imports except in test modules

### Naming Conventions
- **Modules**: snake_case (`core`, `library`, `ui`)
- **Structs/Enums**: PascalCase (`ModuleNode`, `AstRoot`, `CommandError`)
- **Functions/Methods**: snake_case (`find_node_by_id`, `to_scad`, `add_module`)
- **Variables**: snake_case (`input_buffer`, `selected_nodes`, `undo_stack`)
- **Type parameters**: single uppercase letter (`T`, `E`)
- **Lifetimes**: lowercase single letter (`'a`, `'static`)

### Error Handling
- Use `thiserror` crate for defining error types
- Define error enums with `#[derive(Error, Debug)]`
- Use `#[error("...")]` for error messages
- Propagate errors using `?` operator
- Define `Result<T, E>` aliases in each module

### Serialization
- Use `serde` with `Serialize` and `Deserialize` derives
- Prefer `serde_json` for JSON serialization
- Derive traits on enums and structs: `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]`

### Formatting Rules
- Indentation: 4 spaces (no tabs)
- Line length: aim for 100 characters
- Braces: same line for structs/enums/functions, new line for match arms
- Trailing commas: use in multi-line structs/enums/arrays

### Type Usage
- Prefer `Option<T>` for nullable values
- Use `Result<T, E>` for fallible operations
- Use `Vec<T>` for dynamic arrays
- Use `String` for owned strings, `&str` for borrowed strings
- Use `Box<T>` for heap allocation when needed

### Pattern Matching
- Use exhaustive match patterns
- Handle all enum variants
- Use `if let` for single-case pattern matching

## Project Structure

- Each crate has a `src/lib.rs` defining public API
- Binary crates have `src/main.rs` as entry point
- Split large modules into multiple files (e.g., `app.rs`, `commands.rs`, `ui.rs`)
- Use `pub` for items accessible outside the crate, `pub(crate)` for internal visibility
- Shared dependencies defined in workspace `Cargo.toml`
- Crate-specific dependencies in each crate's `Cargo.toml`
- Use path dependencies for local crates: `openscad-core = { path = "../core" }`

## Testing Guidelines

- Place tests in `#[cfg(test)]` modules within same file
- Test public API and important internal functions
- Use descriptive test names: `test_function_name_scenario`
- Include both success and failure cases
- Create minimal test fixtures
- Use `assert_eq!` for expected values
- Test serialization round-trips (serialize then deserialize)

## Development Workflow

1. Run `cargo fmt --all` to ensure consistent formatting
2. Run `cargo clippy --workspace -- -D warnings` to catch lint issues
3. Run `cargo test --workspace` to ensure all tests pass
4. Run `cargo check --workspace` for compilation errors
5. Follow existing patterns in the codebase
6. Add tests for new functionality
7. Update documentation if needed

## Notes for AI Agents

- This is a Rust 2021 edition project
- All crates share the same version (0.1.0)
- The project uses a workspace resolver ("2")
- No Cursor or Copilot-specific rules are present
- Follow existing patterns rather than introducing new styles
- When in doubt, look at similar code in the codebase for guidance