//! The plugin's own settings (the server's standard group is derived by the
//! host from the `Language`): whether the Pulsar compiler's problems are shown
//! as you type.

use pulsar_config::{FieldType, NamespaceSchema, SchemaEntry};

/// The key of the compiler-diagnostics switch.
pub(crate) const COMPILER_DIAGNOSTICS: &str = "compiler_diagnostics";

/// The plugin's main settings group (page "Plugin: TypeScript").
pub(crate) fn schema() -> NamespaceSchema {
    NamespaceSchema::new(
        "TypeScript",
        "TypeScript classes: language server diagnostics and the Pulsar compiler's",
    )
    .setting(
        COMPILER_DIAGNOSTICS,
        SchemaEntry::new(
            "Show the Pulsar compiler's problems as you type, beside the language server's. The language server accepts TypeScript the script subset does not (imports, arrow functions, generics); with this off, those only fail at Play.",
            true,
        )
        .label("Compiler diagnostics")
        .page("Diagnostics")
        .field_type(FieldType::Checkbox),
    )
}

/// Whether compiler diagnostics are on: the person's setting, on by default
/// (and while the group is not registered).
pub(crate) fn compiler_diagnostics_enabled() -> bool {
    pulsar_settings::plugin::handle(pulsar_plugins::origin!(), "")
        .and_then(|handle| handle.get_bool(COMPILER_DIAGNOSTICS))
        .unwrap_or(true)
}
