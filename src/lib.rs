//! TypeScript as a scripting language.
//!
//! A class is a directory `src/classes/<Class>/` holding `class.ts`. This
//! plugin compiles it (with `pulsar_script_ts`) to the same
//! engine script module a Blueprint class produces, in the same place (the
//! project's module store, `pulsar_class::modules`), so the engine
//! loads, links, runs and hot-reloads it exactly like one, and keeps each
//! field's identity in `class.schema.json` next to the source (commit it).
//!
//! It also writes `Pulsar/types/pulsar.d.ts`, generated from the same native
//! registry the compiler checks against, so an editor gets autocomplete that
//! cannot disagree with the compiler.
//!
//! # One language per class
//!
//! The host owns the rule (`pulsar_class::language`): a class has exactly one
//! language, decided by the marker files the registered languages declare. This
//! plugin names no other language: it enumerates the classes that are its alone
//! (`pulsar_class::is_owned_by`) and compiles those; a class that is two
//! languages', or whose module another language wrote, is the host's problem
//! to report.
//!
//! # The language server
//!
//! The same registration carries typescript-language-server ([`server::SPEC`]):
//! found through the person's setting, the project's `node_modules`, `PATH`
//! and then a managed install the person approves ([`install`]: pinned
//! versions, digests checked before anything is unpacked, never silent), and
//! launched through Node's entry point rather than the `.cmd` shim
//! ([`locate`]). A `tsconfig.json` for the class folder, pointing at
//! `Pulsar/types/pulsar.d.ts` with the DOM off, is written before it starts.
//! No Node, or no server, is an explicit status with what to do, never a silent
//! no-op.
//!
//! # Two sources of diagnostics
//!
//! The server accepts TypeScript the script subset rejects, so a class also
//! has the compiler's verdict on its unsaved text
//! ([`ScriptLanguage::check_source`](plugin_editor_api::ScriptLanguage), source
//! `Pulsar`), which the host publishes beside the server's. The plugin's own
//! setting, `compiler_diagnostics`, turns that off.
//!
//! This crate is the root of the `pulsar.typescript` plugin, an editor tool:
//! its root's `build` registers one [`Language`](plugin_editor_api::Language)
//! (identity, file extensions, the `class.ts` marker, the compiler and the
//! server) and the plugin's settings, gated by the plugin, so a project that
//! disables it has no TypeScript.

mod archive;
mod compile;
mod examples;
mod install;
mod language;
mod locate;
mod server;
mod settings;

pub use compile::{
    check_class_source, compile_class_dir, compile_project_classes, validate_project_classes,
    CLASS_FILE, SCHEMA_FILE,
};
pub use examples::EXAMPLES;
pub use language::{script_language, TypeScriptLanguage};
pub use server::{ID as SERVER_ID, SPEC};

// The plugin's descriptor: linking this crate installs it.
pulsar_plugins::installed!("pulsar-plugin.toml", build);

/// The language this plugin brings: one registration carrying everything.
pub const LANGUAGE: plugin_editor_api::Language = plugin_editor_api::Language {
    origin: pulsar_plugins::origin!(),
    id: compile::LANGUAGE_ID,
    display_name: "TypeScript",
    file_extensions: &["ts", "mts", "cts"],
    language_ids: &[],
    highlight_id: "typescript",
    class_markers: &[CLASS_FILE],
    config_files: &["tsconfig.json"],
    compiler: Some(script_language),
    server: Some(&SPEC),
};

/// What the plugin registers (this crate is its root): its examples and its
/// language.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    // The icon the editor shows beside this plugin (Extensions window).
    p.add(pulsar_plugins::PluginIcon {
        origin: p.origin(),
        format: pulsar_plugins::IconFormat::Png,
        bytes: include_bytes!("../icon.png"),
    });
    examples::build(p);
    p.add(LANGUAGE);
    p.add(pulsar_settings::plugin::PluginSettings {
        origin: p.origin(),
        id: "",
        schema: settings::schema,
    });
}

// The plugin as a library of its own, loaded by the editor from `plugins/editor`
// (`cargo build -p <this package> --release --features dll`).
#[cfg(feature = "dll")]
plugin_editor_api::export_library_plugin!();
