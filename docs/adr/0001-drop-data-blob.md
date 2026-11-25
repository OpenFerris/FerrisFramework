# ADR 0001 — Retirar el blob `data` de las tablas de documento

Estado: **aceptada y ejecutada** (greenfield, sin migración de bases antiguas).
Ámbito: `ferris-db`, `ferris-schema`, `ferris-document`, `ferris-api`, `ferris-wasm`.

## Resolución

La retirada se ejecutó como un corte limpio, no como una migración por fases:

- Las tablas `tab<DocType>` ya **no tienen** columna `data`. Una base creada con
  el blob no se migra: no hay legado que preservar, se recrea el esquema.
- `ferris migrate` ya no rellena columnas desde JSON; la migración genera el
  esquema nativo directamente. La pasada de *backfill* se eliminó junto con el
  blob.
- Los snapshots históricos siguen en JSON: `tab_version.data` y la lápida de
  `Deleted Document` guardan el documento reconstruido como cuerpo JSON. Los
  campos declarados `JSON` conservan su columna JSON. Esos son datos, no el
  espejo del estado vivo.

## Contexto

Cada `tab<DocType>` guardaba el documento completo en una columna `data`
(JSON) **y** una columna por campo declarado. Las columnas eran la fuente de
verdad para filtros, índices y tipos nativos; el blob era el espejo que
sostenía:

- hooks y eventos (`DocumentHook` recibe un `Value`),
- snapshots de `tab_version`,
- la API de datos de los plugins WASM,
- import/export y el motor de informes,
- campos que aún no tienen columna (custom fields, tipos sin mapping),
- filas anteriores al backfill.

Mantener los dos escribe el doble, parsea JSON en cada lectura y permite que
columna y blob discrepen. Ya no hay requisito de compatibilidad: se puede
eliminar el blob de las tablas de documento.

## Decisión

Las columnas son la **única** fuente de verdad de un documento. El blob
desaparece de `tab<DocType>`; se conserva JSON solo donde es un dato en sí
(`tab_version.data` como snapshot histórico, y campos `JSON` declarados).

### Modelo destino

- Columnas de sistema: `name`, `owner`, `docstatus`, `creation`, `modified`,
  `modified_by`, `amended_from`, `workflow_state`.
- Una columna por campo declarado, incluidos los **custom fields** (se leen del
  meta efectivo, no solo del registro compilado).
- Las tablas hijas ya son reales; `parent`, `parenttype`, `parentfield`, `idx`
  siguen siendo columnas.
- Los campos `Table` no tienen columna propia: su documento se reconstruye
  desde la tabla hija.
- Los campos virtuales no se persisten.
- Un campo que no sea columna ni tabla hija es un error de escritura; ya no se
  acepta por el blob.

### Lectura y escritura

- `read_row_via` deja de seleccionar `data`; construye el `Value` desde las
  columnas y las tablas hijas.
- `insert_row` / `persist_written` escriben solo columnas; el `Value` que
  reciben hooks y plugins se reconstruye en memoria.
- `tab_version` sigue guardando un snapshot JSON del documento reconstruido:
  es historia, no el estado vivo.

## Fases

El corte se hizo de una vez sobre un esquema greenfield; las fases se listan como
registro de lo que se sustituyó. La fase 5 (`DROP COLUMN` sobre bases con blob)
no se ejecuta: no hay bases antiguas que migrar.

1. **Inventario y columnas de sistema.** Añadir `modified_by`,
   `amended_from`, `workflow_state`; poblar desde el blob; tests en los tres
   motores.
2. **Meta efectivo.** `stored_fields` y la creación de columnas usan
   `dyn_meta` (compilado + `Custom Field` + `Property Setter`), no solo
   `get_doctype`. Sin esto, un custom field perdería su valor al quitar el blob.
3. **Lectura columnar.** Reconstruir el documento desde columnas/hijas;
   comparar con el blob en tests (modo dual-read) durante una fase.
4. **Escritura columnar.** Dejar de escribir campos declarados en `data`;
   hooks/plugins/versiones leen la reconstrucción. El blob queda vacío o solo
   con lo que aún no tenga columna.
5. **DDL.** Quitar `data` de `document_columns` y de las tablas existentes:
   `ALTER TABLE ... DROP COLUMN` en PG/MySQL y rebuild en SQLite si la versión
   no lo soporta. El runner deja de emitir la columna.
6. **Limpieza.** Eliminar el residual JSON de `list_sql`, los índices de
   expresión y el fallback de `hydrate`; `search` pasa a FTS.

Cada fase se validó en SQLite, PostgreSQL y MySQL. Al ser greenfield no hubo
ventana de doble lectura ni backup: se partió de un esquema sin blob, sin
`ALTER TABLE ... DROP COLUMN` y sin fallback de lectura.

## Consecuencias

- Menos I/O por escritura y menos parseo por lectura; los índices y tipos
  nativos cubren todo el documento.
- `DocTypeStore` deja de serializar `DoctypeMeta` en `data`: sus campos pasan a
  columnas (o a un campo `JSON` declarado).
- Los hooks que hoy mutan `data` de forma no declarada dejan de compilar; es
  intencional.
- Un DocType con un campo no columnar falla al escribir en vez de degradar a
  blob.

## Lo que no entra

- `tab_version.data` y la lápida de `Deleted Document`: son snapshots
  históricos, no el estado vivo.
- Campos declarados `JSON`: su columna sigue siendo JSON.
- Compatibilidad con bases que aún tengan el blob: no se migran. No hay legado;
  una base antigua se recrea con el esquema columnar.
