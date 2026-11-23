//! Signature (`Signature`): a canvas-captured drawing stored as a PNG data
//! URL, with no upload round-trip — the value *is* the data URL, which is what
//! the engine stores and what the form draws back on load.
//!
//! Drawing is wired with pointer events whose handlers delegate to
//! `wasm32`-gated helpers: SSR emits a plain `<canvas>` plus the buttons, and
//! only the browser attaches behaviour (no browser global during SSR, per
//! `AGENTS.md` §5). The native no-op twins exist so the same view compiles for
//! the `ssr` feature.

use leptos::prelude::*;

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;

/// The fixed backing-store size the canvas paints at (CSS scales it to the
/// form's width; the handlers map pointer coordinates through the element's
/// bounding box, so the stored image stays crisp).
const CANVAS_WIDTH: u32 = 600;
const CANVAS_HEIGHT: u32 = 240;

#[component]
pub fn SignatureField(id: String, value: RwSignal<String>, read_only: bool) -> impl IntoView {
    let drawing = RwSignal::new(false);
    // `id` is consumed by several closures (the canvas handlers and the clear
    // button), and a `StoredValue` is `Copy` so each can read its own copy.
    let id_store = StoredValue::new(id.clone());
    // Restore a signature loaded from the document once, at mount.
    Effect::new(move |_| {
        let data = value.get_untracked();
        let id = id_store.get_value();
        restore_canvas(&id, data);
    });

    view! {
        <div class="flex flex-col gap-2" data-name="__SignatureField">
            <canvas
                id=id
                width=CANVAS_WIDTH
                height=CANVAS_HEIGHT
                class="h-40 w-full rounded-md border bg-background touch-none"
                role="img"
                aria-label=t("components.form.field.signature")
                on:pointerdown=move |ev| {
                    drawing.set(true);
                    begin_draw(&id_store.get_value(), &ev);
                }
                on:pointermove=move |ev| {
                    continue_draw(&id_store.get_value(), &ev, drawing.get(), value)
                }
                on:pointerup=move |_| {
                    drawing.set(false);
                    end_draw(&id_store.get_value(), value);
                }
                on:pointercancel=move |_| {
                    drawing.set(false);
                    end_draw(&id_store.get_value(), value);
                }
            ></canvas>
            <Show when=move || !read_only>
                <div class="flex items-center gap-2">
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        on:click=move |_| {
                            clear_canvas(&id_store.get_value());
                            value.set(String::new());
                            drawing.set(false);
                        }
                    >
                        {t("components.form.field.clear")}
                    </Button>
                    <Show when=move || !value.get().trim().is_empty()>
                        <span class="text-xs text-muted-foreground">
                            {t("components.form.field.signature_saved")}
                        </span>
                    </Show>
                </div>
            </Show>
        </div>
    }
}

// The four handlers below are the SSR/hydrate seam: the browser version does
// the canvas work, the native one exists only so the same `view!` compiles for
// the `ssr` feature. Never call a browser global outside the `wasm32` twin.

/// Begin a stroke at the pointer's position.
fn begin_draw(id: &str, ev: &leptos::ev::PointerEvent) {
    #[cfg(target_arch = "wasm32")]
    wasm::begin(id, coords(ev));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (id, ev);
}

/// Extend the stroke; commits the resulting data URL into `value` on each
/// segment so an interrupted pointer sequence still leaves a complete image.
fn continue_draw(id: &str, ev: &leptos::ev::PointerEvent, drawing: bool, value: RwSignal<String>) {
    #[cfg(target_arch = "wasm32")]
    wasm::continue_(id, coords(ev), drawing, value);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (id, ev, drawing, value);
}

/// Finish the stroke (pointer up/cancel) and commit the data URL.
fn end_draw(id: &str, value: RwSignal<String>) {
    #[cfg(target_arch = "wasm32")]
    wasm::end(id, value);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (id, value);
}

fn clear_canvas(id: &str) {
    #[cfg(target_arch = "wasm32")]
    wasm::clear(id);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = id;
}

/// Repaint the canvas from a stored data URL (load of an existing document).
fn restore_canvas(id: &str, data: String) {
    #[cfg(target_arch = "wasm32")]
    wasm::restore(id, data);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (id, data);
}

/// Client coordinates, read through a per-target twin so the accessor only
/// needs to exist where it is used.
#[cfg(target_arch = "wasm32")]
fn coords(ev: &leptos::ev::PointerEvent) -> (i32, i32) {
    (ev.client_x(), ev.client_y())
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    //! The canvas work, browser-only. No `unwrap`: a missing element or
    //! context aborts the gesture silently rather than poisoning the form —
    //! the widget simply stays inert until the DOM is there.

    use leptos::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::HtmlCanvasElement;

    fn context(id: &str) -> Option<(HtmlCanvasElement, web_sys::CanvasRenderingContext2d)> {
        let canvas = web_sys::window()?
            .document()?
            .get_element_by_id(id)?
            .dyn_into::<HtmlCanvasElement>()
            .ok()?;
        let ctx = canvas
            .get_context("2d")
            .ok()?
            .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())?;
        Some((canvas, ctx))
    }

    /// Pointer position in canvas pixel coordinates (the element is
    /// CSS-scaled, so the bounding box maps CSS pixels to the backing store).
    fn point(canvas: &HtmlCanvasElement, (client_x, client_y): (i32, i32)) -> (f64, f64) {
        let rect = canvas.get_bounding_client_rect();
        let scale_x = if rect.width() > 0.0 {
            f64::from(canvas.width()) / rect.width()
        } else {
            1.0
        };
        let scale_y = if rect.height() > 0.0 {
            f64::from(canvas.height()) / rect.height()
        } else {
            1.0
        };
        (
            (f64::from(client_x) - rect.left()) * scale_x,
            (f64::from(client_y) - rect.top()) * scale_y,
        )
    }

    pub fn begin(id: &str, at: (i32, i32)) {
        let Some((canvas, ctx)) = context(id) else {
            return;
        };
        ctx.begin_path();
        let (x, y) = point(&canvas, at);
        ctx.move_to(x, y);
    }

    pub fn continue_(id: &str, at: (i32, i32), drawing: bool, value: RwSignal<String>) {
        if !drawing {
            return;
        }
        let Some((canvas, ctx)) = context(id) else {
            return;
        };
        let (x, y) = point(&canvas, at);
        ctx.line_to(x, y);
        ctx.stroke();
        // Commit per segment: a pointer that leaves the canvas (or the tab)
        // mid-stroke still leaves the form holding everything drawn so far.
        value.set(
            canvas
                .to_data_url_with_type("image/png")
                .unwrap_or_default(),
        );
    }

    pub fn end(id: &str, value: RwSignal<String>) {
        let Some((canvas, _)) = context(id) else {
            return;
        };
        value.set(
            canvas
                .to_data_url_with_type("image/png")
                .unwrap_or_default(),
        );
    }

    pub fn clear(id: &str) {
        let Some((canvas, ctx)) = context(id) else {
            return;
        };
        paint_paper(&canvas, &ctx);
    }

    /// The pad is paper, not a themed surface: the value is a PNG, so ink and
    /// background must not depend on the theme or a signature drawn in one
    /// theme disappears in the other (black ink on the dark canvas). Every
    /// restore starts from white paper, which also rescues older signatures
    /// saved with a transparent background.
    fn paint_paper(canvas: &HtmlCanvasElement, ctx: &web_sys::CanvasRenderingContext2d) {
        ctx.set_fill_style_str("#ffffff");
        ctx.fill_rect(
            0.0,
            0.0,
            f64::from(canvas.width()),
            f64::from(canvas.height()),
        );
        ctx.set_stroke_style_str("#111111");
        ctx.set_line_width(2.0);
    }

    /// Repaint the canvas from a stored data URL. The load is asynchronous and
    /// browser-only, hence the closure (`once_into_js` leaks it into JS so it
    /// survives until the image lands — a dropped `Closure` would abort).
    pub fn restore(id: &str, data: String) {
        let Some((canvas, ctx)) = context(id) else {
            return;
        };
        paint_paper(&canvas, &ctx);
        if data.trim().is_empty() {
            return;
        }
        let Some(image) = web_sys::HtmlImageElement::new().ok() else {
            return;
        };
        image.set_src(&data);
        let ctx_draw = ctx.clone();
        let image_draw = image.clone();
        let onload = Closure::once_into_js(move || {
            let _ = ctx_draw.draw_image_with_html_image_element(&image_draw, 0.0, 0.0);
        });
        image.set_onload(Some(onload.unchecked_ref::<js_sys::Function>()));
    }
}
