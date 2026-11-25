# ADR 0002 — Extraer el Desk a FerrisDesk

Estado: **propuesta** (verificación de viabilidad + plan de ejecución).
Ámbito: `ferris-web`, `ferris-server`, `ferris-cli`,
`FerrisCore::{ferris-web-ui, ferris-web-shared}`, CI y documentación.
Fecha: 2026-11-25.

---

## 1. Veredicto

La visión es correcta en el diagnóstico y está mal dimensionada en el reparto de
esfuerzo. El 80 % del beneficio arquitectónico está en la **Fase 1** (un núcleo
puro con bucle comando → estado); el 80 % del coste está en la **Fase 2**
(reescribir ~79 000 líneas de UI funcionando).

| Fase | Viabilidad | Coste | Veredicto |
|---|---|---|---|
| 1. `desk-core` + `desk-ports` | **Alta** | M | Adelante. Es donde está el valor. |
| 2. UI web ligera (HTML/CSS/JS o Web Components) | Técnicamente sí | **XL** | No como está planteada. Reescribir la UI no aporta separación: la aporta la Fase 1. |
| 3. Tauri v2 + `adapter-native` | **Alta** si es *sidecar*; baja si se reimplementa el almacenamiento | M | Adelante, pero con el motor Rust dentro, no con un SQLite paralelo. |
| 4. Offline-first, Web Components, UniFFI | **Media** | M | La mitad ya existe (Web Components). Solo el offline es trabajo nuevo. |

Tres correcciones de fondo:

1. **El motor de metadatos ya no está acoplado a la UI.** `ferris-schema::contract`
   (4 910 LOC) y `ferris-web-shared::meta` son serde puro y ya los compilan las
   dos mitades. Lo acoplado es el **estado de vista** (`RwSignal` dentro de
   `FormField`), no el motor.
2. **El bucle no es `UiCommand → ViewState`, es `UiCommand → (State, Effects)`.**
   Guardar, listar, navegar y persistir son E/S. Un reductor que solo devuelve
   estado obliga a meter E/S en el core o a fingir que no existe. El core emite
   **efectos**; los adaptadores los ejecutan.
3. **Los permisos no se calculan en el cliente.** El servidor ya es la
   autoridad. El core *proyecta* lo que el servidor decidió (campos read-only,
   sin botón de crear, acciones disponibles). Si el core "gestiona permisos",
   se crea una segunda fuente de verdad y una falsa sensación de seguridad.

---

## 2. Estado actual verificado

### 2.1 Inventario

| Crate | Dónde | Archivos `.rs` | LOC | Qué es |
|---|---|---|---|---|
| `ferris-web` | FerrisFramework | 229 | 57 331 | Desk Leptos (SSR + hydrate) **y además** el sitio público y el catálogo |
| `ferris-web-ui` | FerrisCore | 159 | 20 496 | ~90 primitivas Leptos, hooks, catálogo de clases |
| `ferris-web-shared` | FerrisCore | 21 | 3 389 | Contrato de ayuda: meta, tipos, i18n, html, país, fechas |
| `ferris-schema::contract` | FerrisCore | — | 4 910 | Contrato serde puro (meta, lista, desk, layout, ui, estilo) |

Dentro de `ferris-web` conviven cuatro cosas distintas:

| Parte | LOC aprox. | Naturaleza |
|---|---|---|
| Desk (views, desk, app, components, ext, api) | ~55 000 | UI Leptos |
| `src/website/` + `src/markdown/` | ~1 966 | **Renderizador HTML del sitio público** (sin Leptos) |
| `src/doctypes.rs` + `src/workspaces.rs` | 394 | Declaraciones de catálogo del servidor |
| `DESK_CSS`, `vendor_assets`, `preview_assets`, `i18n` | — | Activos y contexto compartidos |

Esta mezcla es el hallazgo más importante: sacar "el frontend" tal cual se lleva
por delante el sitio público, que hoy sirve `ferris serve` en
`ferris-server/src/router/pages.rs:29` y `:217`, y las declaraciones de las
DocTypes del CMS (`ferris-server/src/catalog.rs:38-39`, `:84-85`). **La
extracción es una partición en tres, no en dos.**

### 2.2 Lo que el plan cree tener que construir y ya existe

| Pieza del plan | Estado real | Evidencia |
|---|---|---|
| Contrato de metadatos serializable | Hecho | `ferris-schema/src/contract/` (4 910 LOC, solo serde) |
| Reglas de campos (`depends_on`) | Hecho, pero dentro de la UI | `views/services/form_service/condition.rs:90,108` |
| Reglas obligatorio / read-only dependientes | **Declaradas y no implementadas** | `DynField.mandatory_depends_on`, `read_only_depends_on`; solo se leen en el editor (`edit_mode/schema.rs:60-62`) |
| Columnas, filtros, vistas guardadas | Hecho y compartido | `ferris-web-shared/src/meta/{columns,filters,view}.rs` |
| Puertos de infraestructura | **A medias, sin traits** | `api/store.rs:26-29` (`enum DocStore { Server, Client }`) |
| Adaptador de navegador (fetch, localStorage, SSE) | Hecho y en uso | `api/client/`, `desk/notifications.rs:157` |
| API REST completa | Hecho, en Core | `ferris-api/src/router/routes.rs:122-168` (36 entradas) |
| Login JSON + cookie + CSRF | Hecho | `ferris-api/src/session.rs:228`; `ferris-auth/src/middleware.rs:257` |
| Extensiones en Web Components (slots, shadow DOM, assets) | **Hecho** | `ferris-web/src/ext/mod.rs:1-40`, `ext/mount.rs` |
| Hooks `before_save` / `validate` / `on_change` | **Hecho, en servidor** | `ferris-core/src/hooks/point.rs:90-131`; WIT en `ferris-wasm/wit/ferris.wit` |
| i18n con catálogos servidos por API | Hecho | `ferris-core/src/i18n/{en,es}.*.toml`, `GET /api/i18n` |
| SQLite real con FTS5 y dialecto | Hecho | `ferris-db/src/driver.rs:24-25,311-329`; `ferris-schema/src/search.rs:161` |

Consecuencia: las fases 2 y 4 del plan original proponen construir un sistema de
extensiones en JS y un "puente de módulos especializados" que **ya existen** con
otra forma. Duplicarlos crea un tercer mecanismo de extensión con semántica
distinta (y peor: sin el sandbox del host WASM ni la validación del servidor).

### 2.3 Lo que sí está acoplado y justifica el trabajo

| Acoplamiento | Evidencia | Coste de romperlo |
|---|---|---|
| El modelo de formulario **es** reactivo | `FormField { value: RwSignal<String>, checked: RwSignal<bool>, error: RwSignal<Option<String>> }` en `form_service/layout.rs:49-51` | Medio: hay que separar dato de señal |
| Validación y payload leen señales y traducen con `use_i18n_value()` | `layout.rs:452-458`, `payload.rs:9` | Medio: el core debe devolver claves, no frases |
| Servicios con `spawn_local` + `use_navigate` | `form_service/mod.rs:35-37`, `writes.rs:21-23`, `view_core.rs:8-9` | Medio: la navegación pasa a ser un efecto |
| `DocStore` es un `enum` concreto, no un puerto | `api/store.rs:26-29` | Bajo |
| La feature `web` mezcla UI y DocTypes del CMS | `ferris-server/Cargo.toml:36-43`; `catalog.rs:38` | Bajo, pero bloquea la extracción |
| `ferris-web-ui` y `ferris-web-shared` son Desk viviendo en Core | Solo los usa `ferris-web` (`grep` de `ferris-web-shared` en Core y Framework) | Bajo (mover), medio (versionar) |
| La CLI asume `ferris-web/pkg` y cargo-leptos | `ferris-cli/src/commands/dev/hydrate.rs:100-140` | Bajo |
| SSR acoplado a `leptos_axum` | `ferris-server/src/router/build.rs:633` | Medio: hay que decidir cómo se sirve la SPA |

### 2.4 Deuda que el refactor saldaría (y que hoy es invisible)

- `form_service/layout.rs:1-7` afirma "sin señales, sin I/O y sin `RwSignal`" y
  la línea 49 tiene tres `RwSignal`. La documentación miente porque el módulo no
  tiene una frontera real.
- `mandatory_depends_on`, `read_only_depends_on` y `collapsible_depends_on`
  están en el contrato y en el constructor de DocTypes, y el renderizador los
  ignora. Un DocType puede declarar comportamiento que no ocurre.
- `ferris-web/pkg/ferris-web_bg.wasm` (artefacto de desarrollo, ignorado por
  git) pesa **313 MB**. La propia CLI avisa de ~145 MiB en debug
  (`hydrate.rs`). Es un síntoma del tamaño del árbol, no del contrato.

---

## 3. Viabilidad, fase por fase

### 3.1 Fase 1 — núcleo puro: viable y es el objetivo real

El núcleo puede ser **síncrono y sin runtime**. Eso es lo que hace que
`cargo test` corra en milisegundos y que compile para `wasm32` sin `tokio`.

```rust
// desk-core: sin async, sin I/O, sin leptos, sin web-sys.
pub enum FormCommand {
    Loaded { meta: DynMeta, doc: Value },
    SetField { field: String, value: FieldValue },
    SetChildField { table: String, row: usize, field: String, value: FieldValue },
    Blur { field: String },
    Save,
    Duplicate,
    Discard,
}

pub enum Effect {
    Fetch { request: ApiRequest },
    Save { doctype: String, name: Option<String>, payload: Value, verb: WriteVerb },
    Navigate { to: String, replace: bool },
    Persist { key: String, json: String },
    Notify { level: Level, message: MessageKey },
    Log { level: Level, message: String },
}

// `state` es la única fuente de verdad; `ViewState` es una proyección
// prestada, no un segundo estado serializado.
pub fn reduce(state: &mut FormState, cmd: FormCommand) -> Vec<Effect>;
```

Alcance concreto (LOC actuales a mover, no a reescribir):

| Módulo actual | LOC | Destino |
|---|---|---|
| `form_service/layout.rs` | 623 | `desk-core::form::layout` (datos, no señales) |
| `form_service/payload.rs` | 204 | `desk-core::form::payload` |
| `form_service/condition.rs` | 503 | `desk-core::rules` (evaluador puro, ya lo es) |
| `form_service/layout_ops.rs` + `layout_history.rs` | 476 | `desk-core::form::undo` |
| `form_service/{mod,writes,methods}.rs` | 1 247 | `desk-core::form` (reductor) + efectos |
| `list_service.rs` | 604 | `desk-core::list` |
| `view_core.rs` + `view_core/` | 900 | `desk-core::query` |
| `ferris-web-shared/src/meta` | 656 | `desk-core::meta` |

Total: ~5 200 LOC de lógica real. El resto de `ferris-web` es presentación.

**Decisiones de diseño que el plan original no cubre y hay que cerrar:**

- **Tablas hijas.** `Table` es un campo con filas; `FormCommand` necesita
  identidad de fila (`row: usize` se rompe al reordenar/borrar). Usar `row_id`
  estable, no índice.
- **`fetch_from` / `fetch_if_empty`** disparan E/S → efecto, nunca dentro del
  reductor.
- **Mensajes de validación.** El core devuelve `MessageKey` (p. ej.
  `components.form.validation.required`), no texto. El adaptador traduce con el
  catálogo de `/api/i18n`. Si el core traduce, deja de ser puro y depende del
  idioma del proceso.
- **Permisos.** El core solo proyecta: `can_create`, `can_submit`,
  `read_only_depends_on`. La autorización sigue en `ferris-api`.
- **`serde` en la frontera, no en el interior.** El core expone tipos Rust; los
  `Serialize` viven en los DTO del contrato. Serializar estado en cada comando
  es coste sin beneficio cuando UI y core comparten proceso (Leptos/Tauri
  nativo). Solo hace falta en el límite wasm-bindgen.
- **Números.** Nada de `u64` en tipos que crucen `wasm-bindgen` directamente
  (mapea a `BigInt`). Por JSON no hay problema.

### 3.2 Fase 2 — la decisión que cambia el proyecto

Reescribir la UI significa volver a construir, como mínimo:

listas, formularios (todos los tipos de campo), kanban, calendario, gantt, árbol,
informes, dashboard con grid y widgets, escritorio con tiles arrastrables,
buscador global, notificaciones, panel de usuario, login/relogin, ajustes,
constructor de DocTypes, gestor de permisos, matriz de roles, portal público,
blog, formularios web, i18n, accesibilidad, CSP con nonce y 353 tests.

| Opción | LOC a reescribir | Riesgo | Qué se conserva |
|---|---|---|---|
| **A. Leptos como adaptador** (CSR, sin SSR) | 0 | Bajo | Todo el Desk; el core manda |
| B. Web Components / HTML+JS desde cero | ~55 000 | **Muy alto** | Solo el contrato |
| C. Piloto: solo Lista + Formulario genéricos | ~8 000 | Medio | El Desk viejo convive |

La opción A cumple el objetivo arquitectónico igual que B: la UI deja de tener
reglas de negocio porque las reglas están en `desk-core`, y la UI se limita a
`render(view(state))` + `dispatch(cmd)`. La diferencia es que A no tira 79 000
líneas funcionando. Además, quitar SSR reduce el acoplamiento con el servidor
(`leptos_axum` desaparece del binario) y el bundle deja de hidratar.

Recomendación: **A ahora, C como banco de pruebas, B solo si A falla por una
razón medida** (tamaño de bundle, arranque, o la necesidad de incrustar la UI en
terceros). Cualquier decisión debe tomarse con un número: `wasm-opt -Oz` del
build CSR y tiempo hasta interactivo, medidos, no estimados.

### 3.3 Fase 3 — Tauri: el camino corto es el *sidecar*

El plan propone `adapter-native` con `reqwest` + SQLite local. Eso reimplementa
permisos, validación, naming, tablas hijas, versiones, workflows y migraciones
en el cliente — exactamente lo que el propio plan quiere evitar. Ferris ya es
un binario con SQLite de primera clase.

| Enfoque | Qué se escribe | Qué se obtiene |
|---|---|---|
| **3A. Sidecar** `ferris serve` local + SQLite en el directorio de datos | Empaquetado, arranque, puerto efímero, token de sesión local, CSP | Motor completo, offline real (= local), sin motor de sincronización |
| 3B. `reqwest` a un servidor remoto + caché SQLite propia | Un motor de caché y sincronización | Solo lectura offline al principio |

3A convierte "offline-first" en "local-first" sin inventar nada. El coste se
mueve a empaquetar el binario y a la compilación cruzada para móvil, que hay
que **medir con un spike** antes de comprometerse: `sqlx`+`ring`+`axum` en
`aarch64-linux-android` y `aarch64-apple-ios` no está verificado en este
repositorio (ver §9).

Nota de seguridad: en 3A el webview habla con `127.0.0.1`, así que hacen falta
token de arranque, puerto no predecible y CSP estricta. Tauri v2 ya aporta
*capabilities*; no basta con confiar en el origen.

### 3.4 Fase 4 — lo que queda de verdad

- Offline con cola de mutaciones: **solo** si se elige 3B. Con 3A no hay cola
  que construir.
- Web Components especializados: ya existen (`ext/`), lo que falta es que el
  host sea la nueva UI.
- UniFFI: no antes de tener una segunda app nativa real. Es coste de
  mantenimiento permanente.

---

## 4. Plan final

### Fase 0 — cirugía en FerrisFramework (obligatoria, precede a todo)

Objetivo: dejar el repositorio en un estado donde la extracción no rompa el
producto. **Sin crear todavía FerrisDesk.**

1. **Partir la feature `web` en dos.** `web` = UI; `cms` = registro de DocTypes
   y workspace del sitio (`catalog.rs:38,84`). Hoy
   `--no-default-features --features api` pierde el CMS, que no es UI.
2. **Extraer `src/website/` + `src/markdown/` a `ferris-website`** (o a Core).
   No dependen de Leptos; sí de `ferris-core`. Es el prerrequisito para que
   `ferris-server` siga sirviendo el sitio público sin `ferris-web`.
3. **Crear `ferris-desk-core` y `ferris-desk-ports` en el workspace** (puros),
   con las reglas de §3.1. Añadirlos a `default-members` para que el chequeo
   barato los cubra.
4. **Refactorizar `ferris-web` para consumirlos** sin cambiar comportamiento:
   `FormField` pasa a `field: FieldState { value: String, .. }` + un mapa de
   señales en la capa Leptos, o un `Store` por índice. La UI sigue igual.
5. **Construir el corpus de paridad** (§5) mientras el Desk viejo aún corre.
6. **Añadir el candado arquitectónico**: un test que falle si `desk-core`
   depende de `leptos`, `web-sys`, `tokio` o `reqwest`, y otro que falle si una
   vista llama a `validate_form`/`build_form_payload` fuera del adaptador.

Salida: mismos gates verdes, mismo Desk, ~5 200 LOC de lógica fuera de la UI y
un corpus que prueba que se comporta igual.

Estimación: 2–3 semanas.

### Fase 1 — nace FerrisDesk

1. Crear el repositorio con la misma disciplina: `Dockerfile`, `compose.yaml`,
   toolchain `1.98.1`, `AGENTS.md` propio, ramas `develop`/`master`.
2. Mover con historia (`git filter-repo`), no copiar: `ferris-web`,
   `ferris-web-ui`, `ferris-web-shared`, `ferris-desk-core`, `ferris-desk-ports`.
3. `FerrisDesk` depende de `FerrisCore` por ruta hermana (`../FerrisCore`), solo
   de `ferris-schema` (`default-features = false`) desde el core.
4. FerrisFramework: `web` pasa a servir la SPA construida (o desaparece y el
   despliegue sirve estáticos). La CLI pierde el modo cargo-leptos.
5. CI: `Format`, `Check`, `Check wasm32`, `Test`, `Core sin dependencias de UI`,
   `Tailwind`, `Tauri build` (matriz por SO).

Salida: dos repositorios verdes; el Desk funciona servido como SPA contra la
misma API, sin SSR.

Estimación: 1–2 semanas.

### Fase 2 — presentación

Según la decisión de §3.2. Entregable mínimo, sea cual sea la opción:

- `desk-adapter-web`: `HttpTransport` (fetch), `LocalStore` (localStorage),
  `RealtimeStream` (SSE `/api/notifications/stream`), `Clock`, `Translator`.
- La UI despacha comandos y pinta `ViewState`. Cero validación propia.
- Lista y formulario genéricos primero; después el resto de vistas.
- Presupuesto de bundle y de tiempo hasta interactivo, medidos en CI.

Salida: checklist de paridad §6 al 100 % para las vistas migradas.

Estimación: 4–8 semanas con A; 12–20 con B.

### Fase 3 — Tauri v2

1. Spike de compilación cruzada del servidor (§9). Si falla, 3A queda para
   escritorio y móvil usa 3B.
2. Escritorio con sidecar + SQLite + token de arranque; CSP y capabilities.
3. Responsive/táctil: reutilizar `app/mobile.rs` (ya existe la preferencia por
   dispositivo) y probar en pantallas reales.
4. Firma, actualizador y empaquetado por plataforma.

Estimación: 3–4 semanas (escritorio), +3–4 (móvil si el spike sale bien).

### Fase 4 — opcional

Cola offline (solo 3B), widgets especializados sobre el nuevo host, UniFFI.

---

## 5. Contrato de paridad (el artefacto que hace verificable el plan)

Antes de mover una línea, capturar del servidor actual un corpus congelado:

- `GET /api/doctype/meta/{doctype}` para un conjunto representativo (una
  DocType con tablas hijas, una Single, una submittable, una del CMS).
- `GET /api/resource/{doctype}` con filtros, orden y paginación.
- Tablas de verdad de `depends_on` (visibilidad, obligatoriedad, read-only),
  incluidas las tres variantes hoy no implementadas.
- Payload resultante tras una secuencia de comandos (crear, editar, tabla hija,
  duplicar, enviar).

`desk-core` reproduce el corpus en tests puros. Es lo que convierte "la UI no
tiene reglas de negocio" en una aserción ejecutable y no en una intención.

---

## 6. Checklist de paridad (criterio de éxito real)

| Entregable | Criterio |
|---|---|
| `desk-core` aislado | `cargo tree -p desk-core` sin `leptos`, `web-sys`, `tokio`, `reqwest`; `cargo test` < 2 s; `cargo check --target wasm32-unknown-unknown` |
| Contrato serde cerrado | El corpus de §5 pasa; ninguna vista contiene `validate`, `payload` ni evaluación de `depends_on` |
| Reglas completas | `depends_on`, `mandatory_depends_on`, `read_only_depends_on`, `collapsible_depends_on` implementadas y cubiertas |
| Desk web | Lista y formulario de cualquier DocType se pintan solo con metadatos; cambiar CSS/HTML no recompila Rust |
| Bundle | Presupuesto medido en CI (wasm `-Oz` + gzip) y tiempo hasta interactivo |
| Tauri | La misma interfaz corre en escritorio y móvil; el almacenamiento es el motor, no una caché paralela |
| Seguridad | El cliente no autoriza nada; el servidor sigue decidiendo |

---

## 7. Estructura propuesta del repositorio FerrisDesk

```text
FerrisDesk/
├── crates/
│   ├── desk-core/        # puro: estado, comandos, efectos, reglas
│   ├── desk-ports/       # traits de infraestructura (solo core)
│   ├── desk-fixtures/    # corpus de paridad (JSON congelado)
│   ├── adapter-web/      # fetch, localStorage, SSE, wasm-bindgen
│   ├── adapter-native/   # cliente del sidecar / reqwest (si 3B)
│   └── desk-leptos/      # presentación (si opción A)
├── apps/
│   ├── web/              # shell, rutas, tema, assets
│   └── tauri/            # Tauri v2: capabilities, sidecar, updater
├── style/                # tailwind.css, components.css, desk.css
├── Dockerfile / compose.yaml / rust-toolchain.toml
└── AGENTS.md             # mismo contrato: contenedor, gates, ramas
```

Dependencias permitidas:

```toml
ferris-schema = { path = "../../FerrisCore/ferris-schema", default-features = false }
```

y nada más de FerrisCore en `desk-core`. `desk-ports` puede añadir
`async-trait` (solo en los traits). Ningún otro crate del workspace base.

---

## 8. Riesgos

| Riesgo | Probabilidad | Impacto | Mitigación |
|---|---|---|---|
| Reescribir la UI consume el proyecto y no llega a paridad | Alta | Crítico | Opción A; piloto C con criterio de salida |
| Divergencia de reglas entre cliente y servidor | Media | Alto | Corpus de §5 + servidor como autoridad |
| El `web` del servidor se rompe al extraer el CMS | Alta | Alto | Fase 0.1–0.2 antes de mover nada |
| `u64`/`BigInt` y fechas en la frontera wasm | Media | Medio | JSON en la frontera; tipos explícitos |
| Móvil: el servidor no compila para Android/iOS | Media | Alto | Spike previo obligatorio; plan B = 3B |
| SQLite local pierde semántica (concurrencia, `ILIKE`) | Baja | Medio | Ya hay dialecto y ramas; usar 3A |
| Dos versiones que se desincronizan | Media | Medio | FerrisDesk declara contra qué versión del contrato compila; el guard de versión sigue en el framework |
| Pérdida de accesibilidad/CSP al reescribir | Media | Alto | Tests actuales se migran, no se borran |

---

## 9. Lo que no está verificado

Todo lo siguiente se afirma como **pendiente**, no como hecho:

- Que `ferris-desk-core` + tests corra en < 2 s (estimación; no medido).
- El tamaño real del wasm en release y con CSR (el artefacto de debug pesa
  313 MB; no hay build release en el árbol).
- Que el servidor compile para `aarch64-linux-android` / `aarch64-apple-ios`.
- Que Tauri v2 móvil acepte el sidecar con SQLite en el directorio de datos de
  cada plataforma.
- El tiempo real de las fases (estimaciones de orden de magnitud).

Verificación realizada para este documento: solo lectura sobre el árbol
(`grep`, `find`, `wc`, `git log`). No se ejecutó `cargo` en el host, conforme a
`AGENTS.md` §0.

---

## 10. Decisión pendiente

La única bifurcación que cambia el plan es la capa de presentación (§3.2). Todo
lo demás es ejecutable en paralelo y no depende de ella. Se recomienda:

**A + Fase 0 primero.** Extraer el núcleo dentro del monorepo, probarlo contra
el corpus con el Desk viejo como referencia, y solo después mover el
repositorio. La separación se demuestra con tests, no con una mudanza.
