//! Metadata-driven form view.
//!
//! The view is a full-size, Frappe-shaped form: a top bar with breadcrumbs and
//! a status badge (and the mode-dependent actions), the tabbed fields, the
//! static right sidebar and the comment thread plus activity timeline at the end
//! of the active tab. All of it is composed from `components::views::form::*`
//! and driven by [`FormService`], so the view layer only arranges.
//!
//! It is also the route's dispatcher for the two kinds of document view a
//! doctype can *declare* instead of the form (`ferris_schema::contract::
//! DocView`): [`crate::views::DashboardDocView`] for `dashboard`, and an
//! explicit refusal for `custom` when no app answers it. An app's own
//! `form.view` element wins before any of this — [`FormView`] mounts it in
//! place of this component — so the declaration and the app cannot both be
//! obeyed, and the app is the more specific of the two.

mod helpers;

use helpers::{FormPhase, custom_view_missing, opens_as_dashboard};

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use ferris_schema::contract::DocView;

use crate::app::use_mobile_prefs;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::components::ui::skeleton::Skeleton;
use crate::components::views::common::ListErrorBanner;
use crate::components::views::form::edit_mode::CreateFieldDialog;
use crate::components::views::form::field_renderer::render_form_field;
use crate::components::views::form::field_types::FormDocContext;
use crate::components::views::form::{
    FieldEditCtx, FormEngagement, FormSidebar, FormTabs, FormTopBar, FormTourLauncher,
    InspectorPanel,
};
use crate::ext::{
    DoctypeViewHost, SLOT_FORM_BEFORE_FIELDS, SLOT_FORM_SIDEBAR_BOTTOM, SLOT_FORM_SIDEBAR_TOP,
    UiCtx, UiExtension, UiSlot,
};
use crate::i18n::use_i18n;
use crate::views::DashboardDocView;
use crate::views::services::FormService;
use crate::views::views_components::{Feedback, FeedbackKind};

/// The `/desk/:module/:doctype/:name` entry point (and the Single form the list
/// view renders): an app that declared a [`SLOT_FORM_VIEW`] for this doctype
/// owns the body, and every other doctype keeps the built-in metadata form.
///
/// The decision is a signal over the feed, which is empty on the server and on
/// the client's first pass: SSR and the hydration pass both render
/// [`MetadataFormView`], and the swap to the app's element happens only once
/// the feed lands — the same deterministic first paint every feed-driven
/// surface in the Desk has.
///
/// [`SLOT_FORM_VIEW`]: crate::ext::SLOT_FORM_VIEW
#[component]
pub fn FormView() -> impl IntoView {
    let params = use_params_map();
    // Params as memos (see `MetadataFormView`): a derived signal evaluates its
    // body where it is built, outside a reactive scope.
    let doctype: Signal<String> = Signal::from(Memo::new(move |_| {
        params.with(|p| p.get("doctype").unwrap_or_default())
    }));
    let module: Signal<String> = Signal::from(Memo::new(move |_| {
        params.with(|p| p.get("module").unwrap_or_default())
    }));
    // Same rule as the form itself: a missing `:name` is a Single, so its
    // document is named after the doctype; a present `new`/`<name>` is verbatim.
    let docname: Signal<String> = Signal::from(Memo::new(move |_| {
        let name = params.with(|p| p.get("name").unwrap_or_default());
        if name.is_empty() {
            params.with(|p| p.get("doctype").unwrap_or_default())
        } else {
            name
        }
    }));

    let ui_ctx = use_context::<UiCtx>();
    // A memo, for the same reason as the params above: a derived signal evaluates
    // its body where it is built, and reading the `doctype` memo there warns.
    let declared: Signal<Option<(String, UiExtension)>> = Signal::from(Memo::new(move |_| {
        let ctx = ui_ctx?;
        let dt = doctype.get();
        (!dt.is_empty()).then(|| ctx.form_view_for(&dt)).flatten()
    }));
    // The feed lands after the first paint, and a derived signal is not a
    // render gate on its own: every `apps` write can re-run this view even when
    // the doctype still has no `form.view`. Rebuilding the branch disposed and
    // recreated `MetadataFormView`, so the whole form loaded twice (meta,
    // document, sidebar and engagement). A `Memo<bool>` only notifies when the
    // choice actually changes, so the built-in form survives the feed update.
    let uses_metadata_form = Memo::new(move |_| declared.get().is_none());

    view! {
        {move || {
            if uses_metadata_form.get() {
                view! { <MetadataFormView /> }.into_any()
            } else {
                view! {
                    <DoctypeViewHost
                        spec=declared
                        doctype=doctype
                        module=module
                        docname=docname
                    />
                }
                .into_any()
            }
        }}
    }
}

#[component]
fn MetadataFormView() -> impl IntoView {
    let params = use_params_map();
    let i18n = use_i18n();
    let i18n_sv = i18n;
    let tr = move |key: &str| {
        i18n_sv
            .try_with_value(|i| i.t(key))
            .unwrap_or_else(|| key.to_string())
    };
    // Signals, not values: the router does not rebuild this component when only
    // the params change, so the service has to re-read them
    // to follow `/…/TASK00001` → `/…/TASK00002`.
    // `Memo`, not `Signal::derive`: a derived signal evaluates its body as soon
    // as it is built, which happens here in the component body (no reactive
    // context) and made Leptos warn on every form render. A memo reads the
    // router's params inside its own reactive scope.
    let doctype: Signal<String> = Signal::from(Memo::new(move |_| {
        params.with(|p| p.get("doctype").unwrap_or_default())
    }));
    let module: Signal<String> = Signal::from(Memo::new(move |_| {
        params.with(|p| p.get("module").unwrap_or_default())
    }));
    // A Single is reached at `/desk/:module/:doctype` with no `:name`; its one
    // document is named after the doctype (`DoctypeMeta::single_name`), so the
    // missing param means "the single", not "the new form". A present `new`/
    // `<name>` (the ordinary form route) is used verbatim.
    let docname: Signal<String> = Signal::from(Memo::new(move |_| {
        let name = params.with(|p| p.get("name").unwrap_or_default());
        if name.is_empty() {
            params.with(|p| p.get("doctype").unwrap_or_default())
        } else {
            name
        }
    }));

    // Create the form service.
    let service = StoredValue::new(FormService::new(
        doctype.into(),
        module.into(),
        docname.into(),
    ));
    let svc = service.get_value();
    let svc_for_view = svc.clone();
    // The engagement panel gets its own handle: it must live inside the `Show`
    // below without borrowing the one the branch closure already captured.
    let engagement_service = svc.clone();
    // OpenUI slot (`form.sidebar.*`): the plugin sidebar column only renders
    // while the feed declares one of its slots for this doctype. `use_context`
    // (not `expect_context`): standalone renders in tests simply get no sidebar.
    let ui_ctx = use_context::<UiCtx>();
    let sidebar_declared = Memo::new(move |_| {
        let Some(ctx) = ui_ctx else {
            return false;
        };
        let dt = doctype.get();
        let current = (!dt.is_empty()).then_some(dt.as_str());
        !ctx.extensions_for(SLOT_FORM_SIDEBAR_TOP, current)
            .is_empty()
            || !ctx
                .extensions_for(SLOT_FORM_SIDEBAR_BOTTOM, current)
                .is_empty()
    });

    // The right sidebar is a wide-screen affordance: on a phone it would push
    // the fields below the fold, so there it is hidden unless the device asked
    // for it (see `app::mobile`). Off the mobile breakpoint nothing changes.
    let mobile_prefs = use_mobile_prefs();

    // Field-edit state shared with the fields and the inspector. The form is
    // the field list: entering the layout route claims no column, and the right
    // column is only claimed once the user selects a field.
    let creating = RwSignal::new(false);
    let creating_after = RwSignal::new(String::new());
    // Bumped after a field is created so the inspector re-reads the doctype's
    // customization and knows the new field as a custom one.
    let custom_revision = RwSignal::new(0_u64);
    // The drag and drop state of the field surfaces: which field is being
    // dragged and which one it is hovering. Both are names, not indices, so a
    // metadata reload after a drop cannot point them at another field.
    let dragging = RwSignal::new(None::<String>);
    let drop_target = RwSignal::new(None::<String>);
    // Leaving edit mode clears the panel state here: the exit action belongs to
    // the shared top bar (owned elsewhere) and may only flip `editing`, so the
    // panel's own signals are normalised in one place instead of trusting every
    // caller to remember them.
    let editing_surface = svc.editing;
    let selected_surface = svc.selected_field;
    Effect::new(move |_| {
        if !editing_surface.get() {
            selected_surface.set(None);
            creating.set(false);
            dragging.set(None);
            drop_target.set(None);
        }
    });

    // The create callback below runs from the create dialog's async task, after
    // the dialog has already decided to close. Pinning that update to this
    // view's long-lived owner keeps what it mounts owned by the form instead of
    // by the panel that is about to unmount.
    let view_owner = Owner::current();

    // The three outer states, as a value that only notifies on a *change*.
    //
    // Reading `loading`, `fields` and `meta` directly in the view closure made
    // every load a rebuild of the whole form: the `loading` flip and the new
    // `DynMeta` object both re-ran the branch, so the inspector (and the
    // fields, sidebar and dialogs) were re-created after every property save.
    // A memo keeps the `Ready` subtree mounted across reloads and re-creates it
    // only when the document/route actually changes phase.
    let phase = {
        let loading = svc_for_view.loading;
        let error = svc_for_view.error;
        let fields = svc_for_view.fields;
        Memo::new(move |_| {
            if loading.get() {
                return FormPhase::Loading;
            }
            if error.get().is_some() && fields.with(Vec::is_empty) {
                return FormPhase::Failed;
            }
            FormPhase::Ready
        })
    };
    // The declared document view does not change while a form is open; a
    // `Memo` keeps its update from rebuilding the Ready subtree when a newer
    // metadata snapshot arrives.
    let view_kind = {
        let meta = svc_for_view.meta;
        Memo::new(move |_| meta.get().map_or(DocView::Form, |meta| meta.view))
    };
    let opens_dashboard = {
        let svc = svc_for_view.clone();
        Memo::new(move |_| opens_as_dashboard(&svc))
    };

    view! {
        <div
            // The form is a document surface: white, like the top bar above it,
            // instead of the muted page background the dashboard/card pages use.
            // It owns the whole width: a metadata-driven form can be as wide as
            // the screen, and the centred 7xl column wasted half of a desktop.
            //
            // From `lg` it is also the fixed-height frame the inner panes need
            // (`h-full` + `min-h-0` + `overflow-hidden`): the top of the page
            // stays put and each column scrolls itself. The one branch without
            // an inner pane — a dashboard document — owns its own scroller, so
            // nothing is clipped. A phone keeps the normal page scroll
            // (`max-lg:min-h-full`): a nested pane there would trap a long form
            // behind two scrollbars.
            //
            // In edit mode the whole surface takes the calm muted look of a
            // disabled form. That is the only signal now: the old banner that
            // explained "haz clic en un campo" is gone, and the fields carry a
            // dashed outline (`layout::FormSectionView`) instead.
            class=move || {
                if editing_surface.get() {
                    "flex min-w-0 flex-col bg-muted/40 p-4 max-lg:min-h-full lg:h-full lg:min-h-0 lg:overflow-hidden"
                } else {
                    "flex min-w-0 flex-col bg-card p-4 max-lg:min-h-full lg:h-full lg:min-h-0 lg:overflow-hidden"
                }
            }
            data-name="MetadataFormView"
        >
            {move || {
                let svc_view = svc_for_view.clone();
                match phase.get() {
                    // A load that failed is not a form. `fields` stays empty
                    // when the metadata or the document could not be read, so
                    // rendering the editor over it offered an active "Guardar"
                    // on a document that does not exist — one click and it
                    // created a different one.
                    FormPhase::Loading => view! {
                        <div class="flex flex-col gap-4" aria-busy="true">
                            <div class="flex items-center gap-3">
                                <Skeleton class="h-9 w-48" />
                                <Skeleton class="h-6 w-24" />
                            </div>
                            <div class="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1fr)_18rem]">
                                <div class="flex flex-col gap-3">
                                    <Skeleton class="h-8 w-1/2" />
                                    <Skeleton class="h-64 w-full" />
                                    <Skeleton class="h-64 w-full" />
                                </div>
                                <div class="flex flex-col gap-3 max-lg:hidden">
                                    <Skeleton class="h-32 w-full" />
                                    <Skeleton class="h-24 w-full" />
                                </div>
                            </div>
                        </div>
                    }
                    .into_any(),
                    FormPhase::Failed => {
                        let failed_error = svc_view.error;
                        let failed_service = svc_view;
                        view! {
                            <div class="rounded-xl border bg-card p-6" data-name="FormLoadError">
                                <div class="flex items-start gap-3">
                                    <span class="flex size-9 shrink-0 items-center justify-center rounded-full bg-destructive/10 text-destructive">
                                        <Icon name="circle-alert" class="size-5" />
                                    </span>
                                    <div class="flex min-w-0 flex-col gap-2">
                                        <h2 class="text-base font-semibold">
                                            {i18n.t("views.form.load_error_title")}
                                        </h2>
                                        <ListErrorBanner error=failed_error.read_only() />
                                        <p class="text-sm text-muted-foreground">
                                            {i18n.t("views.form.load_error_body")}
                                        </p>
                                    </div>
                                </div>
                                <div class="mt-4 flex justify-end gap-2">
                                    <Button
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Sm
                                        on:click=move |_| failed_service.reload()
                                    >
                                        <Icon name="refresh-cw" class="size-4" />
                                        {i18n.t("views.common.retry")}
                                    </Button>
                                </div>
                            </div>
                        }
                        .into_any()
                    }
                    FormPhase::Ready => {
                        let view_kind = view_kind.get();

                        // Same predicate as the wrapper's width — `new` and a
                        // non-form declaration are the only ways out (see
                        // `opens_as_dashboard`).
                        if opens_dashboard.get() {
                        return view! {
                            // The surface is a fixed-height frame from `lg`; the
                            // dashboard is the one branch that is not an inner
                            // scroll pane, so it scrolls itself.
                            <div class="space-y-3 lg:h-full lg:overflow-y-auto">
                                // The load's error belongs to this view too: a
                                // read that fails after the first one
                                // succeeded would otherwise stay invisible.
                                <ListErrorBanner error=svc_view.error.read_only() />
                                <DashboardDocView
                                    meta=svc_view.meta.read_only()
                                    doc=svc_view.doc.read_only()
                                    docname=docname
                                    doctype=doctype
                                />
                            </div>
                        }
                        .into_any();
                    }
                    // The app's element, if the feed declares one, is mounted
                    // by `FormView` before this view runs — so reaching here
                    // with `Custom` means nothing answered the declaration.
                    // The layout route is the exception: it edits the shape of
                    // any doctype, so a custom-view doctype must still reach the
                    // layout editor instead of the "nothing to open" refusal.
                    if view_kind == DocView::Custom && !svc_view.layout_mode.get() {
                        return custom_view_missing(&i18n).into_any();
                    }

                    let sidebar_visible = svc_view.sidebar_visible;
                    // `Copy` handle so the layout's `Show` predicates do not
                    // capture (and thereby move) the whole service before the
                    // children are built.
                    let is_new = svc_view.is_new;
                    // Cloned per render so the enclosing `move ||` stays `FnMut`
                    // rather than consuming the handle on the first pass.
                    let engagement_service = engagement_service.clone();
                    // `StoredValue` is `Copy`: the layout's `Show` children are
                    // rebuilt, and a moved service would make them `FnOnce`.
                    let sidebar_service = StoredValue::new(svc_view.clone());
                    let inspector_service = StoredValue::new(svc_view.clone());
                    // `Copy` handles: the layout's `Show` predicates and the
                    // sidebar have to read them without moving the service.
                    let editing_state = svc_view.editing;
                    let selected_state = svc_view.selected_field;
                    // Whether the right column exists at all. Entering the
                    // layout route alone must not claim it: only a selected
                    // field does. Outside edit mode the document's sidebar —
                    // static card and/or plugin slots — owns it, and *all* of
                    // it follows the sidebar toggle: hiding the sidebar and
                    // leaving the plugin panels behind kept a 18rem column with
                    // nothing the user asked for. The grid's track list follows
                    // this too: an empty explicit `18rem` track would still take
                    // that width from the fields.
                    let column_occupied = move || {
                        if editing_state.get() {
                            selected_state.get().is_some()
                        } else {
                            sidebar_visible.get()
                                && (sidebar_declared.get() || !is_new.get())
                        }
                    };
                    // Action-bearing fieldtypes (`Button`,
                    // `Attachment Gallery`) act on the whole document rather
                    // than on their own value, so the form hands them its
                    // identity, raw doc and notice channel once, here — the
                    // widgets read it with `use_context`.
                    provide_context(FormDocContext {
                        doctype: svc_view.doctype,
                        docname: svc_view.docname,
                        is_new: svc_view.is_new,
                        doc: svc_view.doc.read_only(),
                        notice: svc_view.notice,
                        failure: svc_view.error,
                        reload: svc_view.reload,
                    });
                    // Edit mode: every field gets a click target that selects it
                    // for the inspector, a drag handle that reorders it and an
                    // insert rule. Provided once here; the dashboard's parameter
                    // panel renders the same fields without this context and
                    // stays untouched.
                    provide_context(FieldEditCtx {
                        editing: svc_view.editing.read_only().into(),
                        selected: svc_view.selected_field,
                        creating,
                        creating_after,
                        custom_revision,
                        dragging,
                        drop_target,
                        reorder: Callback::new({
                            let reorder_service = StoredValue::new(svc_view.clone());
                            move |(fieldname, before): (String, String)| {
                                reorder_service
                                    .get_value()
                                    .move_field_before(fieldname, before);
                            }
                        }),
                    });
                    view! {
                        <div class="flex min-h-0 flex-1 flex-col gap-4">
                            <FormTopBar service=svc_view.clone() />

                            <div class="flex flex-wrap items-center justify-end gap-2">
                                <FormTourLauncher doctype=doctype service=svc_view.clone() />
                            </div>

                            <ListErrorBanner error=svc_view.error.read_only() />
                            <Feedback
                                kind=FeedbackKind::Success
                                message=Signal::derive(move || svc_view.notice.get())
                            />

                            // The work area. From `lg` it is a fixed-height grid
                            // whose left column is the only vertical scroller; the
                            // right column is pinned and scrolls itself. A phone
                            // keeps one normal page scroll: a nested pane there
                            // would trap a long form behind two scrollbars.
                            //
                            // Fields and comments share the left scroller on a wide
                            // window, which is why they sit in a wrapper that is
                            // `display: contents` below `lg`: there its children
                            // become the flex items again, and the phone order
                            // stays fields → sidebar → comments instead of putting
                            // the activity feed between the fields and the sidebar.
                            <div class=move || {
                                if column_occupied() {
                                    // `lg:grid-rows-1` (a `minmax(0, 1fr)` row) is
                                    // load-bearing: with an implicit `auto` row the
                                    // columns would grow past the frame and the
                                    // inner scroll panes would never engage.
                                    "flex min-h-0 flex-1 flex-col gap-6 lg:grid lg:grid-cols-[minmax(0,1fr)_18rem] lg:grid-rows-1 lg:items-stretch"
                                } else {
                                    "flex min-h-0 flex-1 flex-col gap-6 lg:grid lg:grid-cols-1 lg:grid-rows-1 lg:items-stretch"
                                }
                            }>
                                <div class="max-lg:contents lg:col-start-1 lg:row-start-1 lg:flex lg:h-full lg:min-h-0 lg:flex-col lg:gap-6 lg:overflow-y-auto">
                                    // ---- Active tab content ----
                                    <div class="order-1 flex min-w-0 flex-col gap-6 lg:order-none">
                                        // OpenUI slot: between the top bar and the fields.
                                        <UiSlot slot_name=SLOT_FORM_BEFORE_FIELDS doctype=doctype />

                                        // The editor's own hint: the old banner
                                        // explained "haz clic en un campo" in a
                                        // full-width strip; this one line does
                                        // the same without pushing the canvas
                                        // down, and it is the only chrome the
                                        // layout route adds above the fields.
                                        <Show when=move || editing_state.get()>
                                            <div
                                                class="flex items-start gap-2 rounded-lg border border-dashed bg-card/70 px-3 py-2 text-xs text-muted-foreground"
                                                data-name="LayoutHint"
                                            >
                                                <Icon name="layout-panel-left" class="mt-0.5 size-4 shrink-0" />
                                                <p class="min-w-0">
                                                    {tr("views.form.layout_hint")}
                                                </p>
                                            </div>
                                        </Show>

                                        <FormTabs
                                            tab_labels=svc_view.tab_labels
                                            default_tab=svc_view.default_tab
                                            sections=svc_view.sections
                                            fields=svc_view.fields
                                            render_field=render_form_field
                                        />
                                    </div>

                                    // ---- Comments and activity ----
                                    // They belong to a document that exists. A `new`
                                    // form has neither, so the whole panel is withheld
                                    // rather than rendered empty (the request: "en el
                                    // modo de creación no ha de aparecer para comentar
                                    // ni actividad").
                                    <Show when=move || !is_new.get()>
                                        <div class="order-3 min-w-0 lg:order-none">
                                            <FormEngagement service=engagement_service.clone() />
                                        </div>
                                    </Show>
                                </div>

                                // ---- Right column: inspector, static sidebar or
                                // plugin panels ----
                                // Rendered only while it has something to show
                                // (`column_occupied`), with a single full-width
                                // track otherwise.
                                <div class=move || {
                                    if column_occupied() {
                                        // From `lg` this is a pane of its own,
                                        // pinned beside the fields and scrolling
                                        // independently; below, it stacks after
                                        // them (`order-2`).
                                        "order-2 flex min-h-0 w-full min-w-0 flex-col gap-6 lg:sticky lg:top-0 lg:col-start-2 lg:row-start-1 lg:h-full lg:order-none lg:overflow-y-auto"
                                    } else {
                                        "hidden"
                                    }
                                }>
                                    <Show when=move || editing_state.get()>
                                        <InspectorPanel service=inspector_service.get_value() />
                                    </Show>

                                    <Show when=move || !editing_state.get()>
                                        // The whole extension column follows the
                                        // same toggle as the static card: a
                                        // hidden sidebar that kept drawing its
                                        // plugin panels was not hidden.
                                        <Show when=move || sidebar_visible.get()>
                                            // OpenUI slot (`form.sidebar.top`): the
                                            // extension column starts the sidebar, above
                                            // the static card, so a plugin's panel reads
                                            // as the first thing in the column.
                                            <UiSlot slot_name=SLOT_FORM_SIDEBAR_TOP doctype=doctype />

                                            // ---- Static form sidebar ----
                                            // Hidden on a phone unless the device enables
                                            // it; when shown it stacks under the fields.
                                            <Show when=move || !is_new.get()>
                                                // CSS, not a media-query signal: the server
                                                // already paints a phone without the
                                                // sidebar, so hydration cannot make it
                                                // disappear (the old gate showed it for the
                                                // first frames and then pushed the fields
                                                // up).
                                                <div class=move || {
                                                    if mobile_prefs.get().form_sidebar {
                                                        // Full width only while stacked; from
                                                        // `lg` the inner aside owns its 18rem.
                                                        "w-full shrink-0 lg:w-auto"
                                                    } else {
                                                        "hidden shrink-0 lg:block"
                                                    }
                                                }>
                                                    <FormSidebar service=sidebar_service.get_value() />
                                                </div>
                                            </Show>

                                            // OpenUI slot (`form.sidebar.bottom`): the
                                            // extension column closes the sidebar, after
                                            // the provenance footer.
                                            <UiSlot slot_name=SLOT_FORM_SIDEBAR_BOTTOM doctype=doctype />
                                        </Show>
                                    </Show>
                                </div>
                            </div>

                            // The "new field" dialog, opened by the insert rules
                            // between fields (`+` on the hairline) and by the
                            // inspector's list. It is mounted outside the right
                            // column on purpose: the rules must be able to create
                            // a field while nothing is selected and the column is
                            // collapsed. It stays mounted and closed, so the
                            // scroll lock is released by the close rather than by
                            // a teardown.
                            <CreateFieldDialog
                                service=svc_view.clone()
                                open=creating
                                anchor=creating_after
                                on_created=Callback::new({
                                    let view_owner = view_owner.clone();
                                    move |name: String| {
                                        let apply = || {
                                            creating.set(false);
                                            selected_state.set(Some(name));
                                            custom_revision.update(|n| *n += 1);
                                        };
                                        match &view_owner {
                                            Some(owner) => owner.with(apply),
                                            None => apply(),
                                        }
                                    }
                                })
                            />
                        </div>
                    }
                    .into_any()
                    }
                }
            }}
        </div>
    }
}
