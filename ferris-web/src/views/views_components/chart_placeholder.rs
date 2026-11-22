//! Chart card powered by `ui::charts::AreaChart`.

use leptos::prelude::*;

use crate::components::ui::card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
use crate::components::ui::charts::{AreaChart, ChartCurve};

#[component]
pub fn ChartPlaceholder(
    #[prop(into)] title: String,
    #[prop(into, optional)] subtitle: String,
    #[prop(optional)] values: Option<Signal<String>>,
    #[prop(optional)] labels: Option<Signal<String>>,
) -> impl IntoView {
    let empty_values = Signal::derive(move || "[]".to_string());
    let empty_labels = Signal::derive(move || "[]".to_string());
    let show_sub = !subtitle.is_empty();
    let sub_signal = Signal::derive(move || subtitle.clone());

    view! {
        <Card class="h-full">
            <CardHeader>
                <CardTitle>{move || title.clone()}</CardTitle>
                <Show when=move || show_sub>
                    <CardDescription>{move || sub_signal.get()}</CardDescription>
                </Show>
            </CardHeader>
            <CardContent>
                <AreaChart
                    json_values={values.unwrap_or(empty_values)}
                    json_labels={labels.unwrap_or(empty_labels)}
                    curve=ChartCurve::Smooth
                    class=Signal::derive(move || "h-64".to_string())
                    gradient=true
                    show_grid=true
                />
            </CardContent>
        </Card>
    }
}
