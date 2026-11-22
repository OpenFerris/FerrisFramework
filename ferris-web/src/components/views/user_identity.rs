//! User identities as the Desk shows them: a name and a link to the user.
//!
//! Every reference (provenance, assignee, share, comment author, activity
//! actor) is a user **id** on the wire. These components resolve it once to the
//! visual name, keep the id as the tooltip and fall back to the id when the
//! lookup fails, so a row is never blank and a screen reader never announces a
//! UUID where a name belongs.
//!
//! A name is also the way into the user document: [`users::href`] builds
//! `/desk/:module/User/:id` from the module resolved once at boot, so clicking
//! a person anywhere opens their form.

use leptos::prelude::*;
use leptos::task::spawn_local;
use tw_merge::tw_merge;

use crate::api::users;
use crate::components::ui::avatar::{Avatar, AvatarFallback, AvatarSize};

/// The label of `id`: cached value first, then a single fetch.
///
/// Blank until the lookup answers: printing the id first would flash a UUID in
/// the very place the name belongs. [`users::display`] falls back to the id
/// itself, so a failed lookup still renders something after the fetch.
fn resolve(id: String, label: RwSignal<String>) {
    if id.trim().is_empty() {
        label.set(String::new());
        return;
    }
    if let Some(cached) = users::cached(&id) {
        label.set(cached);
        return;
    }
    label.set(String::new());
    spawn_local(async move {
        let resolved = users::display(&id).await;
        label.set(resolved);
    });
}

/// Whether the `User` module is known, so a name can be a link.
///
/// The module is fetched once for the whole process; this signal only tells the
/// component when the answer arrived.
fn use_linkable() -> RwSignal<bool> {
    let linked = RwSignal::new(users::module().is_some());
    users::ensure_module(Callback::new(move |_| linked.set(true)));
    linked
}

/// The name (or a link to the user), with the id as the tooltip.
#[component]
fn NameText(
    #[prop(into)] id: Signal<String>,
    label: RwSignal<String>,
    #[prop(into)] class: String,
) -> impl IntoView {
    let linked = use_linkable();
    let class = StoredValue::new(tw_merge!(
        "min-w-0 truncate underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        class
    ));
    view! {
        {move || {
            let text = label.get();
            if text.is_empty() {
                return ().into_any();
            }
            let name = id.get();
            // A name links only to a user that exists: the id of a deleted user
            // would open a 404, and a link that does not go where it says is
            // worse than plain text.
            let exists = users::cached(&name).is_some();
            // Reading `linked` subscribes this render to the module answer.
            let href = (exists && linked.get())
                .then(|| users::href(&name))
                .flatten();
            match href {
                Some(href) => view! {
                    <a href=href class=class.get_value() title=name.clone()>{text}</a>
                }
                .into_any(),
                None => view! {
                    <span class=class.get_value() title=name>{text}</span>
                }
                .into_any(),
            }
        }}
    }
}

/// Just the name (provenance rows: "Creado por Administrator").
#[component]
pub fn UserName(
    #[prop(into)] id: Signal<String>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let label = RwSignal::new(String::new());
    Effect::new(move |_| resolve(id.get(), label));
    view! { <NameText id=id label=label class=class /> }
}

/// A standalone avatar: initials of the resolved name.
///
/// Used where the picture is the anchor of a block (the form sidebar header);
/// `class` owns its size and shape.
#[component]
pub fn UserAvatar(
    #[prop(into)] id: Signal<String>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let label = RwSignal::new(String::new());
    Effect::new(move |_| resolve(id.get(), label));
    let initials = move || users::initials(&label.get());
    let avatar_class = tw_merge!("size-8", class.clone());
    view! {
        <Avatar class=avatar_class>
            <AvatarFallback class=class>{initials}</AvatarFallback>
        </Avatar>
    }
}

/// Avatar + name, the shape the assignment and share lists use.
#[component]
pub fn UserIdentity(#[prop(into)] id: Signal<String>) -> impl IntoView {
    let label = RwSignal::new(String::new());
    Effect::new(move |_| resolve(id.get(), label));
    let initials = move || users::initials(&label.get());
    view! {
        <span class="flex min-w-0 items-center gap-2">
            <Avatar size=AvatarSize::Sm>
                <AvatarFallback>{initials}</AvatarFallback>
            </Avatar>
            <NameText id=id label=label class="" />
        </span>
    }
}
