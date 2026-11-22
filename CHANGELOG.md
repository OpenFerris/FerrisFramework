# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-11-22

**Minor bump** (`0.2.0 → 0.3.0`): `Cargo.toml`, `ferris-wasm/wit/ferris.wit` and
`test/wit/ferris.wit` move together, and the OpenAPI document reports the same
number. As a pre-1.0 minor it is allowed to break; in practice the `mqtt`
interface is the one removal (see below), while the rest is additive. The policy
is in [`VERSIONING.md`](VERSIONING.md).

### Removed

- **MQTT is retired from the framework.** The `ferris-mqtt` crate, its
  `mqtt` Cargo feature in `ferris-server`/`ferris-cli`, the `[mqtt]` config
  section, the `io::mqtt` WIT interface and the `mqtt-qos`/`mqtt-subscription`
  types are gone; `platform::features()` no longer reports `mqtt`. The docs
  pages `configuracion/mqtt-y-secretos` and `guias/mqtt` are removed from the
  current site (the archived `0.2/` snapshot keeps them). A deployment that
  configured brokers must drop `[mqtt]`; an app that imported `io::mqtt` must be
  recompiled against `ferris:framework@0.3.0` without it. No automatic data
  migration runs: the `MQTT Broker`, `MQTT Subscription` and `MQTT Message`
  tables are no longer registered or read, so drop or archive them explicitly
  when decommissioning a deployment that used them.

### Hardening and features (audit waves A–D)

- **Containment (A)**: secret redaction by path in the CLI, tenant registry
  permissions, private-by-default file uploads with safe `Content-Disposition`,
  verified client certificates no longer imply anonymous service access,
  per-app event/job routing, plugin ingress behind the declared route auth,
  CSRF/API and GraphQL fixes, and document lifecycle/transaction fixes.
- **Integrity and audit (B)**: append-only audit trail with a hash chain,
  `Custom DocPerm` evaluation, persisted login lockout, MFA and JWT/API-key
  revocation, orphan recovery for jobs/files, and WASM `process`/quota
  boundaries.
- **Multi-tenant and Frappe parity (C)**: host-to-tenant resolution with an
  allow-list, per-tenant database/Redis/blob scoping, `frappe.client.*`
  compatibility methods and REST normalizers, `User Permission`, and the
  facade features implemented or withdrawn.
- **Performance, Desk and DX (D)**: metadata/`ensure_table` caching, bounded
  search, cursor and unique-index work, SSR/Desk fixes (navigation, focus,
  theme, bulk delete, calendar/gantt), and the CI/docs/tooling work in this
  release (docs build, SSR binary, `--locked`, audits, Docker pinning).

### Limpieza de código y flujo de apps

- **Auditorías y planes históricos fuera del árbol**: se eliminan los 23
  `AUDIT.md`, `docs/` (auditorías y planes), `tmp/`, y los planes internos de
  `ferris-web/`. La procedencia queda en `git log`; `AGENTS.md` ya no apunta a
  archivos inexistentes.
- **Código muerto**: fuera `ferris-schema/src/wit.rs` (generador de un paquete
  que no es el contrato real), `ferris-realtime/src/gateway.rs` (sin consumidor
  de producción), `ferris-wasm/src/loader.rs` (`PluginManager`, sustituido por
  `PluginStore`), `ToastWrapper`/`show_toast`, y el ejemplo
  `ferris-wasm/examples/load_demo.rs` (hardcodeaba `/tmp`).
- **Duplicaciones unificadas**: `system_manager`/`system_manager_single`
  (cuatro copias → `ferris-schema::builders`), `now_rfc3339` (tres copias),
  `escape_html` (cuatro copias → `ferris-web-shared::html`), la codificación de
  rutas (`urlencode` → `route::path_segment`) y la decodificación
  (`percent_decode`), y `read_package_name` (duplicado de
  `build::package_name`).
- **Flujo de apps**: `ferris new` deja de recibir un `CliConfig` que no usaba;
  `ferris dev` genera un secreto JWT efímero en memoria cuando el proyecto no
  configura uno (avisa y no toca `serve`/`run`, que siguen fail-closed); el
  README del scaffold y las guías de `new`/primeros pasos se corrigen (seis
  interfaces, nueve exports, `schema`, orden de `dev`/`build`).
- **Frontend**: se retiran ocho hooks sin caller y se centralizan los helpers de
  escape/percent; la lista de fieldtypes pasa a 47 (`Widget`).

### DocTypes y API del Desk — paridad

- **Ciclo de vida en `data`**: `rename-doc` y `set-docstatus` (`submit`,
  `cancel`, `amend`) ejecutan las mismas reglas que
  `POST /api/resource/.../{rename,submit,cancel,amend}`; `amend` vive ahora en
  `DocumentEngine` y la ruta REST y el import WASM comparten una sola
  implementación.
- **Metadata completa para guests**: `data.doctype-json` devuelve la
  proyección de `GET /api/doctype/meta/{doctype}` (campos, layout, `virtual`,
  `allow-on-submit`, naming, vistas, actions, links y permission types) y
  `data.field-types` el vocabulario canónico. `schema.declare-doctype(json)`
  permite declarar el shape completo desde `describe()`; el host valida
  fieldtypes, nombres reservados y propiedad de la app antes de registrar nada.
- **`DynField` completo**: el espejo de campo incorpora `search_index`,
  `print_hide`, `report_hide`, `no_copy`, `ignore_user_permissions`,
  `ignore_xss_filter`, `convert_to_user_timezone`, `print_width`, `width`,
  `in_global_search`, `in_preview`, `mask` y `sort_options`; `DynMeta` incorpora
  `is_tree`, `track_changes`, `quick_entry`, `workflow`, los campos de rol
  genéricos y la superficie `allow-*`/`in-*`.
- **Metadata parcial y validada**: `DoctypeMeta`, `FieldMeta` e `IndexMeta`
  llevan `#[serde(default)]` a nivel de struct, así que una declaración de app
  puede omitir cualquier propiedad y las que falten toman su valor por defecto.
  La validación común (`validate_app_doctype`) comprueba nombre, `fieldname` no
  vacío, fieldtype canónico, fieldnames únicos y que `naming_type = field`
  declare `naming_field`, antes de tocar el registro. El descriptor y
  `schema.declare-doctype` usan esa única validación.
- **Permission Types en `platform.can`**: un nombre que no es un derecho
  integrado se resuelve contra `Custom DocPerm` con la misma regla de rol e
  `if_owner` que el Desk; un tipo desconocido es un veredicto `false`, no el
  error «unknown permission».
- **Escáner**: `components/ui/scanner` abre la cámara, decodifica con
  `BarcodeDetector`, llama a `on_scan` por valor, soporta `multiple` con
  debounce y libera todas las pistas al cerrar.
- **Form Tours**: DocTypes `Form Tour`/`Form Tour Step`,
  `GET /api/doctype/form-tours/{doctype}`, `FormTourLauncher` en el formulario y
  `Tour` para resaltar el campo y navegar los pasos.
- **Logging de cliente**: `api::log_client` envía a `POST /api/log/client` y el
  formulario reporta sus fallos de carga/guardado para que no se pierdan al
  cerrar la pestaña.
- **Docs**: nuevas páginas `frontend/api-desk` y `referencia/paridad-frappe` con
  la matriz de las 17 características de DocType y las 13 superficies de API.

### Impresión (`ferris-print`)

- **Formatos de usuario con MiniJinja + Typst**: un `Print Format` elige motor
  `Jinja` o `Typst`, guarda la plantilla y sus `parameters`/`sample_data`, y el
  motor convierte la fuente a PDF/SVG/PNG/HTML. Un formato `Jinja` se renderiza
  primero con MiniJinja y el Typst resultante compila con el mismo motor.
- **Contexto y filtros estables**: `doc`, `params`, `meta`, `user`, `now` y
  `format`; filtros `typst`, `typst_str`, `typst_value`, `date`, `money`,
  `number`, `percent` y `json`. Los límites de tamaño, fuel y recursión convierten
  una plantilla desbocada en un error de validación.
- **`Print Format` centraliza la definición**: `format_name`, `doc_type`,
  `engine`, `template`, `parameters`, `sample_data`, página, márgenes, escala y
  `enabled`. `Print Settings` aporta el fallback de página.
- **Nueva vista previa** `POST /api/print/preview/{doctype}` para un formato
  guardado o una plantilla inline, con `data` de muestra y el permiso `print` del
  DocType.
- **Métodos base para apps**: `print.render`, `print.preview`,
  `print.formats.list` y `print.format.save`, registrados como bases de
  `MethodRegistry`; una app puede sobrescribirlos con `overrides` y usar
  `data::call-method` como `super`. El llamante sale del usuario ambiente, nunca
  de los parámetros.
- **Includes y herencia Jinja**: `FERRIS_PRINT_TEMPLATE_DIR` fija la raíz,
  `Print Format.template_dir` elige una subcarpeta relativa y el mismo
  directorio sirve a `{% include %}`/`{% extends %}` y a los `read()` de Typst;
  `..`, rutas absolutas y symlinks fuera de la raíz se rechazan.
- **Se retiran las capacidades no aplicadas**: `letter_head`, `copies`, `duplex`,
  `color`, `page_range`, `metadata`, `style` CSS y los tipos `JS/HTML/Custom`;
  también los DocTypes `Print Heading`, `Letter Heading` y `Print Format
  Builder`, y los `BatchPrinter`/`PrintQueue`/`TemplateEngine` sin consumidor.
- **`PrintOptions.scale` pasa a `Option<f32>`** para distinguir “no enviado” de
  “1.0”; `PrintFormatDef` incorpora `engine`, `parameters`, `sample_data`,
  `description` y `scale`.

### WIT 0.3.0 — L4/SO

- **El contrato sube a `ferris:framework@0.3.0`** (minor pre-1.0: la app se
  recompila contra el paquete nuevo). El world añade el export
  `handle-datagram(stream-id, data, peer)`, hermano de `handle-stream`: devuelve
  los bytes que salen al mismo peer o una lista vacía para descartar, y
  `stream-info.protocol` distingue `udp`, `raw` e `icmp`.
- **UDP (`udp`)**: `io::udp-connect`, `udp-listen`, `udp-send` y `udp-close`
  abren sockets que el host posee; cada datagrama entra por `handle-datagram`.
  Una respuesta es el valor devuelto, así que un protocolo de petición/respuesta
  no necesita un segundo envío.
- **DNS autoritativo (`dns-server`)**: `io::dns-serve` liga UDP y TCP en el
  mismo puerto y contesta la zona desde los registros que entrega la app; el
  guest nunca ve una consulta ni un socket. `dns-update` sustituye una zona viva
  de forma atómica y `dns-stop` la apaga.
- **Sockets raw/ICMP (`raw-sockets`)**: `io::raw-open`, `raw-send` y `raw-close`
  entregan paquetes enteros; la app construye la cabecera IP y el host solo mueve
  los bytes y aplica sus topes.
- **Procesos (`process`)**: `platform::process-run`/`spawn`/`wait`/`kill` corren
  programas del mundo sin shell, con el entorno declarado y `cwd` confinado a
  `/world`. El host posee el fork, las tuberías, el deadline y la limpieza;
  `stdout`/`stderr` se capturan y un exceso de salida falla con el tope, nunca se
  trunca en silencio.
- **Hilos (`threads`)**: la propuesta de threads de WebAssembly se declara como
  capacidad sin función WIT; el host la reporta en `platform::features()` y
  `platform::grants()` cuando el motor la soporta.
- **`ferris app`**: `list`, `install`, `enable`, `disable` y `remove [--purge]`
  operan sobre `plugins/` y `[apps] enabled`; no hay registro ni manifiesto
  aparte. `install` valida que el fichero sea un componente, escribe de forma
  atómica, crea el mundo y habilita la app. `disable` materializa la lista
  cuando estaba vacía ("todas") para que la configuración describa lo que se
  publica de verdad.
- **Sin tope arbitrario de streams por app**: el límite de 256 conexiones se
  retira; el techo real es el del sistema operativo y la política global del
  host, no una cifra inventada que negaba la conexión 257 a una app sana.
- **Puertos y configuración mínimos**: `address`/`port` viajan por llamada,
  `none`/`0` significa inferido y `stream-describe` reporta el par efectivo
  (`local-addr`/`remote-addr`, `port`). No hay tabla de límites por app ni una
  treintena de parámetros de operador: los topes globales del host se aplican a
  todas las apps por igual.

### Correo entrante (IMAP)

- **Cliente IMAP propio** (`ferris-email/src/imap/`): asíncrono sobre
  `tokio::net` + `tokio-rustls` (TLS implícito 993 o STARTTLS 143, nunca
  `native-tls`), con `LOGIN`, `SELECT`, `UID SEARCH UNSEEN`,
  `UID FETCH <uid> (UID RFC822)`, `UID STORE +FLAGS (\Seen)` y `LOGOUT`. El
  parser de respuestas entiende tagged/untagged y literales `{n}`, y los tests
  usan un servidor TCP de guion local, sin red externa.
- **Pipeline entrante**: `InboundEmail` parsea MIME con `mail-parser`
  (`from`/`to`/`subject`/`text`/`html`/adjuntos) y `InboundProcessor` escribe el
  DocType `Inbound Email` (cabeceras, cuerpos, fuente RFC822, estado,
  `message_id`, `in_reply_to` y `thread`), publica la notificación in-app y
  enlaza respuestas con su mensaje padre.
- **Poller por cuenta**: el hub de email arranca un poller por cada
  `EmailAccountConfig` con `imap_host`, en el proceso con rol `worker` o
  `scheduler`; se apaga con el `shutdown`. Marca `\Seen` **solo** después de
  guardar; un fallo deja el mensaje sin leer y reintenta con backoff. Los
  mensajes se deduplican por `(account, mailbox, uid)`.
- **SMTP con rustls**: `lettre` deja de compilar `native-tls` y usa
  `tokio1-rustls-tls` (`ring`), la misma pila que IMAP. La contraseña de IMAP se
  redacta en `Debug`.

### Notificaciones push in-app

- **Reintentos reales**: una `Push Notification` fallida guarda `attempts`,
  `error` y `next_attempt_at`, y el sweep `integrations_push_delivery` vuelve a
  intentarla con backoff exponencial hasta 5 intentos. La fila nunca queda en un
  estado intermedio bloqueado.
- **Entrega in-app/SSE**: el canal `push` escribe un `Notification` y lo publica
  en el feed de la sesión; no hay APNs ni FCM.
- **`mail-parser` 1.0** entra en `[workspace.dependencies]` (puro Rust,
  Apache-2.0 OR MIT, sin TLS propio); sustituye a un parser MIME escrito a mano.

### Puertos de cola/mailer y handles

- **`ferris_core::ports`**: `QueuePort`/`JobRequest`, `MailerPort`/`EmailMessage`
  y `ServiceHandles` (slots `OnceLock`, primera instalación gana). Un módulo
  encola o envía sin depender de `ferris-jobs`/`ferris-email`.
- **Adaptadores**: `ferris_jobs::ports::JobQueuePort` sobre la cola real y
  `EmailHubMailer` sobre el hub de email del servidor (misma cola que el worker).
  `ServiceContext.handles` los recibe antes de `modules.boot`.

### Módulos nativos, ciclo de vida y files

- **El contrato `Module` está completo**: registro de DocTypes, workspaces y
  métodos; rutas de API; rutas públicas fuera de las capas de auth; OpenAPI; y
  `boot`/`shutdown` con `ServiceContext`/`ServiceRoles`. `files` es el primer
  módulo con ciclo de vida completo: construye el store y las descargas firmadas
  en `boot`, monta la API del Desk y la ruta pública, y registra `files.list`.
- **`print`, `report`, `files`, `email` e `integrations` registran sus propias
  declaraciones**; el catálogo del servidor ya no llama a sus crates. El
  runtime de email/integrations y de realtime/jobs/scheduler sigue en el
  composition root porque comparte cola, mailer o capas de socket.
- **El servidor arranca y para los módulos**: el registro se construye antes del
  bootstrap de authz, se arranca antes del router y se apaga en orden inverso.

### Módulos nativos y features opcionales

- **`ferris-api` deja de depender de las verticales.** El trait `Module`
  (`src/module.rs`) define rutas, orden, prefijo y fragmento OpenAPI; `print` y
  `report` lo implementan en su crate detrás de la feature `http`, y
  `ferris-server` los compone en `router/modules.rs`. La API ya no importa
  `ferris-print`, `ferris-report` ni `ferris-realtime`; un test de arquitectura
  falla si vuelve a hacerlo.
- **El host sigue siendo dueño de la seguridad.** Los routers de módulo se
  anidan dentro de `require_auth_or_device`/`require_csrf`; un módulo no puede
  montar su propia autenticación. Sus paths se publican en
  `/api/openapi.json` desde el mismo registro que los monta.
- **Notificaciones por puerto.** `ferris-api::notify::Notifier` sustituye la
  llamada directa a `ferris-realtime`; el servidor instala el adaptador realtime
  y el sink por defecto es un no-op.
- **`--no-default-features --features api` ya no arrastra verticales.** Ni el
  servidor ni la CLI compilan `ferris-print` en esa configuración; el comando
  `generate templates` y el benchmark de impresión fallan con un mensaje que
  pide `--features print` en vez de fingir éxito.

### Entorno y documentación

- **Entorno de desarrollo Docker**: `Dockerfile`, `compose.yaml` y
  `.devcontainer/devcontainer.json` fijan Rust 1.98.1, Node 24, los targets
  `wasm32-unknown-unknown`/`wasm32-wasip2`, `wasm-bindgen-cli` 0.2.128,
  `cargo-leptos` y `wasm-tools`. La documentación y `AGENTS.md` dejan de depender
  de Distrobox, Podman o rutas de una máquina concreta.
- **Wiki dividida en dos portales**: **Ferris** (framework) y **Apps WASM**
  (`/apps/**`). El portal de apps es agnóstico de lenguaje: Rust, Python,
  JavaScript/TypeScript, Go y C/C++/C#/Zig, con el contrato WIT como única
  interfaz. Rust solo se presenta como una opción en `apps/languages/rust.md`.
- **Salida para LLMs**: `/llms.txt` (índice), `/llms-full.txt` (corpus completo)
  y `/<slug>.md` (Markdown crudo de cada página), generados desde la colección
  de contenido.
- **Una sola versión para Cargo y WIT**: `Cargo.toml` y
  `package ferris:framework@…` comparten cifra (`0.3.0`), y el número indica
  compatibilidad (patch sin WIT, minor retrocompatible, major rompe). Un test de
  `ferris-wasm` falla si las dos copias del contrato o la versión de Cargo
  divergen. La política vive en `VERSIONING.md` y en
  `docs-site/src/content/docs/referencia/versionado.md`.
- **Documentación versionada**: `starlight-versions` publica la copia actual en
  `/` y archiva cada versión en `/<slug>/` con un selector en la cabecera. La
  versión se lee de `Cargo.toml` y se inyecta en el sitio; no hay una cifra
  duplicada en las páginas.
- **Wiki completa en `docs-site/`**: sitio Astro 7 + Starlight (Vite) en español
  con la arquitectura de las 21 crates, DocTypes/campos/naming/docstatus,
  virtual fields y virtual DocTypes, controladores, permisos, Actions/Links,
  Customize, workflows, CLI completa, `config/ferris.toml`, API HTTP, Desk y
  componentes, contrato de apps `ferris:framework@0.3.0` (capacidades, mundo, rutas, métodos
  heredables), referencia de fieldtypes/errores/features, operaciones y guía de
  contribución. Cada página está enlazada desde el sidebar explícito de
  `astro.config.mjs` y se valida con `npm run build`.

### Limpieza greenfield

- Se retiran los `HookPoint` sin emisor (`AfterDelete`, `OnRename`,
  `BeforeImport`, `AfterImport`): el contrato queda en **19 puntos** reales, sin
  nombres reservados que una app pudiera implementar en vano.
- Se eliminan los alias camelCase de fieldtype (`SmallText`, `DynamicLink`, …).
  El único nombre válido es el canónico de Frappe (`Small Text`, `Dynamic Link`);
  un nombre fuera de la lista se rechaza en el borde.
- Se eliminan los alias históricos de `ssl_mode` (`verifyca`, `verify_full`, …):
  las únicas formas son `verify-ca` y `verify-full`.
- Se retiran de los comentarios del código las referencias a auditorías, tandas y
  hallazgos pasados; el código describe el comportamiento actual.

### Contrato de apps — capacidades primordiales y vistas por defecto

- **Capabilities sin configuración de operador**: desaparecen `[apps] trusted`,
  `capabilities`, `grants` y el `trust-profile`. Hay un conjunto **primordial**
  siempre concedido (mundo —files/blobs/`secrets/.env`—, rutas propias incl.
  HTTPS, HTTP saliente, docstore, config, schema, scheduler, notifications,
  pubsub) y otro **específico** que se concede al declararlo
  (`http-unrestricted`, `tls-client`, `tls-server`, `streams`, `streams-listen`,
  `pki`, `overrides`). `verify: false` en TLS exige
  `http-unrestricted`.
- **Vistas por defecto del DocType en la URL**: `/desk/:module/:doctype` abre la
  lista; `/desk/:module/:doctype/:id` abre la vista que declara el DocType
  (`form` por defecto, o `dashboard`/`custom`); y los segmentos reservados
  `list`, `kanban` y `calendar` abren esa vista de colección. Un documento con
  exactamente esos nombres queda eclipsado (documentado en `DocRoute`).


### WIT 15.0.0 — mundo del plugin y APIs heredables

- **El plugin tiene acceso real a su mundo**: cada guest ve `plugins/<app>/` en
  `/world` (lectura/escritura, rutas absolutas). El preopen es todo el sandbox:
  `..`, rutas absolutas fuera del root y un root simbólico se rechazan; el
  montaje es por instancia, así que una app nunca ve el mundo de otra. La
  capacidad `filesystem` es *interna*: la decisión se toma al instanciar, antes
  de que `describe()` pueda declararla, así que un app `sandboxed` con la
  declaración sola nunca tendría mundo. Verificado en
  `ferris-wasm/tests/selective_wasi.rs` (escritura/lectura, `..`, app hermana,
  ruta absoluta y symlink).
- **Retirada la API de `blobs`/`files`/`secrets` del guest**: cada app gestiona
  sus ficheros y secretos en su mundo. Queda la API HTTP mínima del Desk
  (`/api/files`: listar, subir, descargar) para selectores y adjuntos.
  `tls-material.secret` pasa a ser una **ruta dentro del mundo**
  (`secrets/client.pem`), resuelta por el host; las claves `secret` de `config`
  son del operador y el guest no puede leerlas ni escribirlas.
- **APIs heredables (OOP)**: `descriptor.method.overrides` convierte una
  declaración en una sustitución; el host mantiene una cadena ordenada
  (`base → apps`, una entrada por app, recarga idempotente) en
  `ferris_api::MethodRegistry`, y `data::call-method` es el `super` explícito
  (ámbito del llamante: un método que no sobrescribe es `permission-denied`).
  `files.list` es la primera base: `GET /api/files` solo la adapta, así que una
  app puede convertir el selector del Desk en un Drive sin tocar el frontend.
  Sin `overrides` (o sin nada debajo) la app falla al cargar / no se registra.
- `AGENTS.md`, el scaffold de `ferris new` y `test/` actualizados al contrato 15.


### Added
- **Editor de campos en el propio formulario** (`Personalizar` → `Editar
  campos`): el formulario real se vuelve seleccionable y cada campo abre un
  panel de propiedades en la barra lateral (hoja inferior en móvil). Guarda
  `Property Setter`s de las propiedades que el servidor aplica (etiqueta,
  descripción, opciones, valor por defecto, `depends_on`, marcador, obligatorio,
  solo lectura, oculto, en lista, en filtros), marca las que ya están
  personalizadas y permite restablecerlas. No deja aplicar cambios con el
  documento sucio (se perderían al recargar la forma).
- **Impresión real**: el botón del sidebar pide el PDF al motor de impresión
  (`POST /api/print/{doctype}/{name}`) y abre el formato configurado, en lugar
  de imprimir una captura del Desk.
- **Nivel al compartir**: el editor de `Compartir` permite «Puede ver» o «Puede
  editar» (antes toda compartición era de solo lectura aunque la lista mostrara
  el nivel).
- **Login con OAuth**: los proveedores configurados aparecen bajo el formulario
  de acceso, sin tocar el flujo de contraseña/MFA.
- **Avisos y búsqueda honestos**: guardar los ajustes de notificaciones ya no se
  cierra como si hubiera funcionado ante un error, la búsqueda global distingue
  «no disponible» de «sin resultados», y marcar un aviso como leído recupera el
  estado del servidor si la escritura falla.

- **El formulario ocupa todo el ancho** (adiós a la columna centrada `max-w-7xl`).
- **«Personalizar» entra directo al modo edición**: el formulario real es
  seleccionable; el inspector permite crear campos y rupturas de maquetación
  (pestaña, sección, columna), mover el campo arriba/abajo (Property Setter
  `insert_after` aplicado en el servidor), eliminar campos personalizados,
  editar todas las propiedades que el motor aplica y abrir las opciones
  avanzadas (campos personalizados, acciones y enlaces).
- **Exportar la customización a una app**: `POST /api/customization/{doctype}/export`
  escribe `plugins/<app>/customizations/<doctype>.json` (el editor lo ofrece en
  «Avanzado»). El fichero lleva el DocType dentro (`WorldCustomization`), porque
  el nombre del fichero está saneado y no se puede invertir. El servidor valida
  el nombre de la app antes de convertirlo en ruta y crea el mundo si no existe.
- **Mundo de plugin**: cada app habilitada tiene `plugins/<app>/` con
  `customizations/`, `data/`, `files/` y `secrets/`, creados en el arranque
  (serve y dev). Es la unidad de aislamiento de los datos de la app.
- **El mundo puede contener su propio ejecutable**: el cargador acepta
  `plugins/<app>/<app>.wasm` además de `plugins/<app>.wasm` (gana el plano,
  que es lo que escribe `ferris build`).
- **`ferris migrate` aplica las customizaciones de los mundos** de las apps
  habilitadas (`plugin_world`), así que un mundo exportado vuelve a ser metadata
  efectiva en una base nueva. Se hace en migrate, no en el arranque, para no
  revertir en silencio una edición del Desk posterior al último export.
- **Renombrar documentos** desde el menú «⋯» (diálogo, `allow_rename`, y la
  ruta se mueve al nuevo nombre) y **MFA de autoservicio** en
  `Ajustes → Seguridad` (alta con secreto/URI/códigos de respaldo, activación y
  baja). Los códigos de respaldo se hashean en paralelo: diez Argon2 en serie
  superaban el timeout de la petición en un build de depuración.
- **`secret` de config exige la capacidad `secrets`**: un `config-get/set/delete`
  de una clave declarada secreta ya no se sirve solo con `config` (el WIT lo
  documentaba y el host no lo comprobaba).

### Fixed
- **Sin «pop» al actualizar etiquetas**: el sidebar pliega la respuesta del
  servidor en las señales vivas del formulario en vez de recargarlo (adiós al
  esqueleto, a perder la pestaña activa y a una decena de peticiones por clic).
  Asignaciones, adjuntos y comparticiones actualizan su lista localmente.
- **Impresión HTML**: el motor tipográfico rechazaba `#set page` en el target
  HTML («page configuration is not allowed inside of containers»); la
  configuración de página se omite en HTML y el layout genérico se emite bien.
  Regresión cubierta por `the_generic_layout_renders_in_html_and_pdf`.
- **SSR del Desk**: `ferris-web-ssr` no compilaba (`PluginErrorKind::Timeout`
  sin cubrir); el bin no entraba en los gates por defecto. Se añade su
  comprobación a `AGENTS.md`.

## [0.2.0] - 2026-11-19

> Nota de numeración: las entradas de esta sección describen olas del contrato
> anteriores a la unificación. Desde la versión actual, `Cargo.toml` y el
> paquete WIT comparten cifra (`0.2.0`) y la regla de compatibilidad está en
> [`VERSIONING.md`](VERSIONING.md).

### Changed
- **WIT contract `ferris:framework@13.0.0` — ola breaking: sandbox explícito,
  rutas declaradas y ciclo de vida de streams.** El contrato pasa de trece
  interfaces a **cinco** (`types`, `data`, `io`, `platform`, `descriptor`) y la
  app deja de recibir servicios implícitos: `describe().capabilities` declara lo
  que necesita, el operador concede un perfil (`sandboxed` por defecto,
  `trusted` a petición) y cualquier llamada fuera de la concesión efectiva
  responde `permission-denied` en vez de degradarse en silencio
  (`platform::grants` publica el conjunto efectivo).
  - **Rutas**: nuevo `descriptor::route` (`id`, `method`, `path` como prefijo,
    `kind` `http` | `%stream`, `auth`, `body-limit`, `tls-server`, `listener`).
    El host monta exactamente lo declarado —sin rutas `handle-request` no se
    llama nunca— y despacha con el `route-id` que casó (los métodos de API
    llegan como `method:<name>`), así que el guest parsea `http-request.path`.
  - **TLS y red saliente**: `tls-client`/`tls-server`, `tls-material`
    (`pem` | `secret`), `http-policy` (hosts permitidos, privados, tamaño,
    timeout, redirecciones, credenciales) y `stream-listen`
    (WebSocket/TCP/TLS) como política declarada; el host sigue siendo el único
    que hace el handshake, resuelve nombres o abre ficheros.
  - **Streams**: `handle-stream-event` (`opened` | `closed` | `error`) es un
    export propio y `handle-stream` transporta **sólo datos** — un frame vacío
    es legítimo; `io::stream-describe` añade `listener` al `stream-info`.
  - **Consolidación**: `docstore`/`blobstore`/`schema`/`naming` → `data`
    (`write-doc` + `apply-ops` como únicas escrituras, `get-docs` posicional),
    `http`/`pubsub`/`notification`/`streams` → `io`, y
    `session`/`scheduler`/`pki`/`config` → `platform`. `write-no-events` pasa a
    ser `write-disposition.silent`, `session::can`/`can-many` a
    `platform::can(list<permission-check>)` y `website` a
    `option<list<website-route>>`.
  - Lado *guest* migrado: el guest de referencia (`test/`), los fixtures
    `probe-ok`/`probe-trap`/`probe-fs`, la plantilla de `ferris new` y las dos
    copias del WIT. `ferris-wasm/tests/wit_parses.rs` fija ahora las cinco
    interfaces, los ocho exports y las formas nuevas (capabilities, rutas, TLS,
    policy, `handle-stream-event`) y sigue exigiendo que las copias del contrato
    sean byte-idénticas.

### Added
- **Modularidad real por features (2026-11): un binario contiene lo que
  compila.** `ferris-server` y `ferris-cli` exponen una feature por subsistema
  (`api` como base, `web`, `realtime`, `jobs`, `scheduler`, `email`, `files`,
  `print`, `report`, `integrations`, `wasm`, más `full`), todas activas por
  defecto en ambos. `--no-default-features --features
  api,web` produce un servidor mínimo; `--features full` recupera el conjunto
  completo. `[features]` sigue siendo una compuerta: `false` retira lo
  compilado, `true` nunca añade lo ausente, y una config que **pide** un
  subsistema que el binario no tiene falla al arrancar con el comando exacto
  para reconstruir (`--features <nombre>`) en vez de arrancar un proceso que no
  atiende lo configurado. `ferris doctor` imprime las features compiladas y las
  configuradas lado a lado y falla en mismatch; `ferris info` las reporta y
  `ferris config show` las escribe por **stderr** para no romper el JSON de
  stdout. Hay una sola fuente de verdad de lo compilado
  (`ferris_server::host_features`, publicada en `ferris_core::host_features`
  para `platform::features()`): el CLI no relee `cfg!`, y el check de arranque
  del servidor (`validate_compiled_features`) usa la misma regla que el
  pre-vuelo. Implementación: `ferris-cli/src/features.rs`, el pre-vuelo de
  `serve`/`run`/`dev` (`serve::settings_for_with_features`) y el check
  `Feature matrix` de `doctor`.
- **Fase servidor/runtime del WIT 13: concesiones por app, rutas propias,
  egress con TLS, streams y listener TLS.** El contrato ya compilaba contra
  `ferris:framework@13.0.0`; esta ola implementa el lado del host que lo ejecuta.
  - **Concesiones por app** (`[apps]` / `[apps.grants.<app>]`): `trusted` y
    `capabilities` sustituyen a `FERRIS_PLUGIN_TRUST` como fuente de verdad. Los
    servicios internos (docstore, blobs, config, schema, scheduler,
    notificaciones, pubsub) siguen concedidos por defecto; el egress (HTTP, TLS,
    listeners) sólo si la app es `trusted` o el operador nombra la capability.
    Nombrar una capability `trusted`-only la concede aunque el perfil sea
    `sandboxed` (la autorización es la decisión explícita), y **un nombre
    desconocido falla la carga** con la lista válida en vez de conceder de menos
    en silencio. `ferris-core/src/config/models/apps.rs`,
    `ferris-cli/src/commands/plugin_boot.rs`,
    `ferris-wasm/src/host/{grants,state}.rs`.
  - **Rutas declaradas en la raíz** (`ferris-server/src/router/plugin_routes.rs`):
    `descriptor::route` se monta en axum con su método, prefijo, `auth` y
    `body-limit`, y se despacha con `handle_request_for_route(app, route.id, …)`.
    Una ruta que colisiona con el framework (`/api`, `/plugins`, `/desk`, …), con
    otra app o consigo misma **no se monta** y se registra el motivo
    (fail-closed); los comodines (`{}`, `*`, `:`) se rechazan para que un guest no
    pueda inyectar un patrón de axum. `auth = capability(name)` compara
    `x-ferris-capability` en tiempo constante contra la clave secreta
    `route.<name>.token` del config de la app (401 si falta o no casa, 403 si la
    app no la ha provisionado) y la cabecera **no se reenvía** al guest.
    `/plugins/*` y `/api/method/*` se conservan tal cual.
  - **HTTP/TLS saliente** (`ferris-wasm/src/{tls.rs,host_impl/http.rs}`):
    `tls-client` implementa cert/clave/CA/SNI/ALPN/`verify: false` con rustls, y
    `http-policy` aplica `allowed-hosts`, `allow-private`, topes de cuerpo,
    timeout, redirecciones re-validadas por salto y `allow-credentials`. Un
    nombre allow-listed que resuelve a una dirección privada se rechaza igual;
    `http-unrestricted` (trusted, o capability explícita) salta la lista pero
    mantiene todos los topes.
  - **Listener TLS por app** (`ferris-server/src/plugin_net/tls_routes.rs`): una
    ruta con `tls-server` se sirve en un listener dedicado
    (`[apps] plugin_tls_port`, `[apps.plugin_tls_ports]` con una entrada
    `"app-name" = 8443` por app) con el
    certificado de la app, `client-auth` opcional/requerido contra su CA, ALPN y
    **SNI verificado**: un nombre que la app no declara se rechaza en el
    handshake. El material `secret` se resuelve desde el config y exige la
    capability `secrets`; las claves nunca se registran.
  - **Streams** (`ferris-wasm::StreamService` +
    `ferris-server/src/plugin_net/streams.rs`): `connect-stream` (tcp/tls) y
    `listen-stream` (tcp/tls) abren el socket, anuncian `opened`, bombean cada
    chunk a `handle-stream` y el retorno al peer, y cierran con `closed`/`error`
    vía `handle-stream-event`; `send-stream` escribe con cola acotada
    (backpressure) y `close-stream` cierra e idempotente. Los listeners
    declarados como `%stream` se bindean al arrancar. Lo que no se puede cerrar
    devuelve el error tipado correspondiente y queda documentado: `auth` sobre
    un listener TCP/TLS crudo es `unsupported` (no hay petición que
    autenticar).
  - **Desbloqueo**: `PluginErrorKind::Timeout` se traduce a `504 Gateway
    Timeout` también en el camino `/api/method/*` (antes no compilaba:
    el `match` de `router/plugins.rs` no cubría la variante).
- **Streams: cliente WebSocket saliente y rutas `%stream` montadas en la raíz.**
  Cierra los dos huecos que quedaban en el lado host del 13.0.0.
  - **`io::connect-stream` con `ws://`/`wss://`**
    (`ferris-server/src/plugin_net/streams.rs`): handshake RFC 6455 con
    `tokio-tungstenite` (versión 0.29, la que ya resolvía `axum/ws`; features
    sólo `rustls`, nunca `native-tls`). Los frames de texto y binarios son
    chunks y el retorno de `handle-stream` vuelve como un frame binario; un
    frame mayor que `max-frame` se entrega en trozos de `max-frame` (el guest
    reensambla con el buffer de acarreo) en vez de truncarse. `wss` termina TLS
    con `tokio-rustls` y la política `tls-client` de la app —certificado,
    clave, CA, `server-name`, ALPN, `verify`—, es decir exactamente el mismo
    camino que `io::request` sobre `https`; un `wss` sin política declara usa
    los roots del host y **verifica** (el `Default` derivado de `OutboundTls`
    tiene `verify: false` y no se usa). El egress aplica la misma allow-list y
    el mismo `allow-private` que `tcp`/`tls`.
  - **Rutas `%stream` con `transport: websocket`** (`plugin_routes/stream.rs`):
    el router monta un `WebSocketUpgrade` en el path declarado, con el `auth`
    de la ruta (`none`/`session`/capability) aplicado **antes** del upgrade, y
    reenvía cada frame a `handle-stream`/`handle-stream-event` (`opened` antes
    del primer frame, `closed`/`error` exactamente una vez, Close frame limpio
    al cerrar). Con `tls-server` en la misma ruta es `wss` y se sirve en el
    listener TLS dedicado de la app. `/plugins/{app}/ws` se mantiene tal cual.
  - **Cierre ordenado** (`close-stream`): cerrar la cola de escritura deja que
    el pump vacíe lo ya encolado y cierre el cable (Close frame, FIN,
    `close_notify`) antes de abortar, con un plazo acotado por stream para que
    un guest atascado no retenga `io::close-stream`; los hijos de un listener se
    cierran en paralelo.
  - **Reparto del módulo**: `router/plugin_routes.rs` (713 líneas) pasa a
    `plugin_routes/{mod,table,mount,stream,tests}.rs`, con `mod.rs` sólo
    declarando y reexportando.
  - **`stream-info.bytes-received`**: el runtime ya suma cada chunk al entrar en
    `handle-stream` (`plugin/exports.rs`), y ambos transports usan ese único
    seam, así que el contador refleja lo recibido sin cambios en `plugin/**`;
    queda dicho en la documentación del módulo.  - **Recarga en caliente**: las rutas declaradas se montan **al arrancar**. Un
    reload que edite el guest sigue sirviendo las rutas ya montadas (el
    dispatch va por el store), pero una ruta **nueva** no se monta hasta
    reiniciar: se documenta y se prefiere a montar en caliente un patrón que no
    pasó la validación de colisiones.
- **Endpoints reales de impresión (`ferris-print`)**. `POST /api/print/{doctype}/{name}`
  renderiza un documento con su `Print Format` (o un layout genérico integrado si
  el DocType no declara ninguno) a PDF/SVG/PNG/HTML y devuelve el binario con su
  `content-type` y `Content-Disposition`. Autoriza con `Permission::Print` vía
  `DocumentEngine::get_for`, aplica `paper_size`, `margins`, `orientation`,
  `scale` (página Typst) y `watermark` (fondo de página), y **rechaza** con error
  tipado `copies != 1`, `duplex`, `color = false`, `page_range`, `letter_head`,
  `metadata` y un `style` CSS en vez de ignorarlos. `GET /api/print/formats/{doctype}`
  lista los formatos disponibles más el fallback `builtin`. El motor Typst se
  construye una vez por proceso y renderiza en `spawn_blocking`.
  `ferris-api/src/print.rs`, `ferris-print/src/engine.rs`.
- **Endpoints reales de informes (`ferris-report`)**. `POST /api/report/{name}/run`
  ejecuta un `Report` guardado (solo `Query Report`), deriva sus filtros de los
  placeholders `:name` del SQL y los bindea; `GET /api/report/{name}/export`
  (csv/tsv/md/html/json) devuelve el archivo; `GET /api/report` lista los
  informes legibles. Autoriza `Permission::Read` sobre el `Report` y
  `Permission::Report` sobre su `ref_doctype` (además del read-guard del motor).
  `POST /api/report/{name}/prepare` responde **501** de forma explícita: el ciclo
  asíncrono de `Prepared Report` no está implementado.
  `ferris-api/src/report.rs`, `ferris-report/src/query.rs`.

### Security
- **Ningún 5xx filtra el motivo interno.** Las tres respuestas de error del API
  (`error_response` en `ferris-api/src/router.rs`, `internal` en el login y el
  callback OAuth) devuelven `internal error` y mandan el texto completo al log;
  un 4xx conserva su mensaje porque describe la petición del cliente. Antes el
  cuerpo de un 500 llevaba SQL, rutas y la cadena de causas.
- **CORS del plano de autenticación es una allow-list.** `auth_cors` reflejaba
  cualquier `Origin` junto a `Allow-Credentials: true`, así que cualquier sitio
  podía leer respuestas autenticadas. La nueva `CorsPolicy`
  (`ferris-auth/src/cors.rs`) sólo emite cabeceras para un origen configurado, y
  ninguna cuando CORS está apagado o la lista está vacía; `*` no es una entrada.
- **La autorización precede a los hooks de ciclo de vida.** `create` y `update`
  autorizan antes de `before_validate`/`validate`: un llamante sin permiso ya no
  ejecuta hooks con efectos ni puede distinguir un 400 de un 403. Para quien
  está autorizado, el orden de Frappe (naming → before_validate → validate →
  before_insert) no cambia.
- **El WebSocket acepta la cookie de sesión.** El handshake sólo entendía
  `Authorization: Bearer`, de modo que la pestaña del Desk (cookie, sin token)
  conectaba como invitada: sin sala `user:`, sin presencia y con permisos de
  invitado. La cookie se resuelve por el mismo `SessionManager` y se valida
  igual que el token (cuenta deshabilitada incluida); sin credencial sigue
  siendo anónimo.
- **`last_used_at` de las API keys se escribe como mucho cada 5 minutos.** Cada
  petición autenticada por API key escribía la fila; la columna es auditoría, no
  un dato de autenticación.
- **H15 — los assets de plugin dicen la verdad.** `ui-asset` ya no promete el
  "mismo sandbox" del componente: los assets se sirven **same-origin** y son
  código de confianza de la app (pueden usar la sesión del Desk). El espejo
  `test/wit/ferris.wit` queda byte-idéntico y `serve` avisa una vez al montar
  `/plugins`.
- **Revisión de `Desired State`**: su lectura sigue abierta a cualquier usuario
  autenticado — es el contrato del que dependen los agentes (escritura sólo
  `System Manager`, cubierta por
  `desired_state_pulls_are_open_but_publishes_are_not`). El riesgo (el payload
  de todas las entidades es visible para cualquier sesión) queda documentado en
  el módulo; no se cambia el contrato sin decisión del despliegue.

### Added
- **Scheduler multi-instancia con lease en base de datos**. Cada proceso
  cargaba todos los schedules y encolaba su propia copia de cada ejecución en
  cada tick. Ahora todos compiten por una fila `scheduler_lease`
  (`name`/`owner`/`expires_at`, `UPDATE` condicional + `INSERT` y *heartbeat*)
  y sólo el líder encola y recupera las ejecuciones perdidas; los seguidores
  reintentan y toman el relevo cuando el titular deja de renovar. El lease es
  SQL puro (SQLite, PostgreSQL y MySQL) y no necesita Redis. Tests con dos
  `Scheduler` sobre la misma base de datos.
- **`depends_on` se aplica en el claim**. Las aristas se indexan en
  `job_dependencies` y el `claim` sólo entrega un job cuando todas sus
  dependencias están en `success` (una dependencia cuyo registro ya no existe
  no bloquea la cola). Antes se persistía y se ignoraba, así que un job
  dependiente corría antes de tiempo.
- **Purga de historial configurable**. `history_retention_days` borra las filas
  terminales de `jobs` (y sus aristas de dependencia) más antiguas que la
  ventana, al asumir el liderazgo; `max_concurrent_jobs` limita las
  encolaciones programadas simultáneas y `tick_interval_secs` dimensiona el
  *heartbeat* del lease. La tabla `schedules` sólo guarda la definición viva,
  no un historial de ejecuciones, así que la retención se aplica a `jobs`.

### Changed
- **`[features]` se aplica en rutas y hooks, no solo en roles**. `FeatureFlags`
  gana `report` (default `true`) y el doc del struct enumera el efecto real de
  cada flag: `realtime`/`jobs`/`scheduler` retiran su rol, `email` no construye
  hub/worker, `files` no monta `/files`, `print`/`report`/`search` no montan sus
  rutas `/api/print*`, `/api/report*` y `/api/search` (un 404 honesto, no un
  handler con `warn!`), y `webhooks` impide el dispatch de filas `Webhook` sin
  tocar automation/assignment/milestone/push. `ApiState` transporta los flags y
  `api_router_with_app_state` construye la tabla de rutas a partir de ellos;
  `IntegrationsContext` hace lo propio con `webhooks`. Nuevo
  `FERRIS_REPORT`. `ferris-core/src/config/models.rs`, `ferris-api/src/router.rs`,
  `ferris-server/src/{router,server}.rs`, `ferris-integrations/src/runtime/{state,hooks,webhook}.rs`.
- **`JobQueue` mantiene el ZSET de Redis**: `enqueue` era el único `ZADD` y
  nada hacía `ZREM`, de modo que `queued` (`ZCARD`) contaba como "esperando"
  todos los jobs ya reclamados o terminados. Claim, completado, fallo,
  cancelación, reintento y recuperación de huérfanos actualizan el conjunto;
  los e2e de Redis comprueban el `ZCARD` tras cada transición.
- **El worker toma el permiso antes de reclamar**. Reclamar primero marcaba el
  job `running` (y su `started_at`) mientras la tarea esperaba un slot, así que
  el tiempo de espera contaba como ejecución y otro proceso podía verlo como
  huérfano; ahora un worker sin slot libre no toca la cola.
- **Una fila corrupta de `schedules` ya no impide arrancar**: `list()` la omite
  con un `warn!` que nombra la columna, en lugar de fallar la lectura completa.
- **`tokio-cron-scheduler` sin features**: se eliminan `signal` y
  `postgres-native-tls` (no se usan y arrastraban `tokio-postgres` +
  `native-tls`/OpenSSL); `cargo tree -p ferris-scheduler -e features` ya no
  muestra `native-tls`.
- **Campo `Secret`: credenciales recuperables cifradas en reposo (H3)**. Un
  `Secret` es el contrapunto de `Password`: se cifra con AES-256-GCM
  (`ring`, sin criptografía propia) en un sobre versionado
  `ferris:v1:<key-id>:<base64url>`, con nonce aleatorio por valor y el DocType +
  fieldname como *associated data* (un cifrado movido a otro campo falla la
  autenticación). Al leer, todo caller HTTP/Desk —System Manager incluido— ve la
  máscara; sólo `DocumentEngine::read_secret*` sobre un `EngineContext` con
  `reveal_secrets()` devuelve el plaintext, como `SecretValue` (Debug redactado,
  sin `Display`/`Serialize`). La clave sale de `[secrets] key` /
  `FERRIS_SECRETS__KEY` (32 bytes en hex o base64) y admite `previous_keys` para
  rotar. Los 8 campos de integraciones (`Connected App`, `Webhook`, `Google
  Settings`, `Social Login Key`, `LDAP Settings`, `OAuth Client`, `OAuth
  Provider`) y los 2 de `Email Account` dejan de ser `Password`, con lo que
  OAuth/webhook/SMTP vuelven a firmar y autenticar con el valor real en vez de
  con `"********"`. Las filas escritas antes del cambio contienen hashes Argon2
  irrecuperables: un `read_secret` devuelve `SecretError::NotRecoverable`
  (reintroducir el valor en el Desk) y `ferris doctor` las lista. El fieldtype
  viaja como `string` en WIT, así que el contrato del plugin no cambia.
- **Editor de tablas hijas en el formulario**: los campos `Table` dejan de ser
  un aviso y se editan en una cuadrícula real (checkbox, select, número y texto
  según el campo hijo), con añadir/eliminar fila, renumeración de `idx` y
  estado read-only heredado del documento. El payload de guardado vuelve a
  enviar la tabla —antes se descartaba, así que guardar un padre borraba sus
  filas— y una tabla obligatoria vacía bloquea el guardado.
- **Página de Ajustes real** (`/settings`, `/settings/permissions`,
  `/settings/about`): sitio, comprobación del motor, tema, matriz de permisos
  personalizados y versiones. Cada pestaña es una URL; el diagnóstico ya no
  consulta un `Sales Invoice` hardcodeado.
- **`ferris serve` y `ferris run` publican las apps WASM de `[apps]`**
  (`apps.wasm_dir`, por defecto `./plugins`, y `apps.enabled` como lista
  blanca). Antes sólo `ferris dev` asignaba `ServerSettings.plugins`, así que un
  despliegue creado por `ferris deploy` —que ya copia `plugins/<app>.wasm` junto
  a `config/`— arrancaba sin `/plugins` y sin ningún `/api/method/<name>`, en
  silencio. El arranque ahora descubre los `*.wasm` en orden alfabético estable
  (el nombre del fichero es el nombre del plugin, con guiones), los compila y
  registra sus DocTypes/eventos **antes** de `build_router`, y falla de forma
  explícita si una app listada en `enabled` no existe o un componente no carga.
  Un directorio ausente sigue siendo un arranque válido sin apps.
- **Paridad con Frappe en DocTypes (10 áreas)**: se completan las funciones que
  faltaban del contrato de DocTypes:
  - **Naming**: los nueve métodos (`Set by user`, `Autoincrement`, `By
    fieldname`, `By "Naming Series" field`, `Expression`, `Expression (Old
    Style)`, `Random`, `UUID`, `By script`) están en `NamingType`; `Script` es
    un proveedor Rust registrado (`register_naming_provider`), no Python. Las
    **Document Naming Rule** se aplican antes del método declarado, con
    `priority`, `prefix_digits` y condiciones por campo (tabla hija
    `Document Naming Rule Condition`), y su contador se incrementa en la misma
    transacción que el INSERT.
  - **DocStatus**: helpers `is_draft`/`is_submitted`/`is_cancelled` y
    conversión 0/1/2 (`as_i64`/`from_i64`); el formulario bloquea todo el
    documento cancelado y, en uno enviado, sólo deja editables los campos
    `allow_on_submit`.
  - **Virtual DocField**: `FieldMeta.virtual` (wire `"virtual"`) marca un campo
    calculado al leer. El motor lo rechaza en escrituras
    (`VirtualFieldReadOnly`) y lo resuelve con `register_virtual_field`; el
    Desk lo pinta read-only y no lo envía en el payload.
  - **Virtual DocType**: el motor delega `get`/`list`/`create`/`update`/`delete`
    en un `VirtualDoctypeProvider` registrado
    (`register_virtual_doctype`), sin tabla propia; sin proveedor falla con un
    error tipado (`VirtualDoctypeProviderMissing`).
  - **Controllers**: el ciclo de vida incorpora `before_naming`,
    `before_validate`, `before_update_after_submit`, `on_update_after_submit`,
    `on_change`, `before_rename` y `after_rename`, además de los 16 puntos
    existentes. `DocumentEngine::rename` ejecuta el par de rename en una sola
    transacción y respeta `allow_rename`; no mueve tablas laterales
    (versiones/compartidos/comentarios), limitación explícita.
  - **Actions and Links**: `DocType Action` y `DocType Link` se sirven dentro de
    `GET /api/doctype/meta/{doctype}`; `GET /api/doctype/connections/{doctype}/{name}`
    resuelve las conexiones (incluidas las que viven en una tabla hija) y el
    sidebar del formulario pinta Acciones y Conexiones. Una `Server Action` se
    muestra deshabilitada porque este build no tiene registro de métodos
    whitelisted (nunca un clic silencioso).
  - **Customize**: se añade el DocType `Custom DocPerm` (overrides por
    sitio/rol, `select` reemplaza las filas estándar) y se fusiona en
    `effective_meta`, de modo que la metadata del Desk y la comprobación de
    permisos leen las mismas reglas. Custom Fields, Property Setters, Customize
    Form, Client/Server Script y la edición de Actions/Links con `custom` ya
    existían. En Ferris la lógica de servidor/cliente se ejecuta con el contrato
    de plugins WASM (`describe`/`doctype-event`/assets), no evaluando Python o
    JavaScript almacenado en la base de datos: los DocTypes de script son la
    declaración que el host de plugins consume, no un intérprete embebido.
  - **Permission Types**: `Permission Type` declara permisos por DocType
    (`approve`, `download`, …); se asignan a roles mediante
    `Custom DocPerm.custom_permissions` y se comprueban con
    `has_custom_permission` (misma metadata efectiva). `GET
    /api/doctype/permissions/{doctype}` devuelve los tipos declarados y si el
    usuario los tiene.
  - **Módulos**: `DocType` sigue declarando su `module` y `Module Def` agrupa el
    catálogo; no cambia el contrato.
  - **Amend 1:1**: una enmienda se llama `NAME-1`, `NAME-2`, … (nunca reutiliza
    un número), y copia `amended_from` cuando el DocType lo declara.
  - **`Random` 1:1**: diez caracteres base32hex en minúsculas
    (`0-9a-v`), como el `base32hexencode` de Frappe.
  - **Ciclo de vida WASM completo**: `doctype-event` recibe los 23 puntos del
    motor (naming, validate, update-after-submit, on-change, rename, import…),
    con `validate` incluido entre los que abortan la escritura si el invitado
    devuelve error. No cambia la firma del WIT, solo la tabla documentada y el
    despacho del host.
  - **Server Action ejecutable**: un `DocType Action` de tipo `Server Action`
    llama a `POST /api/method/{action}` con `{doctype, docname, doc}` (la forma
    `kwargs` de Frappe); el sidebar muestra el error del servidor o recarga el
    formulario tras el éxito.
  - **Customize edita Actions y Links**: el diálogo «Personalizar» añade y
    elimina filas de `DocType Action`/`DocType Link` con `custom = 1`, sin salir
    del formulario.
  - **Role Permissions Manager**: página de ajustes con la matriz
    permiso × rol; cada celda escribe `Custom DocPerm.custom_permissions` y
    reconstruye la política instalada al instante. Solo `System Manager`.
  - **Virtual child tables**: un campo virtual de tipo `Table` usa el mismo
    registro; el proveedor devuelve el array y el motor lo entrega tal cual.
  - **Nombre de tabla correcto**: `tabDocTypeAction`, `tabDocTypeLink`,
    `tabPermissionType` y `tabCustomDocPerm` (sin espacios). Con el nombre de
    presentación, las filas se leían como «sin metadatos».
- **Barra superior del escritorio (`/desk`)**: el grid de workspaces deja de ser
  una página sin controles. Lleva una barra con la marca del sistema, buscador
  (abre el diálogo global), campana con el mismo listado que el panel de
  notificaciones y menú del usuario (editar su propio documento, cambiar de tema
  y cerrar sesión). Reutiliza los componentes del shell —`DropdownMenu`,
  `NotificationFeedList`, `ThemeDialog` y el cierre de sesión del `UserMenu`— en
  vez de duplicarlos. Los iconos del grid crecen, y el nombre de cada workspace
  se dibuja siempre debajo en texto plano, con altura reservada para que no se
  solape con la fila siguiente.
- **Iconos SVG declarados por la app**: `workspace.icon` y `folder.icon` aceptan
  un nombre de icono (`"Boxes"`) o un documento SVG (`"<svg …>…</svg>"`). El grid
  lo sanea en el cliente (misma política que las previsualizaciones de widgets) y
  lo monta en un shadow root para que no pueda inyectar estilos en el Desk y para
  que herede `currentColor` y los tokens del tema; SSR y la primera hidratación
  dibujan un `<img>` con data URI seguro. El sidebar y los menús siguen usando la
  forma de nombre, porque son filas de una línea.
- **Parámetros de widget con el renderizador del formulario** (WIT 12.0.0): el
  panel de propiedades deja de tener controles propios y dibuja cada
  `WidgetParam` con el mismo `FormField`/`render_form_field` que un documento.
  Un widget declara la forma de su panel con los marcadores de layout de
  siempre (`Tab Break`, `Section Break`, `Column Break`, `Heading`, `HTML`,
  `Read Only`); sin marcadores, el `group` histórico sigue sintetizando
  secciones, así que ninguna definición existente cambia. El contrato
  `widget-param` (y el DocType `Widget Param`) gana `placeholder`,
  `depends_on`, `read_only` y `hidden`; el salto de versión es mayor porque
  añadir campos a un `record` rompe a todos los invitados que lo construyen.
- **Condicionales `depends_on` en el formulario**: un campo puede declarar una
  expresión (`doc.status == "Open"`, `pagado && !anulado`, `page-size >= 10`)
  y el renderizador la evalúa con un intérprete propio, sin JavaScript: sólo
  identificadores, literales, comparaciones, `&&`/`||`/`!`, paréntesis y el
  prefijo opcional `eval:`/`doc.`. Un campo oculto no se pinta y tampoco puede
  bloquear el guardado con un `reqd`; una expresión mal escrita falla abierta
  (muestra el campo) para que una errata de metadatos no esconda un control
  necesario. El mismo evaluador decide la visibilidad en el panel de parámetros.
- **Catálogo de widgets como DocType `Widget`** (WIT 11.0.0): la definición
  completa (metadatos, parámetros y fuente JS/CSS/SSR) vive en una fila
  `Widget` (`{app}.{widget_name}`); `describe().widgets` es la semilla de la
  migración inicial, que el host aplica de forma idempotente y con `seed_hash`
  para no pisar ediciones del Desk. El Desk monta el cuerpo desde el código
  guardado; las apps ya no listan sus módulos de widget en el manifiesto.
- **Configuración del tablero en el Desk** (fase C6): botón **"Configuración"** en
  el top bar mientras el modo edición está activo, que abre un *drawer* con:
  - **Disposición**: separación entre widgets, padding y radio del tablero
    (vacío = heredar el valor del Desk, nunca 0).
  - **Color**: fondo del tablero por token semántico, sin fondo o color
    personalizado (`input[type=color]`, inyectado como variable CSS).
  - **Chrome**: barra superior y título a ancho completo (se guardan ya; el top
    bar los aplica en la fase C7).
  - Aplicar / Cancelar / **Restaurar global**: el panel edita un *draft* que el
    tablero previsualiza en vivo; Aplicar lo copia a la configuración (y se
    persiste con el "Guardar" del top bar), Cancelar lo descarta y Restaurar
    global lo vacía para ver el look global antes de decidir.
  - El panel de ajustes **por widget** gana la sección **Estilo** (fondo, borde,
    ancho de borde, radio, padding y sombra), sobre el mismo *draft* que ya
    confirmaba título y parámetros: un estilo íntegramente vacío se guarda como
    "sin override", así que el widget vuelve a heredar el default de su
    definición. Los controles compartidos (número, token cerrado, superficie con
    color libre) viven en `views/dashboard/style_fields.rs`, de modo que ambos
    paneles resuelven la regla del contrato igual (el token gana al color libre)
    y una lista de tokens desconocida no se reescribe al abrir el panel.
  - `style/desk.css` regenerado; tests nuevos de las reglas del panel y
    `design_system` en verde.
- **Estilo global del Desk** (fase C8 de la implementación de dashboard): el tablero resuelve su aspecto en tres
  capas — global del Desk → estilo declarado por la app/documento → override
  guardado del tablero.
  - Nueva fila compartida (`Dashboard Settings`, `name = "global"`, campo
    `config`) con endpoints `GET|PUT /api/dashboard-settings`. La lee cualquier
    usuario; la escribe `System Manager` (`Create`), porque cambia el aspecto de
    todos los tableros. Sin fila o con la fila corrupta se usa el estilo
    incorporado, así que un tablero nunca deja de pintar por el global.
  - El *drawer* de configuración gana el conmutador **"Este tablero" / "Global
    del Desk"** (sólo visible si el servidor confirma permiso), con los mismos
    campos para ambos ámbitos y vista previa en vivo: el global se previsualiza
    como base, con el override del tablero todavía por encima.
  - **Guardar global** persiste el ámbito global por su cuenta; **Restaurar
    global** suelta el override del tablero y **Restaurar por defecto** vuelve al
    global incorporado (ambos como borrador hasta guardar).
  - La base constante `style::GLOBAL` desaparece: la precedencia recibe el
    global como dato, y los tests cubren los tres niveles (incluido un `false`
    explícito del tablero que gana a un global `true`).
- **Barra superior y título a ancho completo** (fase C7 de la implementación de dashboard): los dos flags de chrome que el panel
  de configuración ya guardaba ahora se aplican.
  - **Barra a ancho completo**: el shell monta el top bar por encima de la fila
    de la sidebar, de modo que ocupa toda la ventana y la sidebar empieza debajo
    (sin `fixed` ni márgenes negativos); el resto de vistas conserva la barra
    dentro de la columna de contenido.
  - **Título a ancho completo**: el nombre del tablero deja el grupo de migas y
    se dibuja en su propia fila bajo la barra, visible también en móvil.
  - El tablero publica ambos flags desde su estilo resuelto (global → override),
    así que el *drawer* los previsualiza en vivo, y restaura la barra normal al
    desmontarse. Slot nuevo `TopBarChrome` en `ferris-web-ui`.
- **Dashboard builder: parámetros como fields del form + catálogo de widgets
  por categorías con previews** (plan y estado en
  la implementación de widgets):
  - **D1 — `WidgetDef.preview` (WIT 9.0.0)**: el contrato de widgets gana
    `preview: Option<String>` (URL de imagen para la card del catálogo) a
    través de las cuatro capas (`ferris-schema` contract → `WidgetDefSpec` →
    `record widget-def` en el WIT → `descriptor` y `ferris-api::ui`). Es un
    bump **mayor** (se añade campo a un record), siguiendo el precedente de
    las olas anteriores; guardas nuevas en `ferris-wasm/tests/wit_parses.rs`
    (`@9.0.0` + `wave_9_0_0_widget_preview`).
  - **D2 — parámetros de widget como `FormField`**: el panel de propiedades
    deja de tener controles propios (`ParamField`) y renderiza cada
    `WidgetParam` con `render_form_field` — mismo sistema que el formulario de
    documento — tras el mapeo de `views/dashboard/params.rs`
    (`param_fieldtype`/`param_to_field`/`sync_params`). El valor no se
    materializa si el usuario no lo cambia (respeta `param_or`), "Aplicar"
    valida con `validate_form` y los errores pintan por la señal de cada
    campo.
  - **D3 — catálogo en cards por categoría**: `drawer/` sustituye al panel
    plano. Nivel 1: una card por `WidgetDef.category` (`Otros` al final) más
    "Todos los widgets", cada una con mosaico de hasta cuatro previews;
    nivel 2: rejilla de cards de widget (título + badge de app) con `<img>`
    de preview (`preview` declarado → fallback por `kind` → `generic.svg` vía
    `on:error`). Previews servidos desde una tabla embebida
    (`ferris-web/src/preview_assets.rs` + `/static/previews/{*path}` en
    `ferris-server` y `ferris-web-ssr`), y los tres widgets host publican la
    suya. Agrupación por `category` (el `kind` sigue decidiendo el tamaño).
- **Previews de widget por carpeta de origen y con el tema activo** (ola 2 de
  la implementación de widgets):
  - **Categoría = carpeta de origen** (guest SolmecBOX): `WidgetDef.category`
    pasa a ser el nombre legible de la carpeta real del asset
    (`ui/widgets/dashboard/<carpeta>/…`, 16 carpetas; `source` para los
    históricos), derivado en `spec/categories.rs`. El catálogo agrupa por las
    mismas 16 categorías que el árbol de fuentes y la ruta de la preview cuelga
    de esa carpeta (`previews/<carpeta>/<name>.svg`); un test exige que la
    categoría **sea** la de la carpeta del `preview`, así mover un widget sin
    actualizar su definición rompe la build.
  - **Preview con tokens del tema** (Desk): la card del catálogo deja de pintar
    solo un `<img>` (que, al ser documento aislado, no ve `var(--token)`) y
    pasa a un contenedor con **shadow root** donde `ferris-web` inyecta el SVG
    **saneado** (`views/dashboard/preview.rs`): las *custom properties* del
    Desk — claro/oscuro o tema de plugin — se heredan a través de la frontera
    del shadow root, así que la preview sigue el tema activo en runtime. El
    `<img>` se mantiene como markup SSR/hidratación y como fallback `on:error`;
    el saneado (rechaza raíz no-`<svg>`, elimina `<script>`/`<foreignObject>`,
    `on*`, `href`/`xlink:href` externos y `<style>`/`<link>` que traen
    recursos) acota el markup de la app sin añadir dependencias.
- Initial project structure as Rust workspace
- Full-stack framework with metadata-driven doctypes
- Multi-database support (PostgreSQL, MySQL, SQLite)
- Real-time WebSocket support
- Job queue and scheduling
- Authentication and authorization
- Web UI with Leptos
- WASM client support
- File management
- Search functionality
- Email and reporting tools
- **Dashboard builder in the Desk** — `views/dashboard_editor.rs` replaced by
  `views/dashboard/` (`model`, `card`, `canvas`, `drawer`, `props`, `view`):
  the effective layout renders read-only, and an authorized user drags and
  resizes the tiles over a `grid-cols-12` / `auto-rows-[72px]` canvas, adds
  widgets from a palette grouped by app, edits the title, footprint and
  grouped parameters of an instance, and saves back to
  `PUT /api/dashboard-layout/{owner}/{dashboard}`. Native widgets refresh on a
  15 s tick while no dialog or drawer is open.
- **View kinds por doctype en el Desk** — `DoctypeMeta`/`DynMeta` ganan `view`
  (`form` | `dashboard` | `custom`) y `dashboard` (binding nativo / fijo /
  wrapper vía link field). Al abrir un documento en
  `/desk/:module/:doctype/:name` (la URL no cambia), el dispatcher de
  `FormView` monta el motor de dashboards existente (`DashboardDocView`,
  sobre el mismo `DashboardGrid` del Desk) en vez del formulario, o reporta
  el view custom ausente en español; `new` sigue abriendo el formulario. El
  `Dashboard` nativo declara `.dashboard_view()` y sus documentos abren como
  su propio panel. Las apps lo declaran vía WIT (6.0.0).
- **Form View UX: tipo de naming declarable + inputs dedicados + parámetros de
  campo** (plan y estado en el plan de Form View):
  - nuevo `NamingType` (`series` | `uuid` | `auto_increment` | `field` |
    `prompt`) en el contrato (`ferris-schema/src/contract/meta.rs`), espejado en
    `DynMeta` y en el registry (`DoctypeMeta::naming_type`/`naming_field`);
    `NamingType::resolve` conserva el comportamiento previo (serie si la hay,
    UUID si no) y una declaración explícita siempre gana. El `#[doctype]` acepta
    `naming_type`/`naming_field` y el engine acuña el nombre por tipo en
    `write.rs::mint_name` (`Prompt` sin name = `NameRequired`, `Field` sin valor
    = `NamingFieldEmpty`).
  - en `new`, el form-view ya **no** muestra comentarios ni actividad, y el campo
    id se rellena con la serie declarada (o queda de solo lectura cuando el
    naming es del engine) sin enviar la plantilla al backend.
  - `field_types/` se parte en `text`, `number`, `choice`, `datetime`,
    `contact`, `color`, `link` y `misc`, con un componente dedicado por tipo que
    reutiliza `ferris-web-ui` (`Input`, `Textarea`, `SelectNative`, `Checkbox`,
    `InputGroup`, `InputPhone`, `Button`, `Spinner`); el `LinkField` gana
    búsqueda con debounce/secuencia, limpiar, spinner, estado vacío, teclado y
    a11y (`combobox`/`listbox`/`aria-activedescendant`).
  - `DynField` expone `bold`, `max_length`, `precision`, `fetch_from` y
    `fetch_if_empty`, y el renderer los aplica.

### Changed
- **Toolchain 1.98.1 y PATH de `rust_env`**: `/usr/bin` de Alpine trae Rust
  1.96.1 y eclipsaba los shims de rustup en `/usr/local/cargo/bin`, así que
  `cargo`/`rustc` resolvían la toolchain de la distribución aunque
  `rust-toolchain.toml` fije 1.98.1. El contrato (`AGENTS.md` §0) exporta
  `/usr/local/cargo/bin` antes de invocar cargo y la comprobación de sanidad
  exige 1.98.1; `target/` queda ligado a esa toolchain.
- **mTLS de dispositivo y CA interna**: `ferris-server` termina TLS con
  verificación opcional de certificado de cliente (`tls.rs`) y expone los datos
  verificados del certificado como identidad ambiente (`client_identity.rs`);
  `pki.rs` firma CSRs de dispositivo con el CA interno, registra el fingerprint
  emisor y consulta la revocación durante el handshake. El scoping por app de
  `pki::sign-csr`/`pki::revoke` se describe en la entrada de blobs privados. La
  CA debe ser ECDSA o Ed25519 (el backend `ring` no firma con RSA).
- **La configuración `[files]`/`[email]`/`[search]`/`[realtime]`/`[jobs]`/`[scheduler]`/`[features]` ya se aplica**. `ferris serve`/`run` mapean cada sección al `CoreConfig` en vez de pasar `Default::default()` o constantes: `[files]` (backend S3/Azure/GCS, límites, extensiones, optimización), `[email]` (cuentas SMTP y política de cola), `[search]`, `[realtime]`, y todos los knobs de `[jobs]`/`[scheduler]` (`queues`, `default_timeout_secs`, `max_retries`, `retry_backoff_base_secs`, `dead_letter_queue`, `timezone`, `tick_interval_secs`, `max_concurrent_jobs`, `history_retention_days`). `[server] body_limit_mb` es configurable (default 50, `FERRIS_BODY_LIMIT_MB`) y llega al router. `[features]` deja de ser inerte: `realtime`/`jobs`/`scheduler = false` retiran su rol aunque `--all`/`roles = ["all"]` lo pida (con `warn!` nombrando la feature), y `email`/`files` no arrancan el hub/worker ni montan `/files`; `print`/`search`/`webhooks` viajan en `FeatureFlags` y se avisa cuando su consumidor aún no lo lee. `ferris-cli/src/{config.rs,commands/serve.rs}`, `ferris-core/src/config/{models,load}.rs`, `ferris-server/src/{server,background}.rs`.
- **H5: un solo dueño de shares y versiones — el motor**. Se elimina el flag
  `runtime`: `DocumentEngine::new` consulta siempre `tab_share` y graba
  `tab_version` en la **misma transacción** del write (`Create`, `Update`,
  `Delete` con tombstone tras el cascade, `Submit`, `Cancel` y `Rename`, que
  mueve historial y shares al nombre nuevo). El API deja de escribir versiones
  (`VersionManager` queda como lector/importador), de modo que un write produce
  exactamente una fila. `ensure_runtime_tables` se cachea por `Db::id`, no por
  engine, para no pagar DDL en cada request. Los endpoints de shares exigen
  `Permission::Share` (`ensure_shareable`), no `Write`, y `get`/`list` respetan
  los grants persistidos.
- **H4: un plugin no puede declarar ni reemplazar DocTypes del framework o de
  otra app**. El registro gana procedencia (`Origin::Framework` / `App(name)`),
  una lista reservada normalizada (sin espacios, minúsculas), detección de
  colisión de tabla física (`ToDo` == `To Do`, `tabToDo`) y errores tipados
  (`ReservedDocType`, `DocTypeOwnedBy`, `DocTypeTableCollision`). El host WASM
  valida **todo** el descriptor antes de registrar temas/workspaces/UI, fuerza
  `meta.module = app` (la identidad no la elige el guest) y guarda el fingerprint
  de sincronización por `(app, doctype)`.
- **`/api/search` es una búsqueda SQL acotada, no un índice**: el `EngineContext`
  lleva el `meta` del DocType (redacción de `Password`/`Secret` y campos no
  legibles), el total se recorta a `max_results` con un techo de 200 y el barrido
  a 32 DocTypes por consulta. Un fallo de motor distinto de `Authz` ahora
  responde error en vez de "sin resultados". El doc explica que el término se
  filtra en Rust (el plan no lo empuja a SQL), así que la respuesta es siempre
  consistente con los documentos vivos. `ferris-api/src/desk.rs`.
- **`SmtpConfig::default()` es coherente con el puerto 587**: STARTTLS activo e
  implícito apagado. `use_tls` y `use_starttls` juntos se rechazan como
  configuración contradictoria. `ferris-email/src/smtp.rs`.
- **`PropertySetter` valida `fieldtype` con la misma allowlist que
  `add_custom_field`** (H10): un nombre desconocido se rechaza con
  `SchemaError::InvalidFieldType` y un alias legado se guarda canónico, así que
  el setter no puede aceptarse y quedar sin efecto. `ferris-schema/src/customization/mod.rs`.
- **Constancia visual en todo el Desk**:
  - Un solo juego de primitivas de página (`PageHeader`, `SectionTitle`,
    `Feedback`, `EmptyState`, `LoadingLines`) sustituye los títulos, banners y
    estados vacíos que cada vista inventaba. El error siempre es
    `border-destructive/40` + `role="alert"`; el éxito usa `success`.
  - Radios unificados: superficies (Card, Dialog, Popover, Dropdown, Table,
    Command) `rounded-xl`; controles (Button, Input, Textarea, Select, Badge)
    `rounded-md`; feedback `rounded-lg`.
  - `SelectNative` comparte ahora `.ferris-input`: mismo alto (`h-9`), borde,
    foco, inválido y tipografía que el resto de campos (antes era `rounded-lg`,
    `shadow-sm` y `text-sm`).
  - Los campos del formulario llevan `ferris-field` + `aria-invalid`; un error
    pinta el control (incluidos select y widgets complejos) y se anuncia con
    `role="alert"`.
  - Las secciones del formulario son responsivas: una columna en móvil y la
    cuenta declarada desde `md` (antes `repeat(N)` fijo en cualquier ancho).
  - Todas las páginas comparten `p-4` y scroll propio; el formulario y los
    informes ya no quedan recortados por el `overflow-hidden` del shell.
  - Calendario y Kanban dejan de hardcodear `Sales Invoice`: calendario elige
    DocType con un selector y enlaza con su módulo real; Kanban enlaza con el
    módulo del DocType.
  - El botón de imprimir del formulario abre el diálogo del navegador (se
    deshabilita en un documento nuevo); el menú «Más» de la lista exporta la
    página en vez de ofrecer un «Próximamente».
  - Login en español, con los mismos `Input`/`Feedback`/`Button` que el resto
    (la cuenta de desarrollo solo aparece en builds debug).
  - Accesibilidad de producción: `th scope="col"` en todas las tablas del
    sistema, `Skeleton` decorativo (`aria-hidden`), `Command` con
    `role="dialog" aria-modal`, foco automático al abrir, `Enter`/espacio en
    las opciones y `aria-label`/`placeholder` configurables; el selector de
    país de teléfono ya no muestra textos en inglés.
  - `ferris-web`, `ferris-web-ui` y `ferris-web-shared` declaran
    `#![forbid(unsafe_code)]`.
  - `command.rs` (634 líneas) dividido en
    `command/{mod,dialog,input,item}.rs` y `data_grid.rs` (1025) en
    `data_grid/{mod,grid,cells,header}.rs`; el payload del formulario separado
    en `form_service/payload.rs` y la lista de DocTypes para selectores
    centralizada en `views_components/doctypes.rs`.
- **Soporte móvil y ergonomía táctil**:
  - Barra inferior (`Escritorio`, `Buscar`, `Avisos`, `Menú`) en pantallas
    pequeñas; la barra lateral pasa a ser un *drawer* con fondo y cierre por
    toque, sin ocupar el ancho de la página. El rail de escritorio no cambia.
  - Preferencias por dispositivo en `localStorage`
    (`ferris.mobile.preferences`): navegación inferior, menú lateral y panel
    del formulario. Se editan en **Ajustes → General → Móvil** y se aplican al
    instante; SSR usa los valores por defecto y la hidratación aplica los
    guardados, así que no hay parpadeo de layout.
  - Formulario, listas, informes, calendario, kanban, ajustes y diálogos
    adaptados a 320–375 px: barras que envuelven o hacen scroll, columnas
    apiladas, tablas con scroll horizontal y `min-w`, selectores a ancho
    completo, kanban con columnas de 18 rem y *snap*, y diálogos limitados a
    `90dvh` con scroll interno.
  - Objetivos táctiles: botones/iconos ≥40 px en móvil, checkbox/radio con área
    de toque ampliada, tabs con scroll horizontal y barra inferior con
    `safe-area-inset`.
  - El panel de notificaciones ocupa el ancho del móvil en vez de 24 rem, y
    habla español («Avisos», «Eventos»).
- **Pasada de pulido de la UI (todo el Desk, móvil y escritorio)**:
  - **Idioma**: el shell estaba a medias en inglés. Traducidos barra superior,
    menú del módulo, menú de usuario, panel de avisos (incluidas sus notas y el
    diálogo de ajustes), selector de tema, escritorio de iconos, migas y el
    menú de vistas (`Lista`, `Calendario`, `Formulario`). `display_value` ya no
    devuelve `Yes`/`false` en celdas `Check`.
  - **Controles que no llevaban a ningún sitio**: `Sitio web` y `Ayuda` del menú
    del módulo salían de `href="#"`; ahora se construyen desde
    `Website Settings.home_page`/`Navbar Settings.help_link` y sólo aparecen si
    el valor existe. `Primeros pasos` era un `<div>` con aspecto de enlace y
    ahora abre `/settings/about`. `Mi perfil`/`Ajustes` apuntaban a secciones
    inexistentes (`/settings/profile`, `/settings/account`): ahora abren el
    formulario del propio usuario y `/settings`. Una sección de ajustes
    desconocida cae en General en vez de dejar la página en blanco.
  - **Buscador global**: el pie prometía «↑↓ navegar · Enter abrir» y no había
    ninguna de las dos cosas. Ahora hay cursor real (↑↓/Inicio/Fin/Enter),
    cierre con Esc desde cualquier foco, `role="dialog"`, etiquetas accesibles,
    estrellas como botones independientes (antes un `<span>` dentro de un
    `<button>`), tope de `85dvh` en móvil y descarte de respuestas fuera de
    orden al teclear rápido.
  - **Rutas**: los enlaces de documento y de vista no codificaban sus segmentos,
    así que un DocType con espacio (`Sales Invoice`) o un nombre con `/`/`#`
    rompía la navegación. Se centraliza en `ferris_web_shared::route::path_segment`
    y lo usan filas, formulario, kanban, calendario, dashboards y `view_modes`.
  - **Integridad de datos en tablas hijas**: un valor `Table` que no fuera un
    array se leía como «sin filas» y el siguiente guardado lo machacaba con `[]`.
    El editor ahora lo reporta sin tocarlo, y el payload omite el campo en vez de
    vaciarlo. `idx` se renumera al añadir/quitar filas.
  - **Informe**: aplicar filtros lanzaba dos peticiones idénticas (y dos juegos
    de contadores de estado). Ahora el botón incrementa un token y el `Effect`
    lee los filtros sin seguirlos. Se avisa cuando la tabla muestra sólo las
    primeras 500 filas. Kanban avisa del tope de 1000.
  - **Formularios**: las secciones multi-columna repartían los campos en
    «buckets» por columna y el grid los colocaba en otro orden (en móvil se leía
    0,2,4,1,3,5). Ahora se renderizan en orden de declaración. Las referencias
    `aria-describedby` de cada campo apuntan a su error/descripción.
  - **Móvil**: la barra superior no tenía nada a la izquierda por debajo de
    `md`; ahora muestra el título actual y un botón «Volver» al ancestro real.
    El `main` reserva `4rem + safe-area-inset-bottom` (antes 4rem fijos, que en
    iOS tapaba la última fila), el layout usa `h-dvh` y el cajón lateral cierra
    con Esc y expone `role="dialog"`. Si se apaga el menú lateral, el cuarto
    botón de la barra pasa a ser «Ajustes» para que la preferencia no se
    autobloquee.
  - **Menús y táctil**: submenús alcanzables por toque y teclado (antes sólo
    *hover*), `max-md:min-h-10` en filas de menú, popovers de submenú dentro del
    ancho del móvil, toasts con `role`/teclado y `safe-area`, inputs a
    `text-base` en móvil (sin zoom en iOS), botones de icono ≥40 px, filas de
    lista de 48 px y botones de carpeta de 32 px.
  - **Destructivo**: eliminar una carpeta del escritorio pide confirmación
    (antes se borraba con un toque en modo edición) y el formulario de carpeta
    no deja guardar un nombre vacío.
  - **Fallos silenciosos**: si `/api/ui-extensions` falla, la portada y las
    páginas de plugin muestran el error y un botón «Reintentar» en vez de un
    esqueleto eterno. Borrar asignaciones/adjuntos/compartidos ya no ignora el
    error del servidor. La exportación CSV no puede tumbar el módulo wasm
    (`expect` → retorno controlado).
- **Consistencia visual (escala única de padding, radios y tamaños)**:
  - **Página**: todas las raíces usan `p-4` y `gap-4` (Ajustes, Informe,
    Calendario, Kanban, Lista, Formulario, Escritorio, plugin y 404); las
    rejillas de tarjetas usan `gap-3` y las secciones de formulario `gap-4`.
  - **Tarjetas**: `Card` pasa a `p-4 sm:p-6` (16 px en móvil, 24 en escritorio)
    y `py-4 sm:py-6`; cabecera, contenido y pie comparten ese inset. Los KPI
    usan `CardSize::Sm`. Se eliminan los `p-6`/`p-3` que duplicaban el padding
    de la tarjeta.
  - **Radios**: superficies (`Card`, `Empty`, diálogos, panel de lista, estados
    de error) `rounded-xl`; banners `rounded-lg`; controles `rounded-md`;
    elementos de menú `rounded-sm`. El rail se dimensiona con
    `--sidebar-width`/`--sidebar-width-icon` (15rem/4rem) y la barra inferior
    con un nuevo `--bottom__nav__height` (4rem), en vez de literales repetidos.
  - **Controles**: los filtros de informe y el selector de permisos igualan la
    altura densa `h-8` de sus botones; el formulario de carpeta usa `Button`
    (variantes del catálogo) en lugar de botones con clases a mano; las
    pestañas de Ajustes son `h-8` (40 px en móvil).
  - **Tipografía**: las etiquetas de campo son `text-sm` (antes heredaban 16 px
    frente a inputs de 14), las secciones del panel del formulario comparten
    tamaño y las tarjetas de Kanban/Calendario usan `Badge` para sus contadores.
  - **Foco**: todo el chrome interactivo (sidebar, menús, barra inferior, tiles
    del escritorio, pestañas, login) usa el mismo anillo
    `ring-[3px] ring-ring/50` del catálogo; antes mezclaba `ring-2` sin color,
    `ring-ring/30` y primitivos de 3 px.
  - **Tablas**: la lista vive en una superficie `bg-card` con borde y
    `rounded-xl` (antes flotaba sobre el fondo); las tablas a sangre dentro de
    tarjetas alinean su primera/última celda con el padding de la cabecera.
  - **Formulario**: la página es una superficie de documento (`bg-card`), como
    la barra superior, en vez de quedar sobre el gris de la aplicación.
  - **Semántica de color**: los avisos de éxito de «Personalizar» usan los
    tokens `success`, no `primary`.
- **Corrección de overlays, navegación móvil y cableado de vistas**:
  - **Diálogos**: el ancho se aplicaba al wrapper y no al panel `fixed`, y un
    `md:max-w-[calc(100%-2rem)]` residual anulaba cualquier tope: se abrían casi a
    pantalla completa en escritorio. El panel tiene ahora `w-[calc(100vw-1rem)]`
    con `max-w-md` por defecto (cada llamada ajusta el suyo); el wrapper es
    `contents` (ya no añade huecos de flex). El fondo de un diálogo controlado
    recibe `pointer-events` (antes no se cerraba al tocar fuera), el botón de
    cerrar es de 40 px en móvil y todo el texto de cierre está en español.
  - **Dropdowns**: el panel reescribía todo su atributo `style` al cerrarse,
    borrando el `top`/`left`/`min-width` que calcula el posicionador: saltaba y
    colapsaba su ancho durante la transición. Ahora sólo escribe
    `pointer-events` como propiedad. Además `z-[70]`, tope de altura con scroll
    en móvil, submenús táctiles con scroll propio y filas de 40 px.
  - **Menú móvil**: el cajón no se renderizaba en `/desk` porque el *early
    return* de `bare_chrome` se lo comía — el botón «Menú» no hacía nada. Ahora
    se monta fuera de ese bloque, por encima de la barra inferior (`z-60`), con
    cabecera «Menú», botón de cierre, foco al abrir y cierre al navegar o al
    tocar un enlace. La barra inferior ocupa todo el ancho (antes flotaba con
    `max-w-lg` en tablets) y desaparece en pantalla completa.
  - **Cabecera móvil**: botón de menú en las dos barras superiores, «Volver» con
    el título actual, y las acciones hacen scroll horizontal en vez de empujar
    el título fuera de la pantalla.
  - **Buscador**: sube a `z-100` (la barra inferior ya no lo tapa), reinicia el
    cursor al abrir, estrellas con el icono del catálogo, subtítulo acotado y
    altura de diálogo adaptada al teclado.
  - **Avisos en `/desk`**: el panel se monta también ahí (el botón «Avisos» de la
    barra inferior abría un panel inexistente); sigue cerrado por defecto.
  - **Widgets**: la tabla nativa muestra el estado como badge (antes `0/1/2`) y
    la fecha como día (antes un timestamp RFC3339), con ancho mínimo legible.
  - **Huecos fantasma**: los `UiSlot` de plugins vacíos seguían dibujando una caja
    y el `gap` del contenedor contaba con ellos (formulario, barra lateral y
    list-view). Ahora se ocultan hasta que el feed declara una extensión, y la
    tabla de la lista alinea sus celdas extremas con el padding de la página.
  - **Data-driven**: los DocTypes con `in_report_view` ofrecen por fin el modo
    «Informe» en el selector de vistas (ruta `/report/:doctype`); el índice de
    dashboards no enlaza tableros sin ruta; y el sidebar descarta enlaces a
    tablas hijas (p. ej. `DocField`), que no tienen lista propia.
- **WASM: blobs privados, PKI con scope y límites de host (sin bump de WIT,
  contrato `12.0.0`)**:
  - `blobstore::put` guarda ahora **privado** y con dueño `ferris-plugin`, no
    público: conocer el `uri` ya no concede lectura. `get`, `get-range`,
    `metadata` y `signed-url` usan el contexto de capacidad del plugin (no el
    usuario ambiente ni un administrador de paso), y `delete` sólo borra lo que
    el namespace del plugin posee. Una migración idempotente marca como privadas
    las filas históricas de `blobstore/` con `is_private = 0`.
  - `pki::sign-csr` y `pki::revoke` quedan **scoped a la app que llama**
    (el nombre del plugin viaja por `HostApi`, igual que `config`): el CA emite y
    registra el fingerprint con su app, y sólo esa app puede revocarlo. Un
    fingerprint ajeno y uno inexistente responden el mismo error.
  - `http::request.timeout-ms` tiene un techo duro de 60 s, aplicado también al
    default de `FERRIS_HTTP_TIMEOUT_MS`.
  - `list-docs`/`count`/`aggregate` **fallan con `validation-failed`** cuando el
    conjunto que el host debe post-procesar supera el scan cap, en lugar de
    devolver un `total` truncado en silencio.
  - `session::can` responde `false` para una fila inexistente, igual que para
    una fila que la política deniega: deja de ser un oráculo de existencia.
  - Un `before-*` que no puede despacharse (la escritura se origina dentro de la
    propia llamada del guest, donde la instancia no se puede re-entrar) ahora
    **rechaza la escritura** con `validation-failed`; el opt-out explícito es
    `docstore::write-no-events`, que sigue saltándose los eventos del plugin.
  - `PluginStore::plugins()` devuelve las apps **ordenadas por nombre**, para que
    el registro (métodos, DocTypes) no dependa del orden de iteración de un
    `HashMap`.
- **El bundle de hidratación ya no se embebe en el binario del CLI**. `ferris
  dev` hacía `include_bytes!` de `ferris-web/pkg/*`, un directorio git-ignorado
  (`.gitignore:60`): un checkout limpio no podía compilar `ferris-cli`, y el
  binario cargaba ~32 MiB de wasm que igualmente quedaban obsoletos. Ahora se
  resuelve en disco (`FERRIS_HYDRATE_DIR`, `ferris-web/pkg`,
  `ferris-web/target/site/pkg` y el `./pkg` ya instalado) y se copia con la
  misma publicación atómica de siempre; si no hay bundle en ningún sitio, el
  Desk renderiza por SSR y el comando avisa una vez con el comando exacto para
  generarlo, en vez de fallar.
- **El reloj de epoch de WASM arranca antes de `build_router`**: el registro de
  rutas y métodos ya llama al `describe()` de cada app, y hasta ahora ese
  trabajo de arranque corría sin el límite de CPU que el resto de llamadas sí
  tenía.
- **El layout de un tablero pasa a ser global y guarda su configuración**
  (fase C4):
  - la fila `Dashboard Layout` deja de ser *por usuario*
    (`"{user}:{owner}:{dashboard}"`) y pasa a ser **una sola por tablero**
    (`"{owner}:{dashboard}"`), **propiedad del sistema**: se lee y escribe con
    el principal de servicio, así que ningún usuario "posee" el tablero y el
    segundo editor no colisiona con el primero. Editar un tablero lo edita para
    todos, que es la decisión de producto detrás del plan; el gate sigue siendo
    `Permission::Create` sobre `Dashboard Layout` y `DELETE` sigue comprobando
    `Permission::Delete`.
  - el doctype `Dashboard Layout` gana el campo `config` (`Code`, JSON de
    `DashboardConfig`: estilo del tablero y geometría) y pierde el campo `user`,
    que en una fila compartida no significaba nada.
  - `GET|PUT /api/dashboard-layout/{owner}/{dashboard}` pasan a hablar
    `DashboardLayoutPayload { widgets, config }`: el `PUT` guarda lista y
    override, y normaliza ambos (referencias irresolubles fuera, celdas
    ajustadas, números del config clampeados a los techos del contrato).
  - el endpoint document-bound (`/api/dashboard-layout/doc/{doctype}/{name}`)
    lee y escribe el `config` en el campo bindable del propio documento, cuando
    el doctype lo declara; el `Dashboard` y `Dashboard Widget` nativos ganan un
    campo `style` (`JSON`) para que un tablero escrito a mano también pueda
    declarar apariencia, igual que un plugin en `describe()`.
  - las filas guardadas con la clave interina por usuario no se leen: un
    tablero vuelve a sembrar en su primer guardado tras el cambio.
- **El host pinta la card y el título/subtítulo; el widget aporta sólo el
  componente** (cierre del cabo de C5 en la implementación de dashboard):
  - `WidgetInstance` gana `show_title`, `subtitle` y `show_subtitle` (todos
    opcionales). El host decide la cabecera: título por defecto visible
    (`label` de la instancia > `title` del widget `host.*` > etiqueta de la
    definición > referencia) y subtítulo opt-in (texto propio o, si falta, la
    `description` de la definición). El panel de ajustes edita ambos y los
    persiste con el layout compartido; el seed del WIT no cambia: la app
    declara contenido, el Desk decide el marco.
  - `views/widget_host.rs` unifica la cabecera para widgets nativos y de
    plugin: `HostStat`/`HostTrend`/`HostTable` devuelven sólo el cuerpo (sin
    `Card` ni título) y `WidgetHost` pinta el encabezado común. El cuerpo del
    plugin sigue siendo un ancla pura; la etiqueta viaja como `data-label`.
  - La celda (`ferris-dashboard-tile`) es el único marco y es siempre flex
    column: el encabezado ocupa su altura y el cuerpo es `flex-1`. Los estilos
    de la card siguen viniendo de `--ferris-tile-*`.
  - en SolmecBOX ninguna definición declara `style`: el widget no fija fondo,
    borde, relleno ni sombra; la card y el título/subtítulo son del host. Los
    widgets de panel siguen sin `.sbx-head`; los históricos conservan sólo la
    barra de acciones (Recargar, Ver más, anotar).
- **Apariencia del tablero y de las teselas en el Desk**
  (fase C5):
  - `views/dashboard/style.rs` (nuevo) es el único sitio donde se resuelve la
    precedencia: el override guardado → el estilo declarado por la app o por el
    documento nativo → el global del Desk (`GLOBAL`, todavía vacío hasta C8) para
    el tablero, y el override de la instancia → el default de su definición para
    cada widget. El resultado se emite como **propiedades CSS por elemento**
    (`--ferris-board-bg/padding/radius`, `--ferris-tile-*`), nunca como clases
    construidas en runtime: un token se convierte en `var(--token)` y un color
    libre sólo se acepta si es `#hex`, una función de color, `var(--token)` o una
    palabra clave, para que el valor guardado no pueda inyectar una segunda
    declaración en el atributo `style`. Las sombras son una escala cerrada
    (`none|xs|sm|md|lg`) con `color-mix(… var(--foreground) …)`.
  - el gap del tablero deja de ser una constante (`GAP_PX`/`ROW_STEP_PX`): la
    geometría (`tile_style`, `board_height`) y la aritmética del arrastre
    (`ActiveDrag::row_step`, capturado al iniciar el gesto) leen el mismo valor,
    así que mover una tesela la deja en la fila donde se soltó.
  - el chrome de edición (superficie y borde de la tesela, franja superior y
    asa de redimensionado) sale de utilidades de color en Rust y pasa a las
    clases `ferris-dashboard-board`, `...-tile--editing`, `...-tile__chrome` y
    `...-tile__resize` del catálogo, con sus reglas en `style/components.css`
    (sólo `var(--token)`/`color-mix`); `design_system` sigue verde y
    `style/desk.css` se regeneró. Una tesela que declara fondo, borde o sombra
    marca `data-surface` para que la `Card` interna no pinte una segunda
    superficie encima.
  - `plugin_page` y `dashboard_doc_view` propagan el estilo declarado
    (`Dashboard.style` o el campo `style` del documento) como `declared_style`;
    el override sigue llegando del endpoint de layout.
- **WIT contract `ferris:framework@10.0.0`** — nueva ola breaking: los widgets y
  los tableros ganan **apariencia declarada por la app**, sobrescribible por el
  usuario (fase C3):
  - dos records nuevos en `interface descriptor`: `style-spec` (`background`,
    `background-custom`, `border-color`, `border-width`, `border-radius`,
    `padding`, `shadow`) y `dashboard-style` (`background`,
    `background-custom`, `gap-px`, `padding-px`, `border-radius`,
    `topbar-full-width`, `title-full-width`).
  - campos nuevos: `widget-def.style: option<style-spec>` (default que la
    definición da a cada instancia), `widget-instance.style: option<style-spec>`
    (override por instancia) y `dashboard.style: option<dashboard-style>` (estilo
    del tablero).
  - **`dashboard.layout` se retira** (era `option<string>`): se parseaba y nunca
    se consumía (defecto D9 del plan), y su única función —la geometría del
    tablero— la cubren ahora `gap-px`/`padding-px` de `dashboard-style`. Sin
    código legacy: el campo desaparece del contrato, del registry, del WIT y del
    doctype `Dashboard`, y el record `dashboard` deja de aceptarlo.
  - `dashboard-format` gana `config: option<string>`, el nombre del campo del
    propio documento que guarda el override de estilo del tablero (como
    `widgets`/`layout`/`label`); a diferencia de aquellos es **opcional**: sin
    binding el documento no tiene override y el tablero usa solo el tema global.
  - **todo campo es `option` y ausente hereda del nivel inferior** (instancia →
    definición → tema del Desk), así que una app que no declara estilo alguno
    renderiza exactamente como antes. Un campo de color nombra un **token
    semántico** (`card`, `muted`, …) para que claro/oscuro y los temas de
    run-time sigan funcionando; el gemelo `*-custom` lleva un color libre que el
    Desk inyecta como variable CSS.
  - fluye por las cuatro capas del contrato: `contract::StyleSpec` /
    `dashboard-style` → `StyleSpec` / `DashboardStyle` del registry →
    records WIT (ambas copias del WIT) → `descriptor::{style_spec,dashboard_style}`
    / `ferris-api::ui::into_contract`.
  - el constructor del record cambia, de ahí el major: guest de referencia
    (`test/`) y guard `wit_parses` (`wave_10_0_0_style_fields`) actualizados, y
    las guardas de versión de las olas anteriores apuntan a la ola actual.
- **WIT contract `ferris:framework@9.0.0`** — nueva ola breaking: `widget-def`
  gana el campo opcional `preview: option<string>`, la **imagen de vista
  previa** (svg/png/webp) que el catálogo en cards del Desk dibuja en la
  cabecera/content de cada card de widget y de categoría:
  - una URL que el cliente puede cargar como `<img src>` (para los widgets
    built-in del host, `/static/previews/*`; para los de una app, el canal de
    assets propio del plugin). Ausente → el Desk pinta su placeholder
    built-in según el `kind` del widget.
  - fluye por las cuatro capas del contrato: `contract::WidgetDef` →
    `WidgetDefSpec` → record WIT `widget-def` (ambas copias del WIT) →
    `descriptor::widget_def_spec` / `into_contract`.
  - el constructor del record cambia, de ahí el major: guest de referencia
    (`test/`) y guard `wit_parses`
    (`wave_9_0_0_widget_preview`, + las guardas de versión de las olas 7/8)
    actualizados. Ver el plan de widgets (fase D1).
- **WIT contract `ferris:framework@7.0.0`** — un doctype puede declarar que
  **cada uno de sus documentos es su propio dashboard** (nuevo binding
  `dashboard-document`), sin crear ni enlazar un registro `Dashboard`:
  - nuevo record `dashboard-format { widgets, layout, label }` — los nombres
    de los campos del propio doctype que llevan cada parte del formato; cada
    uno opcional (`none`/vacío = el campo canónico), así que el mínimo se
    declara sin rodeos y cada campo puede renombrarse por separado. `route`
    **no** forma parte del formato: la ruta de un documento es su URL en el
    Desk.
  - nuevo caso `doc-view.dashboard-document(dashboard-format)`. El binding
    gana precedencia sobre el dashboard fijo y pierde ante `link_field`, que
    es la respuesta por documento.
  - el host lo proyecta sobre `DoctypeMeta.dashboard.document`
    (`DocumentFormat` en el contrato), y el Desk lee y escribe los widgets en
    el propio documento vía `GET|PUT
    /api/dashboard-layout/doc/{doctype}/{name}`: sin override por usuario
    (`customized` siempre `false`, sin `DELETE`), `editable` = permiso
    `Write` en el documento, y el guardado es una escritura normal del
    documento (versión, actividad y eventos del doctype) con la misma
    sanificación de widgets que los dashboards de app.
  - el campo `widgets` admite la lista como string JSON (campo `Code`) o
    como array; elementos rotos se omiten en vez de romper el render.
  Guest de referencia (`test/`), plantilla del CLI (`ferris new`), fixtures
  `probe-*` (recompilados) y guard `wit_parses`
  (`wave_7_0_0_document_format`) actualizados.
- **WIT contract `ferris:framework@6.0.0`** — los doctypes que una app declara
  en `describe()` eligen cómo se abren sus documentos (el *view kind* que el
  Desk ya soporta en metadatos nativos):
  - `descriptor::doctype` gana el campo requerido `view: doc-view`.
  - nueva variante `doc-view` con los cinco casos de declaración: `form`,
    `dashboard-native`, `dashboard-fixed(id)`, `dashboard-link(field)` y
    `custom`; el binding del dashboard viaja **dentro** del caso, así que no
    puede declararse junto a un view kind que lo ignoraría.
  - el host lo proyecta sobre `DoctypeMeta.view`/`dashboard`
    (`descriptor::project_view`, la única traducción, compartida por el
    registro y el espejo del endpoint `/plugins/{name}`), de modo que la
    cadena registro → `/api/doctype/meta` → Desk es la misma que la de los
    doctypes nativos.
  Guest de referencia (`test/`), plantilla del CLI (`ferris new`), fixtures
  `probe-*` (recompilados) y guard `wit_parses`
  (`wave_6_0_0_doctype_view`) actualizados.
- **WIT contract `ferris:framework@5.0.0`** — nueva ola breaking: el feed
  OpenUI y los widgets nativos pasan a llevar geometría de rejilla:
  - record `grid-size { span, rows }` — la caja de un widget, en columnas
    (`span` sigue siendo el `w` de la rejilla) y en filas de 72 px.
  - `widget-def` gana `icon`, `category`, `size: option<grid-size>` y
    `min: option<grid-size>`; `widget-param` gana `group`, que agrupa sus
    parámetros en el panel de ajustes.
  - `widget-instance` gana `col`, `row` y `rows`: la posición ya no se
    reconstruye en el navegador, se dibuja tal cual y la normalización
    (`ferris-schema/src/contract/layout.rs`) es la única regla de celdas,
    compartida por servidor y Desk.
  El host (`ferris-wasm/src/descriptor.rs::grid_size_spec`) y el guest de
  referencia (`test/src/lib.rs`) quedan adaptados;
  `ferris-wasm/wit/ferris.wit` y `test/wit/ferris.wit` quedan byte a byte
  iguales, y el guard `wave_5_0_0_grid_geometry` en
  `ferris-wasm/tests/wit_parses.rs` bloquea cualquier retroceso a la forma
  vieja.
- **WIT contract `ferris:framework@4.0.0` (segunda parte)** — cuatro funciones
  más de la petición `Ferris_request`, todavía bajo la misma ola breaking
  `@4.0.0` (contrato aún no publicado):
  - `blobstore::get-range(uri, offset, length) -> blob`: lee una ventana de
    bytes con el mismo ACL que `get`; el rango se recorta al tamaño del blob
    (`offset` al final → vacío, `length` largo → EOF). `FileStore::download_range`
    lo respalda en `ferris-files`.
  - `docstore::write-no-events(doctype, data) -> doc`: upsert idéntico a
    `upsert` pero **sin emitir `doctype-event`** — la fila se persiste y se marca
    igual, y el host no reentra en ningún handler. Rompe el bucle que un plugin
    crea al reaccionar a su propia escritura (`EngineContext::suppress_events` +
    clave `ferris.suppress-events` en `HookContext`; los demás hooks globales sí
    corren).
  - `docstore::aggregate(doctype, query, group-by, aggregates) -> list<aggregate-row>`:
    agrupa y calcula `count`/`sum`/`avg`/`min`/`max` sobre el conjunto que casan
    `filters` + `order-by` (acotado al tope de escaneo del host, como el fallback
    `ne`/`between`); sin `group-by` devuelve una fila de total. Records
    `aggregate-spec` / `aggregate-row`.
  - `handle-job(job-type, payload) -> string` (nuevo **export** del world): el
    callback que un cron/`scheduler::enqueue` necesita para reentrar al guest.
    El worker de la app reclama el job y lo enruta aquí con su `job-type` y el
    JSON `payload`; `JobDispatcher` gana un handler de respaldo (`set_fallback`)
    que el servidor instala con el `PluginStore`, y el lado plugin **no tiene
    fallback**: un job sin plugin cargado falla con ruido (reintenta, luego
    dead-letter) en vez de perderse en silencio (`PluginStore::handle_job`).
  Guest de referencia, fixtures `probe-*`, plantilla del CLI, guard `wit_parses`
  y tests actualizados. El resto de la petición (`apply`, `state`,
  `pubsub::subscribe`, `metrics`, tipado de `filter`, cursor, streaming `http`,
  watch ajeno) sigue planificado.
- **WIT contract bumped to `ferris:framework@4.0.0`** — primera ola de la
  petición `Ferris_request` (7 funciones generales de coste bajo, todas
  autocontenidas en `ferris-wasm`):
  - `docstore::get-many(doctype, names)`: lectura por lote que respeta el orden
    de `names` y omite los inexistentes (a diferencia de `get`, aquí un
    `not-found` no es error).
  - `session::can-many(checks: list<permission-check>)`: un booleano por
    consulta, en orden, sin una travesía de frontera por fila.
  - `scheduler`: `schedule` ahora devuelve el id; se añaden `cancel(schedule-id)`
    y `list-schedules()`, acotados al plugin que los registró (etiqueta
    `plugin:<name>` en la tabla compartida). `list` no es un nombre válido en
    WIT (palabra reservada), de ahí `list-schedules`.
  - `streams::stream-info` incorpora `created-at` y `bytes-received` (edad y
    presupuesto de bytes).
  - `http::request` acepta `redirect-policy` (`none`/`limited`/`follow`), con
    re-chequeo de la allow-list en cada salto.
  - `descriptor::doctype` acepta `list-default-order` (ORDER BY por defecto) y el
    descriptor declara un esquema `config` tipado (`config-item`).
  Guest de referencia, fixtures `probe-*`, plantilla del CLI y el guard
  `wit_parses` actualizados a `@4.0.0`. El resto de la petición (`handle-job`,
  `apply`, `state`, `pubsub::subscribe`, `get-range`, `metrics`, `aggregate`,
  tipado de `filter`, cursor, streaming `http`) queda planificado.
- **WIT contract bumped to `ferris:framework@3.0.0`** — widgets globales
  reutilizables con params y dashboards declarados por app: los records
  `widget-param`/`widget-def`/`widget-instance` y `dashboard` sustituyen a los
  dashboards hardcodeados de las vistas del host; el registro de UI del core
  (`ferris-core::ui_extensions`) valida nombres, rutas, `default` y referencias
  al insertar, y expone el catálogo nativo `host.*`; `GET /api/ui-extensions`
  sirve el feed con `revision`; `ui-widget(widget, params)` está tipado en el
  contrato. Guest de referencia, fixtures y plantilla compilan contra `@3.0.0`.
- Cleanup de clippy en el registro de UI, auth (MFA/CSRF/rate-limit) y los
  endpoints de sesión: `# Errors` y `#[must_use]`/`const fn` documentados,
  let-chains, sin nuevos warnings sobre el baseline.
- **Slot de UI `form.view` (vista de doctype personalizada)** — nueva ancla
  *whole-view* en el catálogo de slots: una app declara una
  `ui-extension` con `slot: "form.view"` y `doctype: "<DocType>"` y el Desk
  monta su elemento (p. ej. `<ferris-equipo-editor>`) **en vez del form**
  genérico en `/desk/:module/:doctype/:name` (y para un Single en
  `/desk/:module/:doctype`). El elemento recibe el documento seleccionado como
  `data-doc` (JSON; `null` en la ruta `new`), `data-docstatus`, los parámetros
  de ruta (`data-doctype`/`data-name`/`data-module`), el dueño
  (`data-owner`/`data-component`) y las `props` de la declaración resueltas
  contra el documento (`data-props`, `"$field"`), más un `CustomEvent`
  `ferris-document` con el mismo payload. A diferencia de los slots aditivos
  (`form.toolbar.*`, `form.before/after_fields`, …), `form.view` **reemplaza**
  la vista y **exige** nombre de doctype (una declaración sin él nunca casa, para
  que una app no sustituya todos los forms por omisión). Sin cambios de WIT:
  reutiliza el `ui_extensions` existente. `ferris-web/src/ext/doctype_view.rs`.

### Removed
- **`ferris-search`** (crate completa): no tenía ninguna dependencia ni caller y
  `ferris-api` ya servía `/api/search` con permisos. Mantener un índice que nadie
  escribía era una segunda ruta de búsqueda con ACL ausente. `Cargo.toml`,
  `ferris-search/`.
- **La URL firmada falsa de `FileStore::get_url`**: devolvía
  `?signed=true&expiry=` sin HMAC y no tenía caller; el camino real es
  `FileUrlSigner`/`SignedDownloads`. También se retiran el TUS sin montar
  (`tus.rs`, dependencia `tus`) y la fachada CDN simulada (`cdn.rs`), ambos sin
  consumidor. `ferris-files/`.
- **El cliente IMAP falso y el pipeline inbound**: `ImapClient` devolvía
  `connected: true`, un buzón vacío y `Ok(())` sin conectar; `inbound.rs` no
  parseaba MIME y sus handlers eran no-ops. Se retiran con el DocType
  `Unhandled Email` y los ajustes `enable_incoming`/`default_incoming_account`;
  los campos `imap_*` de `Email Account` quedan etiquetados como no
  implementados. `ferris-email/`.

### Fixed
- **Un email con plantilla ya no sale vacío**: `TemplateEngine::new()` registra
  los builtins bajo `<name>.html`/`<name>.txt`, `load_from_dir` usa la misma
  clave (antes el stem) y un handler sin cuerpo falla con
  `EmailError::NotFound` en vez de enviar un mensaje vacío. La sustitución es de
  una sola pasada (un valor con `{{ otra }}` ya no se expande) y un placeholder
  desconocido queda visible. `ferris-email/src/{templates,handlers}.rs`.
- **HMAC de descargas en tiempo constante y coherencia de `trim`**:
  `verify_path` usa `Mac::verify_slice` (la comparación anterior salía en el
  primer byte distinto) y `FileUrlSigner` normaliza el id **antes** de firmarlo
  y de emitirlo, así que un `/files//abc` ya verifica. `ferris-files/src/signed_url.rs`.
- **Checksum real en `upload`**: el SHA-256 se calcula sobre los bytes escritos y
  se persiste; un checksum declarado que no coincide se rechaza. Antes se
  guardaba el valor del cliente sin verificar. `ferris-files/src/store.rs`.
- **`get_files_by_field` vuelve a encontrar adjuntos**: el prefiltro `LIKE`
  exigía la subcadena `attached_to`, que el JSON de `Vec<DocRef>` nunca
  contiene, así que devolvía siempre vacío. `ferris-files/src/reference.rs`.
- **`FileStore::list` aplica ACL y escapa el `LIKE`**: ya no expone metadatos de
  ficheros privados de otro usuario ni trata `%`/`_` de la carpeta como
  comodines. `ferris-files/src/store.rs`.
- **`GET_LOCK` de MySQL se comprueba** (H11): 0/NULL es un error
  (`DatabaseError::Migration`), no "lock adquirido", y el lock se libera tanto
  si el lote termina bien como si falla (antes un `?` dejaba la sesión del pool
  con el lock tomado). `ferris-db/src/migration.rs`.
- **Un recordatorio con envío fallido queda `sent=false`** (H13): no cuenta como
  entrega, no se marca enviado y el siguiente barrido lo reintenta; un canal
  `notify` desconocido también se deja pendiente en vez de caer en
  notificación. `ferris-integrations/src/runtime/reminder.rs`.
- **El bundle de hidratación ya no rompe con `LinkError` en dev** — `ferris dev`
  servía `/pkg` con el cacheo por omisión del navegador, así que un
  `ferris-web.js` de una build anterior podía acabar emparejado con el wasm
  recién construido; la primera importación de `web-sys` que añadía el tick de
  refresco del dashboard (`setInterval`/`clearInterval`) abortaba la página con
  `LinkError: import object field '__wbg_clearInterval_…' is not a Function`. El
  rol web responde ahora `/pkg` con `Cache-Control: no-store` cuando
  `ServerSettings::dev` está activo (el binario `ferris-web-ssr` siempre, que
  solo existe en dev), y el CLI publica el pegamento y el wasm como una sola
  unidad, de forma que `./pkg` no puede contener ficheros de dos builds
  distintas.

## [0.1.0] - 2026-10-07

### Added
- Initial release
