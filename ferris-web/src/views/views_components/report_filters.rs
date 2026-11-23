//! Report filter card built on rust-ui components.

use icons::{Download, Play};
use leptos::prelude::*;

use crate::api::{Filter, FilterOp};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
use crate::components::ui::input::Input;
use crate::components::ui::select_native::SelectNative;
use crate::i18n::use_i18n;

#[component]
pub fn ReportFilters(
    filters: RwSignal<Vec<Filter>>,
    #[prop(into)] on_run: Callback<()>,
    #[prop(into)] on_export: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let period = RwSignal::new(String::new());
    let status = RwSignal::new(String::new());
    let keyword = RwSignal::new(String::new());

    let apply_report_filters = move || {
        let mut fs: Vec<Filter> = Vec::new();
        match period.get_untracked().as_str() {
            "today" => fs.push(Filter {
                field: "creation".into(),
                op: FilterOp::Gte,
                or: false,
                value: "today".into(),
            }),
            "7d" | "30d" | "90d" => fs.push(Filter {
                field: "creation".into(),
                op: FilterOp::Gte,
                or: false,
                value: period.get_untracked(),
            }),
            _ => {}
        }
        match status.get_untracked().as_str() {
            "draft" => fs.push(Filter {
                field: "docstatus".into(),
                op: FilterOp::Equals,
                or: false,
                value: "draft".into(),
            }),
            "submitted" => fs.push(Filter {
                field: "docstatus".into(),
                op: FilterOp::Equals,
                or: false,
                value: "submitted".into(),
            }),
            "cancelled" => fs.push(Filter {
                field: "docstatus".into(),
                op: FilterOp::Equals,
                or: false,
                value: "cancelled".into(),
            }),
            _ => {}
        }
        let kw = keyword.get_untracked();
        if !kw.is_empty() {
            fs.push(Filter {
                field: "name".into(),
                op: FilterOp::Like,
                or: false,
                value: format!("%{}%", kw),
            });
        }
        filters.set(fs);
        on_run.run(());
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle>{i18n.t("views.report.filters_title")}</CardTitle>
                <CardDescription>{i18n.t("views.report.filters_description")}</CardDescription>
            </CardHeader>
            <CardContent class="flex flex-wrap items-end gap-2">
                <div class="flex w-full min-w-0 flex-col gap-1 sm:w-auto">
                    <label for="report_period" class="text-xs font-medium text-muted-foreground">{i18n.t("views.report.period")}</label>
                    <SelectNative
                        id="report_period"
                        class="h-8"
                        value=period.read_only()
                        on_change=Callback::new(move |ev: leptos::ev::Event| {
                            period.set(event_target_value(&ev));
                        })
                    >
                        <option value="">{i18n.t("views.report.period_all")}</option>
                        <option value="today">{i18n.t("views.report.period_today")}</option>
                        <option value="7d">{i18n.t("views.report.period_7d")}</option>
                        <option value="30d">{i18n.t("views.report.period_30d")}</option>
                        <option value="90d">{i18n.t("views.report.period_90d")}</option>
                    </SelectNative>
                </div>
                <div class="flex w-full min-w-0 flex-col gap-1 sm:w-auto">
                    <label for="report_status" class="text-xs font-medium text-muted-foreground">{i18n.t("views.report.status")}</label>
                    <SelectNative
                        id="report_status"
                        class="h-8"
                        value=status.read_only()
                        on_change=Callback::new(move |ev: leptos::ev::Event| {
                            status.set(event_target_value(&ev));
                        })
                    >
                        <option value="">{i18n.t("views.report.any")}</option>
                        <option value="draft">{i18n.t("views.report.draft")}</option>
                        <option value="submitted">{i18n.t("views.report.submitted_one")}</option>
                        <option value="cancelled">{i18n.t("views.report.cancelled_one")}</option>
                    </SelectNative>
                </div>
                <div class="flex w-full min-w-0 flex-col gap-1 sm:w-auto sm:min-w-40">
                    <label for="report_search" class="text-xs font-medium text-muted-foreground">{i18n.t("views.report.name")}</label>
                    <Input id="report_search" class="h-8" bind_value=keyword placeholder=i18n.t("views.report.name_placeholder") />
                </div>
                <Button variant=ButtonVariant::Default size=ButtonSize::Sm class="w-full sm:w-auto" on:click=move |_| apply_report_filters()>
                    <Play class="size-4" />
                    {i18n.t("views.report.run")}
                </Button>
                <Button variant=ButtonVariant::Outline size=ButtonSize::Sm class="w-full sm:w-auto" on:click=move |_| on_export.run(())>
                    <Download class="size-4" />
                    {i18n.t("views.report.export")}
                </Button>
            </CardContent>
        </Card>
    }
}
