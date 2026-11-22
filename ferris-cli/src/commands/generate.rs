//! Generate command
//!
//! Real generators, no shell-outs to ghost binaries:
//! - `wit`        -> refresh `wit/ferris.wit` from the framework
//! - `schema`     -> `config/schema.json` from the in-process doctype registry
//! - `typescript` -> TS interfaces from the in-process doctype registry
//! - `templates`  -> built-in Typst print templates into `templates/print/`
//! - `api` / `models` / `routes` are honest errors: apps are WIT/WASM
//!   components and their doctypes live in the host runtime, not in the CLI
//!   process (the registry is empty here), so there is nothing to generate.

use super::*;
use crate::config::CliConfig;
use crate::project;
use ferris_schema::registry::list_doctypes_metas;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

pub fn run(args: super::GenerateArgs, config: &CliConfig) -> Result<()> {
    project::check_project_exists(&config.source.project_dir)?;

    match args.what {
        super::GenerateWhat::All => {
            generate_wit(&args)?;
            generate_templates(&args)?;
            generate_schema(&args)?;
            generate_typescript(&args)?;
        }
        super::GenerateWhat::Schema => {
            generate_schema(&args)?;
        }
        super::GenerateWhat::Api => {
            host_only("API routes")?;
        }
        super::GenerateWhat::Models => {
            host_only("models")?;
        }
        super::GenerateWhat::Routes => {
            host_only("routes")?;
        }
        super::GenerateWhat::Typescript => {
            generate_typescript(&args)?;
        }
        super::GenerateWhat::Wit => {
            generate_wit(&args)?;
        }
        super::GenerateWhat::Templates => {
            generate_templates(&args)?;
        }
    }

    info!("Code generation completed!");
    Ok(())
}

fn host_only(what: &str) -> Result<()> {
    Err(anyhow::anyhow!(
        "Cannot generate {what} from the CLI. Ferris apps are WIT/WASM \
         components: doctypes are declared in the app (wit/ferris.wit) and \
         registered inside the host runtime at load time, so the CLI process \
         has no doctype registry to generate from. See 'ferris generate \
         --what schema|typescript' for what can be generated here."
    ))
}

fn out_dir(args: &super::GenerateArgs) -> PathBuf {
    PathBuf::from(args.out_dir.clone().unwrap_or_else(|| ".".to_string()))
}

fn ensure_out(args: &super::GenerateArgs) -> Result<PathBuf> {
    let dir = out_dir(args);
    fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("could not create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn refuse_overwrite(path: &Path) -> Result<()> {
    if path.exists() {
        return Err(anyhow::anyhow!(
            "{} already exists. Pass --force to overwrite.",
            path.display()
        ));
    }
    Ok(())
}

fn generate_wit(args: &super::GenerateArgs) -> Result<()> {
    info!("Generating WIT interfaces...");
    let root = crate::framework::framework_root()
        .map_err(|e| anyhow::anyhow!("{e}. WIT is copied from the framework install."))?;
    let src = root.join(crate::framework::FRAMEWORK_WIT);
    if !src.is_file() {
        return Err(anyhow::anyhow!(
            "framework WIT not found at {}. Set FERRIS_HOME.",
            src.display()
        ));
    }
    let dest = out_dir(args).join("wit/ferris.wit");
    if !args.force {
        // `refuse_overwrite` is the one that looks; checking `exists()` here too
        // was the same question asked twice.
        refuse_overwrite(&dest)?;
    }
    fs::create_dir_all(crate::project::parent_dir(&dest))?;
    fs::copy(&src, &dest)?;
    info!("  wrote {}", dest.display());
    Ok(())
}

/// A build without the `print` feature has no templates to write. Fail loudly
/// instead of reporting a successful generation that produced nothing.
#[cfg(not(feature = "print"))]
fn generate_templates(_args: &super::GenerateArgs) -> Result<()> {
    Err(anyhow::anyhow!(
        "print templates require a build with `--features print`"
    ))
}

fn generate_schema(args: &super::GenerateArgs) -> Result<()> {
    info!("Generating schema...");
    let dir = ensure_out(args)?;

    let doctypes = list_doctypes_metas();
    if doctypes.is_empty() {
        warn!(
            "doctype registry is empty in the CLI process (apps are WIT/WASM \
             components); schema.json will contain no doctypes"
        );
    }

    let items: Vec<serde_json::Value> = doctypes
        .iter()
        .map(|d| {
            let fields: Vec<serde_json::Value> = d
                .fields
                .iter()
                .map(|f| {
                    json!({
                        "fieldname": f.fieldname,
                        "label": f.label,
                        "fieldtype": f.fieldtype,
                        "options": f.options,
                        "default": f.default,
                        "reqd": f.reqd,
                        "read_only": f.read_only,
                        "hidden": f.hidden,
                        "depends_on": f.depends_on,
                        "description": f.description,
                        "in_list_view": f.in_list_view,
                        "in_standard_filter": f.in_standard_filter,
                        "in_global_search": f.in_global_search,
                        "unique": f.unique,
                        "search_index": f.search_index,
                        "permlevel": f.permlevel,
                        "print_hide": f.print_hide,
                        "report_hide": f.report_hide,
                    })
                })
                .collect();
            json!({
                "name": d.name,
                "module": d.module,
                "description": d.description,
                "naming_series": d.naming_series,
                "workflow": d.workflow,
                "is_submittable": d.is_submittable,
                "is_tree": d.is_tree,
                "is_virtual": d.is_virtual,
                "is_single": d.is_single,
                "is_child_table": d.is_child_table,
                "track_changes": d.track_changes,
                "allow_rename": d.allow_rename,
                "allow_copy": d.allow_copy,
                "allow_delete": d.allow_delete,
                "allow_print": d.allow_print,
                "allow_email": d.allow_email,
                "allow_attach": d.allow_attach,
                "fields": fields,
            })
        })
        .collect();

    let document = json!({
        "format": "ferris-schema",
        "version": 1,
        "doctypes": items,
    });

    let dest = dir.join("config/schema.json");
    if !args.force {
        // `refuse_overwrite` is the one that looks; checking `exists()` here too
        // was the same question asked twice.
        refuse_overwrite(&dest)?;
    }
    fs::create_dir_all(crate::project::parent_dir(&dest))?;
    let contents = serde_json::to_string_pretty(&document)?;
    fs::write(&dest, contents)?;
    info!("  wrote {} ({} doctype(s))", dest.display(), doctypes.len());
    Ok(())
}

/// The TypeScript type the scaffolded guest uses for a `fieldtype`.
///
/// Matched on the canonical (Frappe, spaced) spelling the metadata carries.
fn ts_type(fieldtype: &str) -> &'static str {
    match fieldtype {
        "Check" => "boolean",
        "Int" | "Duration" | "Rating" | "Percent" | "Float" | "Currency" => "number",
        "Date" | "Datetime" | "Time" => "string",
        "Data" | "Small Text" | "Text" | "Long Text" | "Code" | "HTML" | "Markdown Editor"
        | "HTML Editor" | "Text Editor" | "Phone" | "Email" | "URL" | "Color" | "Barcode"
        | "Link" | "Dynamic Link" | "Image" | "File" | "Attach" | "Attach Image"
        | "Attachment Gallery" | "Signature" | "Week" | "Password" | "Secret" | "Select"
        | "Read Only" | "Heading" | "Button" => "string",
        "Table" | "Table MultiSelect" | "JSON" | "Geolocation" => "Record<string, unknown>",
        _ => "string",
    }
}

fn ts_entity(name: &str) -> String {
    let mut out = String::new();
    for part in name.split(['_', '-', ' ']) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.push_str(&chars.as_str().to_ascii_lowercase());
        }
    }
    if out.is_empty() {
        "DocType".to_string()
    } else {
        out
    }
}

fn generate_typescript(args: &super::GenerateArgs) -> Result<()> {
    info!("Generating TypeScript types...");
    let dir = ensure_out(args)?;

    let doctypes = list_doctypes_metas();
    let mut body = String::new();
    body.push_str("// Generated by `ferris generate typescript`. Do not edit by hand.\n");
    body.push_str("// Note: doctype metadata lives in the host runtime; the CLI process\n");
    body.push_str("// registers no app doctypes, so these are the framework built-ins only.\n");
    body.push('\n');

    for d in &doctypes {
        let name = ts_entity(&d.name);
        body.push_str(&format!("export interface {name} {{\n"));
        body.push_str("  name: string;\n");
        body.push_str("  owner: string;\n");
        body.push_str("  creation: string;\n");
        body.push_str("  modified: string;\n");
        body.push_str("  modified_by: string;\n");
        body.push_str("  docstatus: 0 | 1 | 2;\n");
        for f in &d.fields {
            let optional = if f.reqd { "" } else { "?" };
            body.push_str(&format!(
                "  {}: {}{};\n",
                f.fieldname,
                ts_type(&f.fieldtype),
                optional
            ));
        }
        body.push_str("}\n\n");
    }

    if doctypes.is_empty() {
        body.push_str("// No doctypes registered in the CLI process.\n");
    }

    let dest = dir.join("web/src/types/generated.ts");
    if !args.force {
        // `refuse_overwrite` is the one that looks; checking `exists()` here too
        // was the same question asked twice.
        refuse_overwrite(&dest)?;
    }
    fs::create_dir_all(crate::project::parent_dir(&dest))?;
    fs::write(&dest, body)?;
    info!("  wrote {}", dest.display());
    Ok(())
}

#[cfg(feature = "print")]
fn generate_templates(args: &super::GenerateArgs) -> Result<()> {
    info!("Generating print templates...");
    let dir = ensure_out(args)?;
    let templates_dir = dir.join("templates/print");
    fs::create_dir_all(&templates_dir)?;
    for (stem, _name, source) in ferris_print::BUILTIN_TEMPLATES {
        let dest = templates_dir.join(format!("{stem}.typ"));
        if !args.force {
            // Refuse everything before writing anything: this loop wrote file by
            // file, so a `--force`-less run against a directory holding one of
            // these templates used to leave the ones before it already replaced.
            refuse_overwrite(&dest)?;
        }
        fs::write(&dest, source)?;
        info!("  wrote {}", dest.display());
    }
    Ok(())
}
