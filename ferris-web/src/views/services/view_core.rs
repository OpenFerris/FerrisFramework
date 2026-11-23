//! Núcleo común de vistas (list, kanban, calendar, ...).
//!
//! Ninguna vista "hereda": componen un `ViewCore` y añaden su propio estado.
//! El trait `ViewService` da comportamiento compartido sin `Deref`.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::hooks::{
    Generation, use_infinite_list_data, use_list_data, use_persisted_view_state,
};
use crate::api::meta::{Column, record_name};
use crate::api::{DocStore, Filter, ListParams};
use crate::components::constants::pagination::PAGINATION;

mod bulk;
mod service;

pub use bulk::BulkProgress;
pub use service::ViewService;

// ────────────────────────────────────────────────────────────────────────────
// Query neutra — todas las vistas hablan este idioma.
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewQuery {
    pub filters: Vec<Filter>,
    /// Filters edited by the quick rows outside the advanced dropdown. Kept
    /// separate so applying the dropdown never replaces them; both are merged
    /// in [`Self::to_list_params`].
    #[serde(default)]
    pub quick_filters: Vec<Filter>,
    pub search: String,
    pub order_by: Option<String>,
    pub limit_start: u64,
    pub limit_page_length: u64,
    pub fields: Option<Vec<String>>,
    /// Show only the documents the user liked (the header heart).
    #[serde(default)]
    pub only_liked: bool,
    /// Campos propios de cada tipo de vista (group_by, date_from, ...).
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

impl ViewQuery {
    pub fn to_list_params(&self) -> ListParams {
        let active: Vec<(String, String, String, bool)> = self
            .filters
            .iter()
            .chain(self.quick_filters.iter())
            .map(Filter::tuple)
            .collect();
        ListParams {
            limit_start: self.limit_start,
            limit_page_length: self.limit_page_length,
            filters: if active.is_empty() {
                None
            } else {
                serde_json::to_string(&active).ok()
            },
            order_by: self.order_by.clone(),
            fields: self.fields.clone(),
            search: (!self.search.trim().is_empty()).then(|| self.search.clone()),
            only_liked: self.only_liked.then_some(true),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Persistencia
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedView {
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub search: String,
    /// Estado específico de la vista (order_by, page_size, columnas, ...).
    #[serde(default)]
    pub extra: Value,
}

fn view_key(module: &str, doctype: &str, view_id: &str) -> String {
    format!("ferris.view.{}.{}.{}", module, doctype, view_id)
}

// ────────────────────────────────────────────────────────────────────────────
// ViewCore
// ────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ViewCore {
    // Identidad — signals, no valores. El router no reconstruye la vista cuando
    // sólo cambian los params, así que capturarlos en la
    // construcción dejaba la lista mostrando el doctype anterior bajo la URL del
    // nuevo: mismas filas, columnas y filtros de otro doctype.
    pub doctype: Signal<String>,
    pub module: Signal<String>,
    pub view_id: &'static str,
    /// The catalogue handle for messages built inside the service (bulk
    /// deletion reports), where no component owner exists.
    pub i18n: crate::i18n::I18n,

    // Metadata
    pub meta: RwSignal<Option<crate::api::DynMeta>>,

    // Query — la vista concreta escribe aquí para pedir datos.
    pub query: RwSignal<ViewQuery>,

    // Datos
    pub rows: ReadSignal<Vec<Value>>,
    pub total: ReadSignal<u64>,
    pub loading: ReadSignal<bool>,
    /// Another page is being appended. Always `false` outside an infinite view.
    pub loading_more: Signal<bool>,
    /// The loaded rows are still a prefix of the server's result set.
    pub has_more: Signal<bool>,
    pub error: ReadSignal<Option<String>>,
    /// Fetch the next page into `rows`. A no-op outside an infinite view.
    pub load_more: Callback<()>,

    // Selección
    pub selected: RwSignal<HashSet<String>>,
    pub selected_count: Memo<usize>,
    pub page_names: Memo<Vec<String>>,
    pub all_page_checked: Memo<bool>,

    // Refresco
    pub reload: Callback<()>,

    // Acciones masivas
    /// Pide confirmación. No borra nada por sí mismo: el borrado es
    /// irreversible y una fila de menú a la que se llega rápido no es un
    /// consentimiento.
    pub bulk_delete: Callback<()>,
    /// Ejecuta el borrado que el usuario acaba de confirmar.
    pub bulk_delete_confirm: Callback<()>,
    /// Cierra la confirmación sin borrar.
    pub bulk_cancel: Callback<()>,
    /// `true` mientras la confirmación está en pantalla.
    pub bulk_confirm: RwSignal<bool>,
    pub bulk_progress: RwSignal<BulkProgress>,
    /// El informe de fallos del último borrado, listo para el banner.
    pub bulk_failure: Memo<Option<String>>,
    pub bulk_export: Callback<Vec<Column>>,
    pub export_page: Callback<Vec<Column>>,

    // Persistencia (filtros + search + extra)
    pub saved: RwSignal<SavedView>,
    /// Clave de almacenamiento de `saved`. Reactiva por lo mismo que `doctype`:
    /// con una clave fija, filtrar una lista escribía los filtros en la entrada
    /// de otro doctype.
    pub view_key: Signal<String>,
}

impl ViewCore {
    pub fn new(doctype: Signal<String>, module: Signal<String>, view_id: &'static str) -> Self {
        Self::new_with(doctype, module, view_id, |_, _| {})
    }

    /// Build the core with the view's persisted state already applied to the
    /// first query.
    ///
    /// The old order created the query with defaults, started `use_list_data`,
    /// and only then ran the effects that restored filters and wrote the list's
    /// `order_by`/page size into it. Every first visit therefore made two `/list`
    /// requests: one with the default query and one with the real one. The
    /// configuration closure runs synchronously, before the fetch effect, so the
    /// first request already carries the restored state.
    pub fn new_with(
        doctype: Signal<String>,
        module: Signal<String>,
        view_id: &'static str,
        configure: impl FnOnce(&mut ViewQuery, &SavedView),
    ) -> Self {
        Self::new_with_mode(doctype, module, view_id, configure, false)
    }

    /// Same as [`Self::new_with`], but the data hook appends pages instead of
    /// replacing the rows. The list view uses this for infinite scrolling.
    pub fn new_infinite_with(
        doctype: Signal<String>,
        module: Signal<String>,
        view_id: &'static str,
        configure: impl FnOnce(&mut ViewQuery, &SavedView),
    ) -> Self {
        Self::new_with_mode(doctype, module, view_id, configure, true)
    }

    fn new_with_mode(
        doctype: Signal<String>,
        module: Signal<String>,
        view_id: &'static str,
        configure: impl FnOnce(&mut ViewQuery, &SavedView),
        infinite: bool,
    ) -> Self {
        // ── Señales base ────────────────────────────────────────────────
        let i18n = crate::i18n::use_i18n();
        let meta = RwSignal::new(None::<crate::api::DynMeta>);
        let selected = RwSignal::new(HashSet::<String>::new());
        let reload_trigger = RwSignal::new(0u64);

        // ── Cargar metadatos ────────────────────────────────────────────
        // La lectura de `doctype` es lo que inscribe este efecto: cambiar de
        // doctype recarga los metadatos. Como dos peticiones pueden solaparse, cada
        // toma una generación y sólo escribe si sigue siendo la más reciente —
        // si no, los metadatos del doctype anterior aterrizarían sobre el nuevo.
        {
            let generation = Generation::default();
            Effect::new(move |_| {
                let dt = doctype.get();
                if dt.is_empty() {
                    meta.set(None);
                    return;
                }
                let mine = generation.claim();
                let guard = generation.clone();
                spawn_local(async move {
                    match DocStore::runtime() {
                        Ok(store) => match store.meta(&dt).await {
                            Ok(m) => {
                                if guard.is_current(mine) {
                                    meta.set(m);
                                }
                            }
                            Err(e) => {
                                if guard.is_current(mine) {
                                    meta.set(None);
                                }
                                leptos::logging::error!("meta: {e}");
                            }
                        },
                        Err(e) => leptos::logging::error!("runtime: {e}"),
                    }
                });
            });
        }

        // ── Persistencia ────────────────────────────────────────────────
        // El signal se inicializa desde localStorage y se vuelve a escribir en
        // cada cambio; el guardado es automático.
        //
        // La clave se deriva de los params, así que también es reactiva: una
        // clave fija metía los filtros del doctype A en la entrada del doctype
        // B en cuanto dos doctypes compartieran vista.
        let view_key = Signal::derive(move || view_key(&module.get(), &doctype.get(), view_id));
        let saved = use_persisted_view_state::<SavedView>(view_key);

        // Seed the first query from localStorage synchronously. The hook above
        // has already read the entry for this key; doing this here (instead of
        // in an effect) is what keeps the first fetch from being unfiltered.
        let seeded = saved.get_untracked();
        let mut initial_query = ViewQuery {
            limit_page_length: PAGINATION::DEFAULT_PAGE_SIZE as u64,
            filters: seeded.filters.clone(),
            search: seeded.search.clone(),
            ..Default::default()
        };
        configure(&mut initial_query, &seeded);
        let query = RwSignal::new(initial_query);

        // ── Query → ListParams → fetch ─────────────────────────────────
        let params = Memo::new(move |_| query.get().to_list_params());
        let (rows, total, loading, loading_more, has_more, error, load_more) = if infinite {
            let data = use_infinite_list_data(
                move || doctype.get(),
                move || params.get(),
                move || reload_trigger.get(),
            );
            (
                data.rows,
                data.total,
                data.loading,
                data.loading_more,
                data.has_more,
                data.error,
                data.load_more,
            )
        } else {
            let (rows, total, loading, error) = use_list_data(
                move || doctype.get(),
                move || params.get(),
                move || reload_trigger.get(),
            );
            (
                rows,
                total,
                loading,
                Signal::derive(|| false),
                Signal::derive(|| false),
                error,
                Callback::new(|_: ()| {}),
            )
        };

        // ── Derivados ───────────────────────────────────────────────────
        let page_names = Memo::new(move |_| {
            rows.get()
                .iter()
                .map(record_name)
                .filter(|n| !n.is_empty())
                .collect::<Vec<_>>()
        });

        let selected_count = Memo::new(move |_| selected.get().len());

        // ── Selection is scoped to what the query shows ──────────────────
        // Checking rows and then filtering/sorting/searching used to keep the
        // previous checkmarks: "Delete N" then ran against documents that were
        // no longer on screen. A new query (or doctype) starts with nothing
        // selected, and the visible set is the authority from then on.
        {
            let applied = RwSignal::new(Some((doctype.get_untracked(), query.get_untracked())));
            Effect::new(move |_| {
                let key = (doctype.get(), query.get());
                if applied.get_untracked().as_ref() == Some(&key) {
                    return;
                }
                applied.set(Some(key));
                selected.set(HashSet::new());
            });
        }

        // The query cleared the selection for the rows it is about to replace;
        // this prunes anything that disappears for another reason (a reload
        // after an edit, a shorter page). Reading `selected` untracked keeps
        // the effect from re-running on its own write.
        Effect::new(move |_| {
            let visible: HashSet<String> = rows
                .get()
                .iter()
                .map(record_name)
                .filter(|name| !name.is_empty())
                .collect();
            let stale: Vec<String> = selected
                .get_untracked()
                .into_iter()
                .filter(|name| !visible.contains(name))
                .collect();
            if !stale.is_empty() {
                selected.update(|names| {
                    for name in &stale {
                        names.remove(name);
                    }
                });
            }
        });

        let all_page_checked = Memo::new(move |_| {
            let names = page_names.get();
            if names.is_empty() {
                return false;
            }
            let sel = selected.get();
            names.iter().all(|n| sel.contains(n))
        });

        let reload = Callback::new(move |_| reload_trigger.update(|n| *n += 1));

        // Restaurar filtros + search al cambiar de clave (no en la primera
        // pasada: `initial_query` ya nació con ellos). Sin este guard, el
        // `RwSignal::update` de la primera ejecución invalidaba la query recién
        // sembrada y provocaba un segundo `/list` idéntico.
        {
            let applied = RwSignal::new(Some(view_key.get_untracked()));
            Effect::new(move |_| {
                let key = view_key.get();
                if applied.get_untracked().as_deref() == Some(key.as_str()) {
                    return;
                }
                applied.set(Some(key));
                let s = saved.get_untracked();
                query.update(|q| {
                    q.filters = s.filters.clone();
                    q.search = s.search.clone();
                });
            });
        }

        // Sincronizar query → saved (filtros y search).
        Effect::new(move |_| {
            let q = query.get();
            saved.update(|s| {
                if s.filters != q.filters || s.search != q.search {
                    s.filters = q.filters.clone();
                    s.search = q.search.clone();
                }
            });
        });

        // ── Acciones masivas ────────────────────────────────────────────
        // El doctype se lee al ejecutar, no se captura: los botones viven más que
        // un doctype, y borrar con el nombre del anterior no borraba nada
        // (ni el documento que el usuario había marcado).
        let bulk_confirm = RwSignal::new(false);
        let bulk_progress = RwSignal::new(BulkProgress::default());

        let bulk_delete = {
            Callback::new(move |_| {
                if selected.get_untracked().is_empty() || bulk_progress.get_untracked().running {
                    return;
                }
                // El aviso de un intento anterior se limpia al pedir otro: si no,
                // volvería a salir detrás del diálogo, persuasivo y equivocado.
                bulk_progress.set(BulkProgress::default());
                bulk_confirm.set(true);
            })
        };

        let bulk_cancel = { Callback::new(move |_| bulk_confirm.set(false)) };

        // El borrado, uno a uno y con el resultado a la vista.
        //
        // Secuencial a propósito. En paralelo se necesita un contador y una
        // retrospectiva; en serie, cada respuesta es atribuible a su documento y
        // el bucle no puede perder el motivo de un fallo.
        let bulk_delete_confirm = {
            Callback::new(move |_| {
                if bulk_progress.get_untracked().running {
                    return;
                }
                // Second gate: only the rows still loaded can be deleted. The
                // pruning effect above normally leaves nothing else, but the
                // dialog is a separate owner from the table and a reload that
                // lands between opening it and confirming must not widen the
                // operation to documents the user cannot see.
                let visible: HashSet<String> = page_names.get_untracked().into_iter().collect();
                let mut names = visible_names(&selected.get_untracked(), &visible);
                // `HashSet` has no order: without this, two attempts over the
                // same selection advance in different orders and the warning
                // from the first does not match the second.
                names.sort();
                if names.is_empty() {
                    return;
                }
                let dt = doctype.get();
                bulk_confirm.set(false);
                let total = names.len();
                bulk_progress.set(BulkProgress {
                    total,
                    running: true,
                    ..Default::default()
                });

                spawn_local(async move {
                    let mut failed: Vec<(String, String)> = Vec::new();
                    for name in &names {
                        let store = match DocStore::runtime() {
                            Ok(store) => store,
                            Err(e) => {
                                // Sin almacén no hay ni un borrado posible. No se
                                // intentó ninguno, así que no se marca ni uno como
                                // fallido: se dice que no se pudo, que es distinto.
                                bulk_progress.update(|p| {
                                    p.fatal = Some(e);
                                    p.done = 0;
                                    p.running = false;
                                });
                                return;
                            }
                        };
                        if let Err(reason) = store.delete(&dt, name).await {
                            failed.push((name.clone(), reason));
                        }
                        bulk_progress.update(|p| p.done += 1);
                    }

                    // Lo que falló sigue seleccionado: es exactamente lo que queda
                    // por borrar, así que un reintento posterior apunta a los
                    // documentos vivos y no a los que ya no existen.
                    selected.set(failed.iter().map(|(n, _)| n.clone()).collect());
                    bulk_progress.update(|p| {
                        p.failed = failed;
                        p.running = false;
                    });
                    reload_trigger.update(|n| *n += 1);
                });
            })
        };

        let bulk_failure = { Memo::new(move |_| bulk_progress.get().failure_report(&i18n)) };

        let bulk_export = {
            Callback::new(move |cols: Vec<Column>| {
                let sel = selected.get_untracked();
                let chosen: Vec<Value> = rows
                    .get_untracked()
                    .into_iter()
                    .filter(|r| sel.contains(&record_name(r)))
                    .collect();
                crate::api::export_csv(&doctype.get(), &cols, &chosen);
            })
        };

        let export_page = {
            Callback::new(move |cols: Vec<Column>| {
                let out = rows.get_untracked();
                crate::api::export_csv(&doctype.get(), &cols, &out);
            })
        };

        Self {
            doctype,
            module,
            view_id,
            i18n,
            meta,
            query,
            rows,
            total,
            loading,
            loading_more,
            has_more,
            error,
            load_more,
            selected,
            selected_count,
            page_names,
            all_page_checked,
            reload,
            bulk_delete,
            bulk_delete_confirm,
            bulk_cancel,
            bulk_confirm,
            bulk_progress,
            bulk_failure,
            bulk_export,
            export_page,
            saved,
            view_key,
        }
    }

    // ── Query ───────────────────────────────────────────────────────────

    pub fn set_filters(&self, filters: Vec<Filter>) {
        self.query.update(|q| q.filters = filters);
        self.reload.run(());
    }

    pub fn set_search(&self, term: String) {
        self.query.update(|q| q.search = term);
        self.reload.run(());
    }

    pub fn set_order(&self, order: Option<String>) {
        self.query.update(|q| q.order_by = order);
        self.reload.run(());
    }

    pub fn set_pagination(&self, start: u64, len: u64) {
        self.query.update(|q| {
            q.limit_start = start;
            q.limit_page_length = len;
        });
    }

    pub fn set_extra(&self, key: &str, value: String) {
        self.query.update(|q| {
            q.extra.insert(key.to_string(), value);
        });
    }

    // ── Selección ───────────────────────────────────────────────────────

    pub fn toggle_row(&self, name: String, on: bool) {
        self.selected.update(|s| {
            if on {
                s.insert(name);
            } else {
                s.remove(&name);
            }
        });
    }

    pub fn toggle_all_on_page(&self, on: bool) {
        let names = self.page_names.get_untracked();
        self.selected.update(|s| {
            for n in names {
                if on {
                    s.insert(n);
                } else {
                    s.remove(&n);
                }
            }
        });
    }

    pub fn clear_selection(&self) {
        self.selected.set(HashSet::new());
    }

    pub fn selected_names(&self) -> Vec<String> {
        self.selected.get_untracked().into_iter().collect()
    }

    // ── Persistencia específica de la vista ─────────────────────────────

    /// Guarda el `extra` propio de la vista (order_by, page_size, etc.).
    pub fn persist_extra(&self, extra: Value) {
        self.saved.update(|s| s.extra = extra);
    }

    /// Recupera el `extra` guardado por la vista.
    pub fn restored_extra(&self) -> Value {
        self.saved.get_untracked().extra
    }
}

/// The selected names that are still part of the loaded rows, sorted.
///
/// Deleting acts on a set that may have been gathered across query changes; the
/// rows on screen are the only documents the user can still see, so they are the
/// only ones a confirmed bulk action may touch.
fn visible_names(selected: &HashSet<String>, visible: &HashSet<String>) -> Vec<String> {
    let mut names: Vec<String> = selected
        .iter()
        .filter(|name| visible.contains(*name))
        .cloned()
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::visible_names;
    use std::collections::HashSet;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    /// Select two rows, filter the query down to one of them, then delete: the
    /// document that left the screen must not be part of the confirmed action.
    #[test]
    fn a_filtered_out_selection_is_not_deleted() {
        let selected = set(&["TASK-1", "TASK-2"]);
        let visible_after_filter = set(&["TASK-1"]);
        assert_eq!(
            visible_names(&selected, &visible_after_filter),
            vec!["TASK-1".to_string()]
        );
    }

    /// A query that matches nothing deletes nothing, even if the checkmarks
    /// have not been repainted yet.
    #[test]
    fn an_empty_result_set_deletes_nothing() {
        let selected = set(&["TASK-1", "TASK-2"]);
        assert!(visible_names(&selected, &HashSet::new()).is_empty());
    }

    #[test]
    fn an_unfiltered_selection_is_kept_in_a_stable_order() {
        let selected = set(&["TASK-2", "TASK-1"]);
        let visible = set(&["TASK-1", "TASK-2"]);
        assert_eq!(
            visible_names(&selected, &visible),
            vec!["TASK-1".to_string(), "TASK-2".to_string()]
        );
    }
}
