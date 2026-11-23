//! `Geolocation` field: a Leaflet map that plots the stored value and writes
//! back what the user draws or drops.
//!
//! The value is GeoJSON — a `FeatureCollection` once anything has been drawn —
//! with the historical `"lat,lon"` pair still accepted for compatibility with
//! Frappe-era data: it is plotted as a single marker and upgraded to GeoJSON
//! the next time the user touches the map. The raw input stays visible and
//! editable, which is also the no-JS fallback (SSR shows the box and the input;
//! the map upgrades in place once the bundle settles).
//!
//! Everything that talks to Leaflet goes through [`crate::ext::vendor`]'s lazy
//! loader and its tiny fallible interop, so a failed bundle load degrades to
//! the input instead of a spinner that never resolves. The parsing half is
//! plain Rust precisely so it is unit-tested natively.

use leptos::prelude::*;

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{Input, InputType};
use crate::i18n::t;

/// What a `Geolocation` value can say. Only the wasm side plots it and the
/// tests exercise it, so it does not exist in a native build at all.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, PartialEq)]
enum GeoValue {
    /// Empty or unplotable: nothing on the map (the raw text stays editable).
    Nothing,
    /// The historical `"lat,lon"` pair, validated against the coordinate
    /// ranges — plotted as a single marker.
    LatLon(f64, f64),
    /// GeoJSON text, handed to `L.geoJSON` verbatim.
    GeoJson(String),
}

/// Parse the raw field value: GeoJSON object first, then the legacy pair.
///
/// Unparseable input is [`GeoValue::Nothing`] rather than an error — the map
/// simply shows nothing while the user keeps typing in the input.
#[cfg(any(target_arch = "wasm32", test))]
fn parse_geo_value(raw: &str) -> GeoValue {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return GeoValue::Nothing;
    }
    // A bare array is not valid GeoJSON at the root, so only objects pass.
    if trimmed.starts_with('{') && serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return GeoValue::GeoJson(trimmed.to_string());
    }
    let mut parts = trimmed.split(',');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(lat), Some(lon), None) => {
            match (lat.trim().parse::<f64>(), lon.trim().parse::<f64>()) {
                (Ok(lat), Ok(lon))
                    if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) =>
                {
                    GeoValue::LatLon(lat, lon)
                }
                _ => GeoValue::Nothing,
            }
        }
        _ => GeoValue::Nothing,
    }
}

#[component]
pub fn GeolocationField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
) -> impl IntoView {
    let map_id = format!("{id}-map");
    // Init once the bundle settles (runs once: it tracks nothing); repaint on
    // every change of the value; dispose on unmount — Leaflet attaches window
    // listeners a bare DOM removal would leave behind.
    {
        let init_id = map_id.clone();
        Effect::new(move |_| init_when_ready(&init_id, value, read_only));
    }
    let repaint_id = map_id.clone();
    Effect::new(move |_| repaint(&repaint_id, &value.get()));
    // The cleanup runs before the view consumes `map_id`, so it gets its own
    // copy.
    let cleanup_id = map_id.clone();
    on_cleanup(move || dispose_map(&cleanup_id));

    view! {
        <div class="flex flex-col gap-2" data-name="__GeolocationField">
            <div
                id=map_id
                class="h-64 w-full overflow-hidden rounded-md border bg-card"
                role="region"
                aria-label=t("components.form.field.map")
            ></div>
            <div class="flex items-center gap-2">
                <Input
                    id=id
                    r#type=InputType::Text
                    bind_value=value
                    readonly=read_only
                    placeholder=if placeholder.is_empty() {
                        t("components.form.field.geo_placeholder")
                    } else {
                        placeholder
                    }
                />
                <Show when=move || !read_only>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on:click=move |_| value.set(String::new())
                    >
                        {t("components.form.field.clear")}
                    </Button>
                </Show>
            </div>
        </div>
    }
}

/// Create the map the first time Leaflet is on the page.
#[cfg(target_arch = "wasm32")]
fn init_when_ready(map_id: &str, value: RwSignal<String>, read_only: bool) {
    let map_id = map_id.to_string();
    crate::ext::vendor::ensure(crate::ext::vendor::Vendor::Leaflet, move || {
        wasm::init_map(&map_id, value, read_only);
    });
}

/// Native twin: SSR renders the map box and the raw input; effects do not run
/// there, so there is nothing to initialize.
#[cfg(not(target_arch = "wasm32"))]
fn init_when_ready(_map_id: &str, _value: RwSignal<String>, _read_only: bool) {}

/// Repaint the map from the current value.
#[cfg(target_arch = "wasm32")]
fn repaint(map_id: &str, raw: &str) {
    wasm::repaint(map_id, raw);
}

/// Native twin of `repaint`.
#[cfg(not(target_arch = "wasm32"))]
fn repaint(_map_id: &str, _raw: &str) {}

/// Drop the map and everything Leaflet attached to the window.
#[cfg(target_arch = "wasm32")]
fn dispose_map(map_id: &str) {
    wasm::dispose(map_id);
}

/// Native twin of `dispose_map`.
#[cfg(not(target_arch = "wasm32"))]
fn dispose_map(_map_id: &str) {}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use leptos::prelude::*;
    use wasm_bindgen::{JsValue, closure::Closure};

    use super::{GeoValue, parse_geo_value};
    use crate::ext::vendor::js;
    use crate::ext::vendor::{self, Vendor};

    /// The single remote origin the CSP allows tile images from — keep in
    /// sync with `ferris-server/src/router.rs::content_security_policy`.
    const TILES: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
    const ATTRIBUTION: &str =
        "© <a href=\"https://www.openstreetmap.org/copyright\">OpenStreetMap</a> contributors";

    /// One live map. `_closures` are the draw-event handlers; they are dropped
    /// only after `map.remove()` has detached them — calling an invalidated
    /// wasm closure would abort the module.
    struct MapHandle {
        map: JsValue,
        group: JsValue,
        _closures: Vec<Closure<dyn FnMut(JsValue)>>,
    }

    thread_local! {
        static MAPS: RefCell<HashMap<String, MapHandle>> = RefCell::new(HashMap::new());
    }

    pub(super) fn init_map(map_id: &str, value: RwSignal<String>, read_only: bool) {
        let Some(leaflet) = vendor::global(Vendor::Leaflet) else {
            js::error("geolocation: the Leaflet global is missing after load");
            return;
        };
        let Some(container) = document().and_then(|doc| doc.get_element_by_id(map_id)) else {
            return; // unmounted before the bundle landed
        };
        let Some(map) = js::call(&leaflet, "map", &[container.into()]) else {
            js::error("geolocation: L.map(...) failed");
            return;
        };
        let tile_options = object(&[
            ("attribution", JsValue::from_str(ATTRIBUTION)),
            ("maxZoom", JsValue::from_f64(19.0)),
        ]);
        if let Some(tiles) = js::call(
            &leaflet,
            "tileLayer",
            &[JsValue::from_str(TILES), tile_options],
        ) {
            let _ = js::call(&map, "addLayer", &[tiles]);
        }

        // The default marker: Leaflet's URL auto-detection looks for its own
        // layout, which `/static/vendor/leaflet/images/` does not match, so
        // the paths are pinned explicitly.
        if let Some(icon) = js::get(&leaflet, "Icon").and_then(|icon| js::get(&icon, "Default")) {
            let icons = object(&[
                (
                    "iconUrl",
                    JsValue::from_str("/static/vendor/leaflet/images/marker-icon.png"),
                ),
                (
                    "iconRetinaUrl",
                    JsValue::from_str("/static/vendor/leaflet/images/marker-icon-2x.png"),
                ),
                (
                    "shadowUrl",
                    JsValue::from_str("/static/vendor/leaflet/images/marker-shadow.png"),
                ),
            ]);
            let _ = js::call(&icon, "mergeOptions", &[icons]);
        }

        let Some(group) = js::call(&leaflet, "featureGroup", &[]) else {
            js::error("geolocation: L.featureGroup(...) failed");
            return;
        };
        let _ = js::call(&map, "addLayer", &[group.clone()]);

        let mut closures = Vec::new();
        if !read_only {
            // leaflet-draw: point, line, area and rectangle; circles are out
            // of the GeoJSON dialect Frappe stores, so they stay disabled.
            let draw = object(&[
                ("marker", JsValue::TRUE),
                ("polyline", JsValue::TRUE),
                ("polygon", JsValue::TRUE),
                ("rectangle", JsValue::TRUE),
                ("circle", JsValue::FALSE),
                ("circlemarker", JsValue::FALSE),
            ]);
            let edit = object(&[("remove", JsValue::TRUE)]);
            let _ = js_sys::Reflect::set(&edit, &JsValue::from_str("featureGroup"), &group);
            let options = object(&[("draw", draw), ("edit", edit)]);
            if let Some(ctor) = js::get(&leaflet, "Control").and_then(|c| js::get(&c, "Draw")) {
                if let Some(control) = js::construct(&ctor, &[options]) {
                    let _ = js::call(&map, "addControl", &[control]);
                    // Every mutation funnels into one write: `draw:created`
                    // carries the new layer, the edited/deleted events just
                    // confirm a change Leaflet has already applied.
                    let on_draw = Closure::<dyn FnMut(JsValue)>::new({
                        let group = group.clone();
                        move |event: JsValue| {
                            if let Some(layer) = js::get(&event, "layer") {
                                let _ = js::call(&group, "addLayer", &[layer]);
                            }
                            write_group(&group, value);
                        }
                    });
                    for event in ["draw:created", "draw:edited", "draw:deleted"] {
                        let _ = js::call(
                            &map,
                            "on",
                            &[JsValue::from_str(event), on_draw.as_ref().clone()],
                        );
                    }
                    closures.push(on_draw);
                }
            }
        }

        // The container has its final size only now that it is on screen.
        let _ = js::call(&map, "invalidateSize", &[]);
        MAPS.with(|maps| {
            maps.borrow_mut().insert(
                map_id.to_string(),
                MapHandle {
                    map: map.clone(),
                    group,
                    _closures: closures,
                },
            );
        });
        // Paint whatever the document already stores.
        repaint(map_id, &value.get_untracked());
    }

    pub(super) fn repaint(map_id: &str, raw: &str) {
        // Clone the two handles out so no Leaflet call runs while the map
        // registry is borrowed.
        let live = MAPS.with(|maps| {
            maps.borrow()
                .get(map_id)
                .map(|handle| (handle.map.clone(), handle.group.clone()))
        });
        let Some((map, group)) = live else { return };
        let Some(leaflet) = vendor::global(Vendor::Leaflet) else {
            return;
        };
        let _ = js::call(&group, "clearLayers", &[]);
        match parse_geo_value(raw) {
            GeoValue::Nothing => {}
            GeoValue::LatLon(lat, lon) => {
                let latlng = js_sys::Array::new();
                latlng.push(&JsValue::from_f64(lat));
                latlng.push(&JsValue::from_f64(lon));
                if let Some(marker) = js::call(&leaflet, "marker", &[latlng.into()]) {
                    let _ = js::call(&group, "addLayer", &[marker]);
                }
            }
            GeoValue::GeoJson(text) => {
                // `parse_geo_value` already validated the JSON with serde.
                if let Ok(parsed) = js_sys::JSON::parse(&text) {
                    if let Some(layer) = js::call(&leaflet, "geoJSON", &[parsed]) {
                        let _ = js::call(&group, "addLayer", &[layer]);
                    }
                }
            }
        }
        // Frame everything on the map; an empty group has no valid bounds and
        // keeps whatever zoom the user left it at.
        if let Some(bounds) = js::call(&group, "getBounds", &[]) {
            let valid = js::call(&bounds, "isValid", &[])
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if valid {
                let padding = js_sys::Array::new();
                padding.push(&JsValue::from_f64(16.0));
                padding.push(&JsValue::from_f64(16.0));
                let options = object(&[("padding", padding.into())]);
                let _ = js::call(&map, "fitBounds", &[bounds, options]);
            }
        }
    }

    pub(super) fn dispose(map_id: &str) {
        if let Some(handle) = MAPS.with(|maps| maps.borrow_mut().remove(map_id)) {
            // `remove` detaches Leaflet's window and DOM listeners; only then
            // is it safe to invalidate the closures behind its events.
            let _ = js::call(&handle.map, "remove", &[]);
            drop(handle);
        }
    }

    /// Serialize the feature group back into the field value.
    fn write_group(group: &JsValue, value: RwSignal<String>) {
        let Some(geojson) = js::call(group, "toGeoJSON", &[]) else {
            return;
        };
        if let Ok(serialized) = js_sys::JSON::stringify(&geojson) {
            if let Some(text) = serialized.as_string() {
                value.set(text);
            }
        }
    }

    /// A JS object literal from `(key, value)` pairs. Keys come from consts
    /// only; no user data ever reaches `Reflect::set`.
    fn object(pairs: &[(&str, JsValue)]) -> JsValue {
        let obj = JsValue::from(js_sys::Object::new());
        for (key, val) in pairs {
            let _ = js_sys::Reflect::set(&obj, &JsValue::from_str(key), val);
        }
        obj
    }

    fn document() -> Option<web_sys::Document> {
        web_sys::window().and_then(|window| window.document())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_historical_latlon_pair() {
        assert_eq!(
            parse_geo_value("40.4168, -3.7038"),
            GeoValue::LatLon(40.4168, -3.7038)
        );
        assert_eq!(
            parse_geo_value("-33.8688,151.2093"),
            GeoValue::LatLon(-33.8688, 151.2093)
        );
    }

    #[test]
    fn rejects_out_of_range_or_partial_coordinates() {
        assert_eq!(parse_geo_value("91,0"), GeoValue::Nothing);
        assert_eq!(parse_geo_value("0,181"), GeoValue::Nothing);
        assert_eq!(parse_geo_value("40.4"), GeoValue::Nothing);
        assert_eq!(parse_geo_value("a,b"), GeoValue::Nothing);
        assert_eq!(parse_geo_value("1,2,3"), GeoValue::Nothing);
        assert_eq!(parse_geo_value("   "), GeoValue::Nothing);
    }

    #[test]
    fn passes_geojson_through_verbatim() {
        let raw = r#"{"type":"Point","coordinates":[-3.7038,40.4168]}"#;
        assert_eq!(parse_geo_value(raw), GeoValue::GeoJson(raw.to_string()));
        // Trimming happens, nothing else.
        assert_eq!(
            parse_geo_value(&format!("  {raw}  ")),
            GeoValue::GeoJson(raw.to_string())
        );
    }

    #[test]
    fn does_not_treat_broken_json_or_bare_arrays_as_geojson() {
        assert_eq!(parse_geo_value("{not json"), GeoValue::Nothing);
        assert_eq!(parse_geo_value("[1,2]"), GeoValue::Nothing);
    }
}
