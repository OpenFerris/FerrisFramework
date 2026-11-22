# FerrisFramework

Fachada pública de Ferris: **12 crates** con el Desk, las verticales
opcionales, el host WASM, el servidor y la CLI.

Depende de [`FerrisCore`](https://github.com/OpenFerris/FerrisCore) (9 crates
base), que debe estar en el directorio hermano `../FerrisCore`:

```text
ferris/
├── FerrisCore/
└── FerrisFramework/
```

## Compilar

Todo se ejecuta dentro del contenedor de este repositorio; `compose.yaml` monta
`../FerrisCore` en `/FerrisCore` para que las rutas Cargo resuelvan.

```bash
docker compose run --rm ferris cargo check --all-targets   # Desk + verticales ligeras
docker compose run --rm ferris cargo check-cli             # server + CLI + wasm + verticales pesadas
docker compose run --rm ferris cargo test --workspace -j 2 -- --test-threads=1
```

## Enlaces

- Documentación: <https://openferris.github.io/FerrisDocs/>
- Repositorio del sitio: <https://github.com/OpenFerris/FerrisDocs>
- Reglas de desarrollo: [`AGENTS.md`](AGENTS.md)
- Política de versiones: [`VERSIONING.md`](VERSIONING.md)

## Ramas

`develop` es la integración (rama por defecto) y `master` es producción. Los
PR de trabajo van contra `develop`; `release/*` y `hotfix/*` son las únicas
ramas que se fusionan en `master`.
