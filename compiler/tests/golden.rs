//! The lowering's output is recorded: compiling the fixtures must give, byte
//! for byte, the modules checked in under `tests/golden/`. A change to how the
//! compiler (or the `ModuleBuilder` it is a client of) emits a module shows
//! here as a diff of the JSON. To accept an intended change, run with
//! `UPDATE_GOLDEN=1` and review the diff.

// A test reading and (on request) rewriting its own fixture files, not the engine's I/O.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use pulsar_script_ts::{compile_class, ClassSchema, ClassSource, SchemaField};
use pulsar_script_vm::NativeRegistry;

use pulsar_script_math as _;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn compiled_json(class_name: &str, source: &str) -> String {
    // A field's id is a fresh uuid until a schema holds it, so a class that is
    // renamed from an earlier field gets that earlier schema.
    let earlier = (class_name == "Hero").then(|| ClassSchema {
        version: 1,
        fields: vec![SchemaField {
            id: "id-hp".to_owned(),
            name: "hp".to_owned(),
            ty: "int".to_owned(),
        }],
    });
    let compiled = compile_class(
        &ClassSource {
            class_name,
            file: "class.ts",
            source,
            schema: earlier.as_ref(),
        },
        &NativeRegistry::with_engine_natives(),
    );
    assert!(
        compiled.diagnostics.is_empty(),
        "{class_name}: {:#?}",
        compiled.diagnostics
    );
    let mut module = compiled.module.expect("a module");
    // The one thing that is not deterministic: the ids of the fields that are new.
    for variable in &mut module.variables {
        let id = variable.id.as_mut().expect("every field has an id");
        if id != "id-hp" {
            *id = format!("id-{}", variable.name);
        }
    }
    // Binary and JSON are both recorded in one text: the binary is the shipped form.
    let mut json = module.to_json().expect("json");
    json.push('\n');
    json.push_str(&format!("binary: {}\n", hex(&module.to_binary())));
    json
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn check(name: &str, class_name: &str, source: &str) {
    let path = golden_dir().join(format!("{name}.module.txt"));
    let actual = compiled_json(class_name, source);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_GOLDEN=1)", path.display()))
        .replace("\r\n", "\n");
    assert!(
        actual == expected,
        "{name}: the emitted module changed (UPDATE_GOLDEN=1 accepts an intended change)"
    );
}

#[test]
fn the_constructs_fixture_is_emitted_unchanged() {
    let source = std::fs::read_to_string(golden_dir().join("Constructs.ts")).expect("fixture");
    check("Constructs", "Constructs", &source);
}

#[test]
fn the_example_classes_are_emitted_unchanged() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples_assets/examples");
    for class in ["Counter", "Door", "Hero", "Patrol"] {
        let source = std::fs::read_to_string(examples.join(class).join("class.ts")).expect("example");
        check(class, class, &source);
    }
}

#[test]
fn the_beacon_class_is_emitted_unchanged() {
    // The class `blueprint_parity` runs against the Blueprint of the same name.
    let source = r#"
export default class Beacon extends ScriptClass {
    log: string = "";
    done: boolean = false;
    flip: boolean = false;
    runs: int = 0;
    pending: boolean = false;

    async on_fire(): Promise<void> {
        if (!this.done) { this.done = true; this.log += "o"; }
        if (this.flip) { this.log += "a"; } else { this.log += "b"; }
        this.flip = !this.flip;
        if (this.runs < 2) { this.runs += 1; this.log += "n"; }
        if (!this.pending) {
            this.pending = true;
            await wait(0.5);
            this.pending = false;
            this.log += "d";
        }
    }
}
"#;
    check("Beacon", "Beacon", source);
}
