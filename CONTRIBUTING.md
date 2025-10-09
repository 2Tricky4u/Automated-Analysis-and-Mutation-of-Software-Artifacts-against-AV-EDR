# Contributing to EDR Lab

Thank you for your interest in contributing to the EDR Lab project!

## Code of Conduct

This project is for educational and research purposes only. All contributors must agree to use this software responsibly and ethically.

## How to Contribute

### Reporting Issues

- Use the GitHub issue tracker
- Provide detailed information about the problem
- Include system information and reproduction steps
- For security issues, follow responsible disclosure practices

### Submitting Changes

1. **Fork the repository**
   ```bash
   git clone https://github.com/2Tricky4u/Automated-Analysis-and-Mutation-of-Software-Artifacts-against-AV-EDR.git
   ```

2. **Create a feature branch**
   ```bash
   git checkout -b feature/my-new-feature
   ```

3. **Make your changes**
   - Follow the coding style guidelines
   - Add tests for new functionality
   - Update documentation as needed

4. **Test your changes**
   ```bash
   cargo test --workspace
   cargo fmt --all
   cargo clippy --workspace -- -D warnings
   ```

5. **Commit your changes**
   ```bash
   git commit -m "Add feature: brief description"
   ```

6. **Push to your fork**
   ```bash
   git push origin feature/my-new-feature
   ```

7. **Submit a pull request**
   - Provide a clear description of the changes
   - Reference any related issues
   - Ensure all tests pass

## Development Setup

### Prerequisites

- Rust 1.75+
- Docker and Docker Compose
- CMake (for ETW consumer)
- protobuf compiler

### Building

```bash
cargo build
```

### Running Tests

```bash
cargo test --workspace
```

### Code Formatting

```bash
cargo fmt --all
```

### Linting

```bash
cargo clippy --workspace -- -D warnings
```

## Coding Guidelines

### Rust Code

- Follow Rust naming conventions
- Use `cargo fmt` for formatting
- Address all `clippy` warnings
- Add documentation comments for public APIs
- Write tests for new functionality

### C++ Code

- Follow modern C++ best practices (C++17+)
- Use consistent indentation (4 spaces)
- Add comments for complex logic
- Handle errors appropriately

### Documentation

- Keep README.md up to date
- Document new features in docs/
- Add examples to docs/EXAMPLES.md
- Update architecture diagrams if needed

## Project Structure

```
.
├── controller/       # Controller services
├── worker/          # Worker services
├── build/           # Build system and Docker
├── telemetry/       # ETW consumer and Filebeat
├── ui/              # Kibana dashboards
└── docs/            # Documentation
```

## Testing Guidelines

### Unit Tests

- Write unit tests for all new functions
- Test edge cases and error conditions
- Use descriptive test names

Example:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let msg = HarnessMessage::new(MessageType::Start);
        assert_eq!(msg.message_type, MessageType::Start);
    }
}
```

### Integration Tests

- Test interactions between components
- Use Docker Compose for integration testing
- Verify gRPC service communication

## Documentation

### Code Documentation

Use Rust doc comments:
```rust
/// Schedules a new analysis job
///
/// # Arguments
/// * `request` - The job request containing configuration
///
/// # Returns
/// A response with the job ID and status
pub async fn schedule_job(&self, request: Request<JobRequest>) 
    -> Result<Response<JobResponse>, Status>
```

### Architecture Documentation

- Update docs/ARCHITECTURE.md for architectural changes
- Add diagrams for complex flows
- Document design decisions

## Performance Considerations

- Profile code for performance bottlenecks
- Optimize hot paths
- Consider resource usage (memory, CPU)
- Use async/await efficiently

## Security Considerations

- Never commit secrets or credentials
- Validate all inputs
- Use secure defaults
- Follow least privilege principle
- Document security implications

## Review Process

1. Code reviews are required for all PRs
2. At least one maintainer must approve
3. All tests must pass
4. Documentation must be updated
5. No unresolved discussions

## Communication

- Use GitHub issues for bugs and features
- Keep discussions focused and respectful
- Be patient with review process

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

## Questions?

Open an issue or discussion on GitHub if you have questions about contributing.

Thank you for contributing to EDR Lab!
