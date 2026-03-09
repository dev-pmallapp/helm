# Testing Guide

HELM follows strict Test-Driven Development (TDD) with red-green-refactor cycles. This guide covers running tests, adding tests, and testing best practices.

## Running Tests

### Rust Tests

- Run all: `make test`
- Individual crate: `cargo test -p <crate-name>`
- Tests are in `crates/<crate>/src/tests/`, wired via `#[cfg(test)] mod tests;` in `lib.rs`.

### Python Tests

- Run all: `make test-python`
- Uses `unittest` in `python/tests/`.
- Example: `python -m unittest discover python/tests/`

### Pre-commit Testing

- `make pre-commit`: Runs `fmt-check`, `clippy`, and `test` (Rust only).
- Must pass before commits.

## Test Organization

- **Rust**: One test file per source module (e.g., `src/tests/rob.rs` for `src/rob.rs`).
- **Python**: `test_*.py` files in `python/tests/`.
- No test code in production files.

## Adding Tests

### Rust Tests

1. Add to `src/tests/<module>.rs`.
2. Use `#[test]` functions.
3. Test public APIs and internal logic.

Example:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_example() {
        // Test code
    }
}
```

### Python Tests

1. Create `test_*.py` in `python/tests/`.
2. Use `unittest.TestCase`.

Example:
```python
import unittest
from helm import Core

class TestCore(unittest.TestCase):
    def test_core_config(self):
        core = Core()
        # Assertions
```

## Test Coverage

- Unit tests for individual components.
- Integration tests for end-to-end flows (e.g., SE execution).
- Examples as integration tests.

## Best Practices

- Write tests before code (TDD).
- Test failure cases and edge cases.
- Use descriptive names.
- Keep tests fast and isolated.
- Run tests frequently during development.

## CI/CD

- Pre-commit hooks ensure tests pass.
- No merging without green tests.