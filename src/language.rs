//! The compiler half of the language (registered by the root's `build`).

use std::path::Path;
use std::sync::Arc;

use plugin_editor_api::{CompileDiagnostic, NativeRegistry, ScriptLanguage};
use pulsar_language::CheckContext;

use crate::compile::{
    check_class_source, compile_project_classes, validate_project_classes, CLASS_FILE, LANGUAGE_ID,
};
use crate::settings;

pub struct TypeScriptLanguage;

impl ScriptLanguage for TypeScriptLanguage {
    fn validate_project(&self, project_root: &Path) -> Result<(), String> {
        validate_project_classes(project_root, &NativeRegistry::with_engine_natives())
    }

    fn compile_project(
        &self,
        project_root: &Path,
        natives: &NativeRegistry,
    ) -> Vec<CompileDiagnostic> {
        compile_project_classes(project_root, natives)
    }

    /// The plugin's own setting: on by default.
    fn live_check_enabled(&self) -> bool {
        settings::compiler_diagnostics_enabled()
    }

    /// The compiler's verdict on a class's unsaved `class.ts`. Only a class
    /// this language owns has one: any other `.ts` file is the language
    /// server's alone.
    fn check_source(&self, ctx: &CheckContext, path: &Path, text: &str) -> Vec<CompileDiagnostic> {
        if !settings::compiler_diagnostics_enabled() {
            return Vec::new();
        }
        let Some(dir) = path.parent() else {
            return Vec::new();
        };
        if path.file_name().and_then(|n| n.to_str()) != Some(CLASS_FILE)
            || !pulsar_class::is_owned_by(dir, LANGUAGE_ID)
        {
            return Vec::new();
        }
        let _ = ctx.project_root;
        check_class_source(dir, text, ctx.natives)
    }
}

/// The TypeScript compiler, for hosts that embed this plugin.
pub fn script_language() -> Arc<dyn ScriptLanguage> {
    Arc::new(TypeScriptLanguage)
}
