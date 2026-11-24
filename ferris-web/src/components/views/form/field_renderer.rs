//! Form view field renderer.
//!
//! Routes a [`FormField`] to the component that fits its `fieldtype`, and wraps
//! the result in the label / required marker / error / description chrome the
//! form shares. Every `fieldtype` the framework ships has an explicit branch
//! naming a real component from [`crate::components::views::form::field_types`];
//! adding a type means adding a branch. The only catch-all is the final `_`,
//! which renders a plain text input for a type the Desk does not know (a
//! plugin's newer fieldtype) rather than panicking — never silently in the
//! normal case, only for a genuinely unknown one.

use crate::components::ui::input::{Input, InputType};
use crate::components::views::form::field_types::*;
use crate::views::services::FormField;
use ferris_web_ui::catalog::controls::FIELD;
use leptos::prelude::*;
use tw_merge::tw_merge;

/// Render a single form field based on its type.
pub fn render_form_field(f: &FormField) -> AnyView {
    let ft = f.fieldtype.as_str();
    // Layout markers carry no value and no control of their own; the section
    // builder has already used them to arrange the form. `HTML` and `Button`
    // are *not* in here: they render real content (an HTML block, an action
    // button) and reach their own arms below.
    if matches!(ft, "Tab Break" | "Section Break" | "Column Break") {
        // An empty fragment: renders no node at all, which is what a layout
        // marker is. `view! {}`/`view! { <></> }` would be the same thing but
        // as a unit expression, which the unit-argument lint flags.
        return AnyView::from(Fragment::new(Vec::new()));
    }

    let id = format!("field-{}", f.fieldname);
    let field_id = id.clone();
    let error_id = format!("{id}-error");
    let description_id = format!("{id}-description");
    let reqd = f.reqd;
    let label = f.label.clone();
    let description = StoredValue::new(f.description.clone().unwrap_or_default());
    let error = f.error;
    let read_only = f.read_only;
    let placeholder = f.placeholder.clone().unwrap_or_default();
    let options = f.options.clone().unwrap_or_default();
    let min = f.min_value.clone().unwrap_or_default();
    let max = f.max_value.clone().unwrap_or_default();
    let max_length = f.max_length;
    let precision = f.precision;
    // The invalid state and the ids travel to the *control*: `aria-invalid` on
    // a non-focusable group is not announced when the user reaches the input.
    let invalid = Signal::derive(move || error.get().is_some());
    let described_by = format!("{error_id} {description_id}");
    let label_id = format!("{id}-label");

    // The `tags` standard field is a plain comma-separated `Data` field on the
    // wire (the engine derives `Tag`/`Tag Link` from it), but the control is a
    // real multi picker over the `Tag` documents: a free-text input for a
    // reference the rest of the Desk already treats as a catalogue is the kind
    // of inconsistency that makes two screens disagree.
    if f.fieldname == "tags" {
        return view! {
            <div class=tw_merge!(FIELD, "flex min-w-0 flex-col gap-1.5") data-name="FormField">
                <label class="flex min-w-0 flex-wrap items-center gap-1.5 text-sm" for=id.clone()>
                    {label}
                </label>
                <TagsField id=id.clone() value=f.value read_only=read_only />
                <Show when=move || !description.with_value(String::is_empty)>
                    <p class="text-xs text-muted-foreground">{move || description.get_value()}</p>
                </Show>
            </div>
        }
        .into_any();
    }

    // A `Heading` is not a value: render it as the caption it is and skip the
    // label/error chrome entirely.
    if ft == "Heading" {
        return view! {
            <h3 class="pt-2 text-base font-semibold text-foreground" data-name="FormHeading">
                {label}
            </h3>
        }
        .into_any();
    }

    let control = match ft {
        "Check" => view! {
            <CheckField
                id=id
                checked=f.checked
                read_only=read_only
                label=label.clone()
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                aria_required=reqd
            />
        }
        .into_any(),

        "Rating" => view! {
            <RatingField
                id=id
                value=f.value
                read_only=read_only
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Select" => view! {
            <SelectField
                id=id
                value=f.value
                read_only=read_only
                options=options
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Link" | "Dynamic Link" => {
            // `Dynamic Link`'s target lives in another field, so it cannot be
            // resolved here; the widget stays usable as typed text and simply
            // does not search.
            let target = if ft == "Dynamic Link"
                || !options
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                String::new()
            } else {
                options.clone()
            };
            view! {
                <LinkFieldComponent
                    id=id
                    value=f.value
                    placeholder=placeholder
                    read_only=read_only
                    target=target
                    aria_invalid=invalid
                    aria_describedby=described_by.clone()
                    required=reqd
                />
            }
            .into_any()
        }

        "Table MultiSelect" => view! {
            <MultiLinkField
                id=id
                value=f.value
                placeholder=placeholder
                read_only=read_only
                target=options
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Text" | "Small Text" | "Long Text" => {
            let rows = if ft == "Long Text" { 4 } else { 3 };
            view! {
                <TextField
                    id=id
                    value=f.value
                    read_only=read_only
                    placeholder=placeholder
                    rows=rows
                    max_length=max_length
                    aria_invalid=invalid
                    aria_describedby=described_by.clone()
                    required=reqd
                />
            }
            .into_any()
        }

        // The two rich editors are their own widgets: Quill renders
        // the stored HTML, the markdown field swaps the textarea for a
        // marked-rendered preview on demand.
        "Text Editor" => view! {
            <TextEditorField
                id=id
                value=f.value
                read_only=read_only
                aria_label=label.clone()
            />
        }
        .into_any(),

        "Markdown Editor" => view! {
            <MarkdownField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                rows=6
                max_length=max_length
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Code" | "HTML Editor" => view! {
            <CodeField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                rows=6
                max_length=max_length
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "JSON" => view! {
            <JsonField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Password" | "Secret" => view! {
            <PasswordField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                max_length=max_length
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Currency" => view! {
            <CurrencyField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                min=min
                max=max
                prefix=if options.is_empty() {
                    "$".to_string()
                } else {
                    options.clone()
                }
                precision=precision
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Percent" => view! {
            <PercentField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                min=min
                max=max
                precision=precision
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Int" => view! {
            <NumberField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                min=min
                max=max
                step="1".to_string()
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Float" | "Duration" => view! {
            <NumberField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                min=min
                max=max
                step="any".to_string()
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Date" => view! {
            <DateField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Datetime" => view! {
            <DateTimeField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Time" => view! {
            <TimeField id=id value=f.value read_only=read_only />
        }
        .into_any(),

        "Week" => view! {
            <WeekField id=id value=f.value read_only=read_only />
        }
        .into_any(),

        "Email" => view! {
            <EmailField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Phone" => view! {
            <PhoneField
                id=id
                value=f.value
                read_only=read_only
                error=f.error
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "URL" => view! {
            <UrlField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Color" => view! {
            <ColorField
                id=id
                value=f.value
                read_only=read_only
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),

        "Geolocation" => view! {
            <GeolocationField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
            />
        }
        .into_any(),

        "Barcode" => view! {
            <BarcodeField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                options=options
            />
        }
        .into_any(),

        "Attach" | "File" => view! {
            <AttachField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
            />
        }
        .into_any(),

        "Image" | "Attach Image" => view! {
            <AttachField
                id=id
                value=f.value
                read_only=read_only
                placeholder=placeholder
                image=true
            />
        }
        .into_any(),

        // The gallery has no value of its own: the files live in the
        // engagement store, keyed by the document (see `gallery.rs`).
        "Attachment Gallery" => view! {
            <AttachmentGalleryField id=id read_only=read_only aria_label=label.clone() />
        }
        .into_any(),

        "Signature" => view! {
            <SignatureField
                id=id
                value=f.value
                read_only=read_only
                aria_label=label.clone()
            />
        }
        .into_any(),

        // Not inputs: a trusted HTML block from the metadata, and an action
        // that calls `/api/method/<options>` on the document.
        "HTML" => view! {
            <HtmlBlockField id=id html=options />
        }
        .into_any(),

        "Button" => view! {
            <ButtonField
                id=id
                label=if label.is_empty() {
                    options.clone()
                } else {
                    label.clone()
                }
                method=options
            />
        }
        .into_any(),

        "Read Only" => view! {
            <ReadOnlyField id=id value=f.value aria_label=label.clone() />
        }
        .into_any(),

        // A widget rendered in place: `options` names `<app>/<widget>` (or a
        // widget id). It is the same host the dashboard grid and the slot
        // system mount, and it carries no value — `build_form_payload` and
        // `validate_form` skip the type.
        "Widget" => view! {
            <WidgetField options=options label=label.clone() />
        }
        .into_any(),

        // The section builder normally routes a child table into its own
        // notice; this is the fallback when the renderer is reached directly.
        "Table" => view! {
            <TableNoticeField label=f.label.clone() />
        }
        .into_any(),

        // `Data` is the generic text input, so it has no arm of its own and
        // lands here. It is the *only* canonical fieldtype that does: every
        // other name in `ferris_schema::fieldtype::FIELDTYPES` is matched
        // explicitly above, which is what keeps a new type from degrading to
        // plain text just because nobody wrote its widget yet.
        _ => view! {
            <Input
                id=id
                r#type=InputType::Text
                bind_value=f.value
                readonly=read_only
                placeholder=placeholder
                attr:maxlength=max_length
                aria_invalid=invalid
                aria_describedby=described_by.clone()
                required=reqd
            />
        }
        .into_any(),
    };

    // The label is chrome for a `text-sm` control: without the size it
    // inherited the body's 16px and every form read one step larger than its
    // inputs.
    let label_class = if f.bold {
        "flex min-w-0 flex-wrap items-center gap-1.5 text-sm font-semibold"
    } else {
        "flex min-w-0 flex-wrap items-center gap-1.5 text-sm"
    };
    // A `Button` prints its label as the button's own text, and a `Widget` host
    // paints it as the widget's heading: a caption above either would show the
    // same word twice (and the `<label for>` would point at no control).
    let caption = !matches!(ft, "Button" | "Widget");
    // Only a real form control can be named with `<label for>`. The rest carry
    // their name as `aria-label` on the widget itself (rating, signature,
    // gallery, read-only and the rich editor), so pointing `for` at their
    // container would name nothing.
    let labelable = !matches!(
        ft,
        "Rating"
            | "Signature"
            | "Attachment Gallery"
            | "Read Only"
            | "HTML"
            | "Text Editor"
            | "Button"
            | "Widget"
            | "Heading"
    );

    // Clones for the `aria-describedby` closure: the ids are also written into
    // the elements they name, and a moved `String` cannot be used twice.
    let described_error_id = error_id.clone();
    let described_description_id = description_id.clone();

    let field_class = tw_merge!(FIELD, "flex min-w-0 flex-col gap-1.5");
    view! {
        <div
            class=field_class
            data-name="FormField"
            role="group"
            data-invalid=move || error.get().is_some().to_string()
            aria-invalid=move || error.get().is_some().to_string()
            aria-required=move || reqd.to_string()
            aria-describedby=move || {
                // Both ids exist in the markup exactly when their text is
                // rendered, so the reference follows the same condition.
                let described = !description.with_value(String::is_empty);
                match (error.get().is_some(), described) {
                    (true, true) => {
                        format!("{described_error_id} {described_description_id}")
                    }
                    (true, false) => described_error_id.clone(),
                    (false, true) => described_description_id.clone(),
                    (false, false) => String::new(),
                }
            }
        >
            <Show when=move || caption>
                <label
                    id=label_id.clone()
                    for=labelable.then(|| field_id.clone())
                    class=label_class
                >
                    <span class="min-w-0 break-words">{label.clone()}</span>
                    <Show when=move || reqd>
                        <span class="text-destructive">" *"</span>
                    </Show>
                </label>
            </Show>
            {control}
            <div class="flex min-w-0 flex-col gap-0.5">
                <Show when=move || error.get().is_some()>
                    <p id=error_id.clone() class="text-xs break-words text-destructive" role="alert">
                        {move || error.get().unwrap_or_default()}
                    </p>
                </Show>
                <Show when=move || !description.with_value(String::is_empty)>
                    <p id=description_id.clone() class="text-xs break-words text-muted-foreground">
                        {move || description.with_value(String::clone)}
                    </p>
                </Show>
            </div>
        </div>
    }
    .into_any()
}
