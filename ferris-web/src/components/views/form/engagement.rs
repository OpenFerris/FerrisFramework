//! The engagement panel at the end of the active tab: the comment thread and
//! the document's history.
//!
//! Both are read from the endpoints the document engine already exposes —
//! `GET /api/engagement/{doctype}/{name}/comments` and
//! `GET /api/activity/{doctype}/{name}` — and a posted comment (or reply) is
//! echoed back through the same reads, so what is on screen is always the
//! server's answer rather than a local guess.
//!
//! The history is the composition `tab_version` + `tab_comment` + `tab_like` the
//! backend builds at read time. It is grouped by day ("Hoy", "Ayer", a date),
//! newest first, with the changed fields as chips and a relative age per entry:
//! a long timeline reads as days, not as an undifferentiated column of rows.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::types::{ActivityEntry, ActivityKind, DocComment};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::components::ui::separator::Separator;
use crate::components::ui::textarea::Textarea;
use crate::components::views::time;
use crate::components::views::user_identity::{UserAvatar, UserName};
use crate::views::services::FormService;

/// Display label for an activity kind.
fn kind_label(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Create => "creó el documento",
        ActivityKind::Update => "actualizó",
        ActivityKind::Submit => "envió",
        ActivityKind::Cancel => "canceló",
        ActivityKind::Amend => "corrigió",
        ActivityKind::Delete => "eliminó",
        ActivityKind::Comment => "comentó",
        ActivityKind::Reply => "respondió",
        ActivityKind::Like => "marcó como favorito",
    }
}

/// Icon for an activity kind.
fn kind_icon(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Create => "file-plus",
        ActivityKind::Update => "pencil",
        ActivityKind::Submit => "check",
        ActivityKind::Cancel => "x",
        ActivityKind::Amend => "refresh-cw",
        ActivityKind::Delete => "trash",
        ActivityKind::Comment => "message-square",
        ActivityKind::Reply => "corner-down-right",
        ActivityKind::Like => "heart",
    }
}

/// One day of the history.
#[derive(Clone, PartialEq)]
struct DayGroup {
    label: String,
    entries: Vec<ActivityEntry>,
}

#[component]
pub fn FormEngagement(service: FormService) -> impl IntoView {
    let service = StoredValue::new(service);
    let svc = service.get_value();
    let doctype = svc.doctype;
    let docname = svc.docname;
    let is_new = svc.is_new;

    let comments = RwSignal::new(Vec::<DocComment>::new());
    let activity = RwSignal::new(Vec::<ActivityEntry>::new());
    let error = RwSignal::new(None::<String>);
    let draft = RwSignal::new(String::new());
    let reply_to = RwSignal::new(None::<String>);
    let reply_draft = RwSignal::new(String::new());

    let reload = move || {
        if is_new.get() {
            return;
        }
        let dt = doctype.get();
        let name = docname.get();
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            match store.list_comments(&dt, &name).await {
                Ok(list) => comments.set(list),
                Err(e) => error.set(Some(e)),
            }
            match store.activity(&dt, &name).await {
                Ok(list) => activity.set(list),
                Err(e) => error.set(Some(e)),
            }
        });
    };
    Effect::new(move |_| {
        let _ = docname.get();
        let _ = is_new.get();
        reload();
    });

    let post = move || {
        let body = draft.get().trim().to_string();
        if body.is_empty() {
            return;
        }
        let (dt, name) = (doctype.get(), docname.get());
        error.set(None);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            match store.add_comment(&dt, &name, &body, None).await {
                Ok(_) => {
                    draft.set(String::new());
                    reload();
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let post_reply = move || {
        let body = reply_draft.get().trim().to_string();
        let Some(parent) = reply_to.get() else {
            return;
        };
        if body.is_empty() {
            return;
        }
        let (dt, name) = (doctype.get(), docname.get());
        error.set(None);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            match store.add_comment(&dt, &name, &body, Some(&parent)).await {
                Ok(_) => {
                    reply_draft.set(String::new());
                    reply_to.set(None);
                    reload();
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    // Group replies under their parent, and keep the others at top level.
    let thread = Memo::new(move |_| {
        let all = comments.get();
        let mut roots: Vec<DocComment> = Vec::new();
        let mut replies: HashMap<String, Vec<DocComment>> = HashMap::new();
        for comment in all {
            match comment.parent.clone() {
                Some(parent) if !parent.is_empty() => {
                    replies.entry(parent).or_default().push(comment);
                }
                _ => roots.push(comment),
            }
        }
        (roots, replies)
    });

    // One group per day, newest first (the endpoint answers oldest first).
    let history = Memo::new(move |_| {
        let mut groups: Vec<DayGroup> = Vec::new();
        for entry in activity.get().into_iter().rev() {
            let label = time::day(&entry.at);
            match groups.last_mut() {
                Some(group) if group.label == label => group.entries.push(entry),
                _ => groups.push(DayGroup {
                    label,
                    entries: vec![entry],
                }),
            }
        }
        groups
    });

    view! {
        <section class="flex flex-col gap-4" data-name="FormEngagement">
            <Show when=move || error.get().is_some()>
                <div
                    class="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                    role="alert"
                >
                    {move || error.get().unwrap_or_default()}
                </div>
            </Show>

            // ---- Comment thread ----
            <div class="flex flex-col gap-3">
                <div class="flex items-baseline justify-between gap-2">
                    <h3 class="flex items-center gap-2 text-sm font-semibold">
                        <Icon name="message-square" class="size-4 text-muted-foreground" />
                        "Comentarios"
                    </h3>
                    <Show when=move || !thread.get().0.is_empty()>
                        <span class="rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                            {move || thread.get().0.len()}
                        </span>
                    </Show>
                </div>

                <Show
                    when=move || !thread.get().0.is_empty()
                    fallback=|| view! {
                        <p class="rounded-lg border border-dashed px-3 py-4 text-center text-xs text-muted-foreground">
                            "Todavía no hay comentarios."
                        </p>
                    }
                >
                    {move || {
                        let (roots, replies) = thread.get();
                        roots.into_iter().map(|root| {
                            let id = root.id.clone();
                            let kids = replies.get(&id).cloned().unwrap_or_default();
                            let has_kids = !kids.is_empty();
                            // Separate owned copies for the two closures below:
                            // the `Show`'s predicate and the fallback's click
                            // handler each capture the id by move.
                            let reply_id_when = id.clone();
                            let reply_id_click = id.clone();
                            view! {
                                <div class="flex flex-col gap-2" data-name="CommentThread">
                                    <CommentRow comment=root />
                                    <Show when=move || has_kids>
                                        // Replies hang off a rail, so the thread
                                        // reads as a conversation instead of a
                                        // flat list of comments.
                                        <div class="ms-10 flex flex-col gap-3 border-s ps-4">
                                            {kids.iter().cloned().map(|kid| view! {
                                                <CommentRow comment=kid compact=true />
                                            }).collect_view()}
                                        </div>
                                    </Show>
                                    <div class="ms-10">
                                        <Show
                                            when=move || reply_to.get().as_deref() == Some(reply_id_when.as_str())
                                            fallback=move || view! {
                                                <Button
                                                    variant=ButtonVariant::Ghost
                                                    size=ButtonSize::Sm
                                                    class="h-7 px-2 text-xs text-muted-foreground"
                                                    on:click={
                                                        let id = reply_id_click.clone();
                                                        move |_| reply_to.set(Some(id.clone()))
                                                    }
                                                >
                                                    <Icon name="corner-down-right" class="size-3.5" />
                                                    "Responder"
                                                </Button>
                                            }
                                        >
                                            <div class="flex flex-col gap-2 rounded-lg border bg-card p-2">
                                                <Textarea
                                                    bind_value=reply_draft
                                                    placeholder="Escribe una respuesta…"
                                                    rows=2u32
                                                />
                                                <div class="flex justify-end gap-2">
                                                    <Button
                                                        variant=ButtonVariant::Ghost
                                                        size=ButtonSize::Sm
                                                        on:click=move |_| {
                                                            reply_to.set(None);
                                                            reply_draft.set(String::new());
                                                        }
                                                    >
                                                        "Cancelar"
                                                    </Button>
                                                    <Button
                                                        variant=ButtonVariant::Default
                                                        size=ButtonSize::Sm
                                                        on:click=move |_| post_reply()
                                                    >
                                                        "Responder"
                                                    </Button>
                                                </div>
                                            </div>
                                        </Show>
                                    </div>
                                </div>
                            }
                        }).collect_view()
                    }}
                </Show>

                // New comment.
                <div class="flex flex-col gap-2 rounded-xl border bg-card p-3">
                    <Textarea
                        bind_value=draft
                        placeholder="Escribe un comentario…"
                        rows=2u32
                    />
                    <div class="flex items-center justify-between gap-2">
                        <p class="text-xs text-muted-foreground">
                            "Se publica en el historial del documento."
                        </p>
                        <Button
                            variant=ButtonVariant::Default
                            size=ButtonSize::Sm
                            attr:disabled=move || is_new.get() || draft.get().trim().is_empty()
                            on:click=move |_| post()
                        >
                            "Comentar"
                        </Button>
                    </div>
                </div>
            </div>

            <Separator />

            // ---- History ----
            <div class="flex flex-col gap-3">
                <div class="flex items-baseline justify-between gap-2">
                    <h3 class="flex items-center gap-2 text-sm font-semibold">
                        <Icon name="history" class="size-4 text-muted-foreground" />
                        "Historial"
                    </h3>
                    <Show when=move || !activity.get().is_empty()>
                        <span class="text-xs text-muted-foreground">
                            {move || format!("{} eventos", activity.get().len())}
                        </span>
                    </Show>
                </div>
                <Show
                    when=move || !history.get().is_empty()
                    fallback=|| view! {
                        <p class="rounded-lg border border-dashed px-3 py-4 text-center text-xs text-muted-foreground">
                            "Sin actividad."
                        </p>
                    }
                >
                    <ol class="flex flex-col gap-6" data-name="ActivityTimeline">
                        {move || history.get().into_iter().map(|group| {
                            let entries = group.entries;
                            let count = entries.len();
                            view! {
                                <li class="flex flex-col gap-3">
                                    <div class="flex items-center gap-2">
                                        <h4 class="text-xs font-medium tracking-wide text-muted-foreground uppercase">
                                            {group.label}
                                        </h4>
                                        <span class="h-px flex-1 bg-border"></span>
                                        <span class="text-xs text-muted-foreground">{count}</span>
                                    </div>
                                    // The rail is the container's own
                                    // pseudo-element: one vertical line behind
                                    // every dot, no per-row borders to drift.
                                    <ol class="relative flex flex-col gap-4 before:absolute before:inset-y-2 before:left-3 before:w-px before:bg-border">
                                        {entries.into_iter().map(|entry| view! {
                                            <ActivityRow entry=entry />
                                        }).collect_view()}
                                    </ol>
                                </li>
                            }
                        }).collect_view()}
                    </ol>
                </Show>
            </div>
        </section>
    }
}

/// One history entry: the rail dot, actor, action, changed fields and age.
///
/// The dot sits on the group's rail (`before:` line on the `<ol>`) with a card
/// background, so the line reads as continuous behind it instead of crossing
/// the icon.
#[component]
fn ActivityRow(entry: ActivityEntry) -> impl IntoView {
    let actor = entry.actor.clone();
    let relative = time::relative(&entry.at);
    let exact = time::exact(&entry.at);
    let label = kind_label(entry.kind);
    let icon = kind_icon(entry.kind);
    let fields = entry.fields;
    let has_fields = !fields.is_empty();
    let detail = entry.detail.unwrap_or_default();
    let has_detail = !detail.is_empty();
    view! {
        <li class="relative flex gap-3" data-name="ActivityEntry">
            <span class="relative z-10 mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-full border bg-card text-muted-foreground shadow-xs">
                <Icon name=icon class="size-3.5" />
            </span>
            <div class="flex min-w-0 flex-1 flex-col gap-1 pb-1">
                <p class="flex flex-wrap items-baseline gap-x-1.5 text-sm leading-5">
                    <UserName id=actor class="font-medium text-foreground" />
                    <span class="text-muted-foreground">{label}</span>
                    <span class="ms-auto text-xs whitespace-nowrap text-muted-foreground" title=exact>
                        {relative}
                    </span>
                </p>
                <Show when=move || has_fields>
                    <div class="flex flex-wrap gap-1">
                        {fields.clone().into_iter().map(|field| view! {
                            <span class="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                                {field}
                            </span>
                        }).collect_view()}
                    </div>
                </Show>
                <Show when=move || has_detail>
                    <p class="rounded-md border-s-2 bg-muted/40 px-2.5 py-1.5 text-sm break-words text-muted-foreground">
                        {detail.clone()}
                    </p>
                </Show>
            </div>
        </li>
    }
}

/// One comment: avatar, author (a link), age and body in a bubble.
///
/// `compact` is the reply variant: same anatomy, tighter bubble, because the
/// thread rail already carries the nesting.
#[component]
fn CommentRow(comment: DocComment, #[prop(optional)] compact: bool) -> impl IntoView {
    let author = comment.commented_by.clone();
    let relative = time::relative(&comment.created_at);
    let exact = time::exact(&comment.created_at);
    let bubble = if compact {
        "rounded-lg border bg-card px-3 py-2"
    } else {
        "rounded-lg bg-muted/40 px-3 py-2"
    };
    view! {
        <div class="flex gap-2.5" data-name="CommentRow">
            <UserAvatar id=author.clone() />
            <div class="flex min-w-0 flex-1 flex-col gap-1">
                <div class="flex flex-wrap items-baseline gap-x-2">
                    <UserName id=author class="text-sm font-medium text-foreground" />
                    <span class="text-xs text-muted-foreground" title=exact>{relative}</span>
                </div>
                // `break-words`: a comment is free text, and one long URL must
                // wrap instead of widening the thread past a 320px screen.
                <div class=bubble>
                    <p class="text-sm whitespace-pre-wrap break-words">{comment.body}</p>
                </div>
            </div>
        </div>
    }
}
