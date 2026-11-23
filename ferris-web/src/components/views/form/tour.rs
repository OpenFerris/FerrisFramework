//! Form tours: load the stored `Form Tour` rows and run the overlay.
//!
//! The data is site metadata, not form state: the tour is fetched once per
//! doctype and the overlay is the shared [`Tour`] component. A `DocType` with no
//! tours renders neither the button nor an empty overlay, so the feature is
//! invisible until someone declares a tour.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use crate::api::DocStore;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::components::ui::tour::{Tour, TourStep};
use crate::i18n::{t, use_i18n};
use crate::views::services::FormService;

/// The "Tour" button and its overlay for one doctype.
#[component]
pub fn FormTourLauncher(
    #[prop(into)] doctype: Signal<String>,
    service: FormService,
) -> impl IntoView {
    let tour_i18n = use_i18n();
    let steps = RwSignal::new(Vec::<TourStep>::new());
    let open = RwSignal::new(false);
    let save_on_completion = RwSignal::new(false);
    let loaded_for = StoredValue::new(String::new());
    let service = StoredValue::new(service);

    Effect::new(move |_| {
        let dt = doctype.get();
        if dt.is_empty() || loaded_for.with_value(|loaded| loaded == &dt) {
            return;
        }
        loaded_for.set_value(dt.clone());
        steps.set(Vec::new());
        let i18n = tour_i18n;
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            let Ok(tours) = store.form_tours(&dt).await else {
                return;
            };
            let Some(first) = tours.first() else {
                return;
            };
            let parsed = tour_steps(first, &i18n);
            if !parsed.is_empty() {
                save_on_completion.set(toggle(first, "save_on_completion"));
                steps.set(parsed);
            }
        });
    });

    view! {
        <Show when=move || !steps.get().is_empty()>
            <Button
                variant=ButtonVariant::Ghost
                size=ButtonSize::Sm
                attr:data-name="FormTourTrigger"
                on:click=move |_| open.set(true)
            >
                <Icon name="map" class="size-4" />
                {t("components.form.tour.start")}
            </Button>
            <Tour
                steps=steps.into()
                open=open
                on_finish=Callback::new(move |()| {
                    if save_on_completion.get_untracked() {
                        service.get_value().save();
                    }
                })
            />
        </Show>
    }
}

/// Project one stored `Form Tour` document onto the overlay steps.
///
/// The child table is stored as an array of objects; older rows may carry it as
/// a JSON string, so both shapes are accepted. A step without a `fieldname` is
/// skipped: there is nothing to highlight and a tour that points at the whole
/// page is noise.
#[must_use]
pub fn tour_steps(tour: &Value, i18n: &(impl crate::i18n::Translate + Sync)) -> Vec<TourStep> {
    let raw = tour.get("steps").cloned().unwrap_or(Value::Null);
    let rows = match raw {
        Value::Array(rows) => rows,
        Value::String(text) => serde_json::from_str::<Vec<Value>>(&text).unwrap_or_default(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => Vec::new(),
    };
    let mut steps: Vec<TourStep> = rows
        .into_iter()
        .filter_map(|row| {
            let fieldname = row.get("fieldname").and_then(Value::as_str)?.trim();
            if fieldname.is_empty() {
                return None;
            }
            let title = row
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(fieldname);
            let description = row
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let position = row
                .get("position")
                .and_then(Value::as_str)
                .unwrap_or("bottom");
            Some(
                TourStep::new(format!("[data-fieldname=\"{fieldname}\"]"), title)
                    .description(description)
                    .position(position),
            )
        })
        .collect();
    // Frappe's `Include Name Field` puts the id first; the synthetic field is
    // rendered with `data-fieldname="name"` like any other.
    if toggle(tour, "include_name_field") {
        steps.insert(
            0,
            TourStep::new(
                "[data-fieldname=\"name\"]",
                i18n.t("components.form.tour.name_title"),
            )
            .description(i18n.t("components.form.tour.name_description")),
        );
    }
    steps
}

/// A stored boolean, accepting the three shapes the engine writes.
fn toggle(value: &Value, field: &str) -> bool {
    value.get(field).is_some_and(|value| match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_i64().is_some_and(|n| n != 0),
        Value::String(text) => text == "1" || text.eq_ignore_ascii_case("true"),
        Value::Null | Value::Array(_) | Value::Object(_) => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_table_row_becomes_a_selector() {
        let tour = serde_json::json!({
            "steps": [{
                "fieldname": "status",
                "title": "Estado",
                "description": "Elige uno.",
                "position": "top"
            }]
        });
        let steps = tour_steps(&tour, &crate::i18n::DeskI18n::default());
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].selector, "[data-fieldname=\"status\"]");
        assert_eq!(steps[0].position, "top");
    }

    #[test]
    fn include_name_field_prepends_the_id_step() {
        let tour = serde_json::json!({
            "include_name_field": 1,
            "steps": [{"fieldname": "status", "title": "Estado"}]
        });
        let steps = tour_steps(&tour, &crate::i18n::DeskI18n::default());
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].selector, "[data-fieldname=\"name\"]");
        assert_eq!(steps[1].selector, "[data-fieldname=\"status\"]");
    }

    #[test]
    fn a_json_string_and_an_empty_row_are_tolerated() {
        let encoded = serde_json::json!({
            "steps": "[{\"fieldname\":\"title\"},{\"title\":\"sin campo\"}]"
        });
        assert_eq!(
            tour_steps(&encoded, &crate::i18n::DeskI18n::default()).len(),
            1
        );
        assert!(tour_steps(&serde_json::json!({}), &crate::i18n::DeskI18n::default()).is_empty());
    }
}
