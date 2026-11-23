//! Shared page chrome: header, feedback and empty/loading states.
//!
//! Every view used to invent its own title size, description colour and error
//! box. That is how a Desk ends up with four different "page title" weights and
//! five error banners. These components are the one place those decisions live:
//!
//! * **`PageHeader`** — title, optional description and a right-aligned action
//!   slot. The title scale (`text-2xl`, `tracking-tight`) and the muted
//!   description are fixed here.
//! * **`Feedback`** — one inline banner for error/success/warning/info, with an
//!   icon, `role="alert"` for errors and `role="status"` for the rest.
//! * **`EmptyState`** — the `ui::empty` primitives with an icon and an optional
//!   action, so "nothing here" always offers the next step.
//! * **`LoadingLines`** — a skeleton block with the same rhythm as a card body.
//!
//! They only compose existing `ferris-web-ui` primitives and semantic tokens;
//! no colour literal or one-off radius is introduced.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::components::ui::empty::{
    Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle,
};
use crate::components::ui::icon::Icon;
use crate::components::ui::skeleton::Skeleton;
use crate::i18n::use_i18n;

/// The header of a page: title, optional description and actions.
#[component]
pub fn PageHeader(
    #[prop(into)] title: String,
    #[prop(into, optional)] description: String,
    /// Action buttons/links, rendered to the right on wide screens and below on
    /// narrow ones.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let has_description = !description.trim().is_empty();
    view! {
        <header class="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
            <div class="flex min-w-0 flex-col gap-1">
                <h1 class="break-words text-2xl font-semibold tracking-tight text-foreground">{title}</h1>
                {has_description.then(|| view! {
                    <p class="max-w-2xl break-words text-sm text-muted-foreground">{description}</p>
                })}
            </div>
            <div class="flex min-w-0 flex-wrap items-center gap-2">
                {children.map(|children| children())}
            </div>
        </header>
    }
}

/// A section heading inside a page (smaller than [`PageHeader`]).
#[component]
pub fn SectionTitle(
    #[prop(into)] title: String,
    #[prop(into, optional)] description: String,
) -> impl IntoView {
    let has_description = !description.trim().is_empty();
    view! {
        <div class="flex flex-col gap-1">
            <h2 class="break-words text-base font-semibold tracking-tight text-foreground">{title}</h2>
            {has_description.then(|| view! {
                <p class="break-words text-sm text-muted-foreground">{description}</p>
            })}
        </div>
    }
}

/// The tone of a [`Feedback`] banner.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum FeedbackKind {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl FeedbackKind {
    const fn classes(self) -> &'static str {
        match self {
            Self::Info => "border-info/40 bg-info/10 text-info",
            Self::Success => "border-success/40 bg-success/10 text-success",
            // Warning yellow on a 10% tint is unreadable; the icon carries the
            // tone and the text keeps the foreground token.
            Self::Warning => "border-warning/50 bg-warning/10 text-foreground",
            Self::Error => "border-destructive/40 bg-destructive/10 text-destructive",
        }
    }

    const fn icon(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Success => "CircleCheck",
            Self::Warning => "TriangleAlert",
            Self::Error => "CircleAlert",
        }
    }
}

/// One inline feedback banner; renders nothing while `message` is `None`.
#[component]
pub fn Feedback(
    #[prop(default = FeedbackKind::Info)] kind: FeedbackKind,
    #[prop(into)] message: Signal<Option<String>>,
) -> impl IntoView {
    view! {
        <Show when=move || message.get().is_some()>
            <div
                class=format!(
                    "flex items-start gap-2 rounded-lg border px-3 py-2 text-sm {}",
                    kind.classes(),
                )
                role=if kind == FeedbackKind::Error { "alert" } else { "status" }
            >
                <Icon name=kind.icon() class="mt-0.5 size-4 shrink-0" />
                <span class="min-w-0 break-words">{move || message.get().unwrap_or_default()}</span>
            </div>
        </Show>
    }
}

/// The empty state of a list/section, with an icon and an optional action.
#[component]
pub fn EmptyState(
    #[prop(into, optional)] icon: Option<String>,
    #[prop(into)] title: String,
    #[prop(into)] description: String,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <Empty>
            <EmptyHeader>
                {icon.map(|icon| view! {
                    <EmptyMedia variant=crate::components::ui::empty::EmptyMediaVariant::Icon>
                        <Icon name=icon />
                    </EmptyMedia>
                })}
                <EmptyTitle>{title}</EmptyTitle>
                <EmptyDescription>{description}</EmptyDescription>
            </EmptyHeader>
            <EmptyContent>{children.map(|children| children())}</EmptyContent>
        </Empty>
    }
}

/// A skeleton block for a loading card body.
#[component]
pub fn LoadingLines(#[prop(default = 3)] lines: usize) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-2" aria-busy="true" aria-live="polite">
            {(0..lines)
                .map(|index| {
                    let width = if index + 1 == lines { "w-2/3" } else { "w-full" };
                    view! { <Skeleton class=format!("h-4 {width}") /> }
                })
                .collect_view()}
        </div>
    }
}

/// The page a route fallback renders: a real 404 inside the Desk, with the way
/// home, instead of the bare `Not Found` string the router used to draw.
#[component]
pub fn NotFoundPage() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="mx-auto flex h-full w-full max-w-2xl flex-col justify-center gap-4 p-4">
            <PageHeader
                title=i18n.t("error.not_found_title")
                description=i18n.t("views.common.not_found_description")
            />
            <EmptyState
                icon="FileQuestion"
                title=i18n.t("views.common.not_found_empty_title")
                description=i18n.t("views.common.not_found_empty_description")
            >
                <A
                    href=crate::app::DESK_PATH
                    attr:class="inline-flex h-9 max-md:h-10 items-center gap-2 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                >
                    <Icon name="House" class="size-4" />
                    {i18n.t("views.common.go_to_desk")}
                </A>
            </EmptyState>
        </div>
    }
}
