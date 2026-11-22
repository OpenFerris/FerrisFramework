//! The remaining field types: secret, structured, files and the two types the
//! form cannot edit in place.
//!
//! `Geolocation` and the two editors are not here: they pull in vendored
//! browser libraries and live in [`super::geo`] and [`super::editor`].
//! Each of the rest had no dedicated widget before and fell through the
//! renderer's `_` arm to a plain text input, which is why they are collected
//! here rather than folded into one of the families above.

use icons::{Image, Info, Paperclip, Trash2};
use leptos::prelude::*;

use crate::components::pickers::FilePicker;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::textarea::Textarea;

/// Password (`Password`): a masked input. The engine hashes on write and masks
/// on read, so the widget only ever handles the plaintext the user typed.
#[component]
pub fn PasswordField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    max_length: Option<u32>,
) -> impl IntoView {
    view! {
        <Input
            id=id
            r#type=InputType::Password
            bind_value=value
            readonly=read_only
            placeholder=placeholder
            attr:maxlength=max_length
            autocomplete="new-password"
        />
    }
}

/// JSON (`JSON`): a monospace textarea. Validation stays server-side; the field
/// only has to make the structure legible. The monospace face is slightly
/// smaller than prose from `md` up; on a phone it stays at 16px so focusing it
/// does not make iOS Safari zoom the page.
#[component]
pub fn JsonField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
) -> impl IntoView {
    view! {
        <Textarea
            id=id
            bind_value=value
            readonly=read_only
            placeholder=placeholder
            rows=6u32
            class="font-mono text-xs max-md:text-base"
        />
    }
}

/// Barcode (`Barcode`): the value stays a plain, editable text input and a
/// JsBarcode SVG mirrors it below, drawn lazily from the vendored bundle
///.
///
/// The symbology comes from the field's `options` when it names one — that is
/// what Frappe puts there — and falls back to `CODE128`, because handing an
/// unknown format to the library makes it throw. The strokes use
/// `currentColor` so the barcode is legible in both themes instead of the
/// library's hardcoded black-on-white.
#[component]
pub fn BarcodeField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    options: String,
) -> impl IntoView {
    let svg_id = format!("{id}-barcode");
    let format = barcode_format(&options);
    // One render per change: before the bundle lands each call queues behind
    // `ensure` (the last queued value wins), afterwards they run synchronously.
    let render_id = svg_id.clone();
    Effect::new(move |_| {
        let current = value.get();
        let svg_id = render_id.clone();
        let format = format.clone();
        crate::ext::vendor::ensure(crate::ext::vendor::Vendor::JsBarcode, move || {
            render_barcode(&svg_id, &current, &format);
        });
    });

    view! {
        <div class="flex flex-col gap-2" data-name="__BarcodeField">
            <Input id=id r#type=InputType::Text bind_value=value readonly=read_only placeholder=placeholder />
            // The box is always in the DOM (so the effect can find the svg
            // the moment a value appears) and simply hidden while empty.
            <div
                class="flex justify-center rounded-md border bg-card p-3"
                class:hidden=move || value.get().trim().is_empty()
            >
                <svg id=svg_id class="h-16 w-full max-w-sm text-foreground" aria-hidden="true"></svg>
            </div>
        </div>
    }
}

/// The barcode symbology: the field's `options` when it names one of
/// JsBarcode's formats (matched case-insensitively, trimmed), else `CODE128`.
fn barcode_format(options: &str) -> String {
    const KNOWN: [&str; 16] = [
        "CODE39",
        "CODE128",
        "EAN",
        "EAN8",
        "EAN13",
        "EAN14",
        "UPC",
        "ITF",
        "ITF14",
        "MSI",
        "MSI10",
        "MSI11",
        "MSI1010",
        "MSI10010",
        "pharmacode",
        "codabar",
    ];
    KNOWN
        .iter()
        .find(|format| format.eq_ignore_ascii_case(options.trim()))
        .map(|format| (*format).to_string())
        .unwrap_or_else(|| "CODE128".to_string())
}

/// Draw (or clear) the SVG for the current value.
#[cfg(target_arch = "wasm32")]
fn render_barcode(svg_id: &str, value: &str, format: &str) {
    use wasm_bindgen::{JsCast, JsValue};

    let barcode = match crate::ext::vendor::global(crate::ext::vendor::Vendor::JsBarcode) {
        Some(global) => global,
        None => {
            crate::ext::vendor::js::error("barcode: the JsBarcode global is missing after load");
            return;
        }
    };
    let Some(svg) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|doc| doc.get_element_by_id(svg_id))
    else {
        return; // unmounted before the bundle landed
    };
    // A stale barcode must not survive a value the format cannot express.
    svg.set_inner_html("");
    if value.trim().is_empty() {
        return;
    }
    let Ok(func) = barcode.dyn_into::<js_sys::Function>() else {
        crate::ext::vendor::js::error("barcode: the JsBarcode global is not callable");
        return;
    };
    let options = js_sys::Object::new();
    let set = |key: &str, val: JsValue| {
        let _ = js_sys::Reflect::set(&options, &JsValue::from_str(key), &val);
    };
    set("format", JsValue::from_str(format));
    set("displayValue", JsValue::TRUE);
    set("lineColor", JsValue::from_str("currentColor"));
    set("background", JsValue::from_str("transparent"));
    set("height", JsValue::from_f64(64.0));
    set("margin", JsValue::from_f64(4.0));
    set("fontSize", JsValue::from_f64(12.0));
    // Wrong characters for the chosen format (letters in an EAN, say) make
    // JsBarcode throw; the JS exception surfaces as `Err` and is logged — the
    // input above still holds the value, the svg just stays empty.
    if func
        .call3(
            &JsValue::UNDEFINED,
            &svg.into(),
            &JsValue::from_str(value),
            &options.into(),
        )
        .is_err()
    {
        crate::ext::vendor::js::error("barcode: the value does not fit the chosen format");
    }
}

/// Native twin: SSR renders the input and the (hidden, empty) svg box.
#[cfg(not(target_arch = "wasm32"))]
fn render_barcode(_svg_id: &str, _value: &str, _format: &str) {}

/// File or image (`Attach`, `Image`).
///
/// The value is a URL served by the Desk (`/api/files/{id}`), with the file
/// name kept in the fragment so the card can show a readable label. The control
/// is the file picker: list the stored files or upload a new one. There is no
/// free-text path box any more — asking a user to type a storage path was the
/// opposite of a selector.
#[component]
pub fn AttachField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    #[prop(default = false)] image: bool,
) -> impl IntoView {
    let _ = placeholder;
    let picker_open = RwSignal::new(false);
    let has_value = Signal::derive(move || !value.get().trim().is_empty());
    let accept = if image {
        "image/*".to_string()
    } else {
        String::new()
    };
    let on_pick = Callback::new(move |item: crate::api::files::FileItem| {
        value.set(crate::api::files::display_url(&item));
    });
    let label = move || crate::api::files::split_url(&value.get()).1;
    let href = move || crate::api::files::split_url(&value.get()).0;
    view! {
        <div class="flex flex-col gap-2">
            <Show when=move || has_value.get()>
                <div class="flex items-center gap-3 rounded-xl border bg-card p-2 text-card-foreground">
                    <span class="flex size-10 shrink-0 items-center justify-center overflow-hidden rounded-lg bg-muted text-muted-foreground">
                        {if image {
                            view! { <img src=href alt="" class="size-10 object-cover" /> }.into_any()
                        } else {
                            view! { <Paperclip class="size-4" /> }.into_any()
                        }}
                    </span>
                    <a
                        href=href
                        target="_blank"
                        rel="noreferrer"
                        class="min-w-0 flex-1 truncate text-sm text-foreground underline-offset-2 hover:underline"
                    >
                        {label}
                    </a>
                    <Show when=move || !read_only>
                        <Button
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::IconXs
                            attr:aria-label="Quitar"
                            on:click=move |_| value.set(String::new())
                        >
                            <Trash2 class="size-3.5" />
                        </Button>
                    </Show>
                </div>
            </Show>
            <Show when=move || !read_only && !has_value.get()>
                <Button
                    // The field's `<label for=…>` points here while the field
                    // is empty; the button is labelable, unlike the card.
                    attr:id=id.clone()
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on:click=move |_| picker_open.set(true)
                >
                    {if image {
                        view! { <Image class="size-4" /> }.into_any()
                    } else {
                        view! { <Paperclip class="size-4" /> }.into_any()
                    }}
                    {if image { "Elegir imagen" } else { "Elegir archivo" }}
                </Button>
            </Show>
            <FilePicker open=picker_open on_pick=on_pick accept=accept />
        </div>
    }
}

/// Introspection field (`Read Only`): a value with no widget at all.
#[component]
pub fn ReadOnlyField(id: String, value: RwSignal<String>) -> impl IntoView {
    // An empty introspection value used to be a blank bordered box, which reads
    // as a broken control; the em dash says "this field is empty" the same way
    // the rest of the Desk does.
    let empty = Signal::derive(move || value.get().trim().is_empty());
    view! {
        <p
            id=id
            class="rounded-md border bg-muted/40 px-3 py-2 text-sm text-foreground"
            class:text-muted-foreground=empty
        >
            {move || {
                let v = value.get();
                if v.trim().is_empty() { "—".to_string() } else { v }
            }}
        </p>
    }
}

/// HTML block (`HTML`): not an input — renders its `options` as HTML on the
/// form, exactly like Frappe (the content comes from the metadata, so only a
/// caller who may write metadata can put markup here).
///
/// This is the one place the Desk renders unsanitized markup, deliberately:
/// it is trusted, admin-authored content. Scripts inside it still do not run —
/// the server's CSP keeps `script-src 'self'` plus a per-response nonce, and
/// `innerHTML` never executes `<script>` elements — so the worst a hostile
/// metadata writer can do is draw, not execute.
#[component]
pub fn HtmlBlockField(id: String, html: String) -> impl IntoView {
    view! {
        <div
            id=id
            class="text-sm text-foreground"
            inner_html=html
            data-name="__HtmlBlockField"
        ></div>
    }
}

/// Child table (`Table`): the form cannot edit rows inline, so it says so and
/// points at where the rows live instead of rendering nothing.
#[component]
pub fn TableNoticeField(#[prop(into)] label: String) -> impl IntoView {
    view! {
        <div class="flex items-start gap-2 rounded-md border border-dashed bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
            <Info class="mt-0.5 size-4 shrink-0" />
            <span>
                {if label.is_empty() {
                    "La tabla hija se edita en la sección del formulario.".to_string()
                } else {
                    format!("«{label}» se edita en la sección del formulario.")
                }}
            </span>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::barcode_format;

    /// An unknown `options` string must never reach JsBarcode: the library
    /// throws on a format it does not know, and the field would render
    /// nothing but an error.
    #[test]
    fn unknown_barcode_options_fall_back_to_code128() {
        assert_eq!(barcode_format("ean13"), "EAN13");
        assert_eq!(barcode_format(" CODE128 "), "CODE128");
        assert_eq!(barcode_format(""), "CODE128");
        assert_eq!(barcode_format("<script>"), "CODE128");
        assert_eq!(barcode_format("PURPLE"), "CODE128");
    }
}
