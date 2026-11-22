# Contributing to Ferris

Thank you for your interest in contributing to Ferris! This document provides guidelines and instructions for contributing.

## Code of Conduct

Please read and follow our [Code of Conduct](CODE_OF_CONDUCT.md) to ensure a welcoming environment for all contributors.

## Getting Started

1. Fork the repository
2. Clone your fork and its sibling: `FerrisFramework` references
   `../FerrisCore` by path, so the two checkouts must sit side by side:

   ```bash
   git clone https://github.com/<you>/FerrisFramework.git
   git clone https://github.com/<you>/FerrisCore.git
   ```

3. Create a new branch: `git checkout -b feature/your-feature-name`
4. Make your changes
5. Run the gates inside the development container:
   `docker compose run --rm ferris cargo fmt --all -- --check` and
   `docker compose run --rm ferris cargo test --workspace -j 2 -- --test-threads=1`
6. Commit your changes: `git commit -m "feat: add your feature"`
7. Push to your fork: `git push origin feature/your-feature-name`
8. Open a Pull Request

## Development Guidelines

### Environment

See [AGENTS.md](AGENTS.md) for the canonical development environment setup. The
repository ships a pinned Docker image (`Dockerfile` + `compose.yaml`) and a
devcontainer; all builds, tests and CLI commands run there, not with the host
toolchain. Running cargo on the host invalidates the shared cache and produces
artifacts the container cannot reuse.

### Documentation

The public documentation is the separate [`FerrisDocs`](https://github.com/OpenFerris/FerrisDocs)
repository (Astro + Starlight). It is written in Spanish, but code, identifiers
and commands keep their real spelling. Every page has `title`/`description`
frontmatter, lives under `src/content/docs/`, and must be linked from the
explicit `sidebar` in `astro.config.mjs`.

```bash
git clone https://github.com/OpenFerris/FerrisDocs.git
cd FerrisDocs
npm ci            # installs the pinned package-lock.json
npm run dev       # http://localhost:4321
npm run build     # validate the static build
```

If a change alters a public command, flag, endpoint, config key, DocType,
fieldtype, WIT signature or feature gate, update the matching page in
`FerrisDocs` in the same change. That repository's `AGENTS.md` has the full
rules.

### Code Style

- Follow Rust's standard formatting: `cargo fmt --all`
- Lint your crate: `cargo clippy -p <crate> --all-targets` (the workspace has a
  historical pedantic/nursery baseline; CI gates on `clippy::correctness`, and a
  PR must not add warnings to the crate it touches)
- Write tests for new functionality
- Keep changes consistent with the existing codebase patterns
- The workspace is memory-bound: run tests with `-j 2 -- --test-threads=1`,
  never at full parallelism

### Commit Messages

We follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:

- `feat:` - New feature
- `fix:` - Bug fix
- `docs:` - Documentation changes
- `style:` - Code style changes (formatting, etc.)
- `refactor:` - Code refactoring
- `test:` - Adding or updating tests
- `chore:` - Maintenance tasks

### Testing

- All tests must pass: `cargo test --workspace -j 2 -- --test-threads=1`
- Add unit tests for new functionality
- Add integration tests where appropriate
- Do not modify tests to make them pass unless the test is incorrect

## Pull Request Process

1. Ensure your code builds without warnings
2. Update documentation if you're changing APIs
3. Add tests for new features
4. Ensure all CI checks pass
5. Request review from maintainers

## License

By contributing to Ferris, you agree that your contributions will be licensed under the MIT License.
