//! KPI stat card built on `ui::card`.

use leptos::prelude::*;

use crate::components::ui::card::{Card, CardContent, CardSize};
use crate::components::ui::icon::Icon;

#[derive(Clone, Debug)]
pub struct StatCard {
    pub label: String,
    // Reactive value: the wrapped closure reads the underlying signal inside
    // the card's `{move || ...}` (a tracking context), so report/dashboard
    // KPIs update when the data lands instead of freezing at mount time.
    pub value: Signal<String>,
    pub change: Option<f64>,
    pub icon: String,
}

impl StatCard {
    pub fn with_value(
        label: impl Into<String>,
        value: impl Fn() -> String + Send + Sync + 'static,
        change: Option<f64>,
        icon: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            value: Signal::derive(value),
            change,
            icon: icon.into(),
        }
    }
}

#[component]
pub fn StatCardView(stat: StatCard) -> impl IntoView {
    let change_text = move || match stat.change {
        Some(c) => format!("{c:+.1}%"),
        None => String::new(),
    };

    view! {
        <Card size=CardSize::Sm>
            <CardContent class="flex min-w-0 items-start justify-between gap-3">
                <div class="flex min-w-0 flex-col gap-1">
                    <span class="break-words text-sm text-muted-foreground">{move || stat.label.clone()}</span>
                    <span class="text-2xl font-semibold tabular-nums">{move || stat.value.get()}</span>
                    <span class=move || match stat.change {
                        Some(change) if change > 0.0 => "text-xs text-success",
                        Some(change) if change < 0.0 => "text-xs text-destructive",
                        _ => "text-xs text-muted-foreground",
                    }>{move || change_text()}</span>
                </div>
                <span class="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                    <Icon name=stat.icon.clone() class="size-4" />
                </span>
            </CardContent>
        </Card>
    }
}
