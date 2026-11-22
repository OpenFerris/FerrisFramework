# FerrisFramework

Fachada pública de Ferris: Desk, verticales opcionales, host WASM, servidor y
CLI. Depende de **FerrisCore** (9 crates base), que debe estar en el directorio
hermano `../FerrisCore`.

```bash
cargo check --all-targets          # Desk + verticales ligeras
cargo check-cli                    # server + CLI + wasm + verticales pesadas
cargo test --workspace -j 2 -- --test-threads=1
```

La documentación pública vive en el repositorio `../FerrisDocs`.
