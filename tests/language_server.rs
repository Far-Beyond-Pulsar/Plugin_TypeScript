//! The TypeScript plugin's language server, end to end, with no Node install:
//! a scripted tsserver on a pipe (`pulsar_lsp::testing`) stands in for
//! typescript-language-server, and the real plugin (its `Language`, its spec,
//! its compiler) runs the rest.
//!
//! * a `.ts` class shows the server's diagnostics in the store under the
//!   server's source, and the workspace was prepared first (tsconfig, d.ts);
//! * a construct the server accepts and the Pulsar compiler rejects shows only
//!   the `Pulsar` source's error;
//! * a binary that is not found is an explicit, actionable status;
//! * (when Node is installed) one real spawn through Node's entry point, and,
//!   on Windows, through the `.cmd` fallback. CI does not depend on them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{AppContext as _, TestAppContext};
use plugin_typescript::{LANGUAGE, SPEC};
use pulsar_language::settings::ServerSettings;
use pulsar_lsp::testing::scripted_server;
use pulsar_lsp::{
    DiagnosticSource, DiagnosticsStore, LanguageServer, LanguageServers, ServerId, ServerStatus,
};
use pulsar_script_math as _;
use serde_json::{json, Value};

/// A class the Pulsar compiler accepts.
const DOOR: &str = r#"
export default class Door extends ScriptClass {
    opened: int = 0;
    open(): void { this.opened += 1; }
}
"#;

/// A class the language server accepts (an arrow function is plain
/// TypeScript) and the script subset rejects.
const DOOR_WITH_ARROW: &str = r#"
export default class Door extends ScriptClass {
    opened: int = 0;
    open(): void {
        const twice = (x: number) => x * 2;
        this.opened += 1;
    }
}
"#;

fn activate() {
    pulsar_plugins::activate(&pulsar_plugins::resolve(
        pulsar_plugins::installed(),
        &pulsar_plugins::ProjectPlugins::default(),
    ));
}

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
    class: PathBuf,
}

fn project(source: &str) -> Project {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let class_dir = root.join("src").join("classes").join("Door");
    std::fs::create_dir_all(&class_dir).unwrap();
    let class = class_dir.join("class.ts");
    std::fs::write(&class, source).unwrap();
    Project {
        _dir: dir,
        root,
        class,
    }
}

/// Run the executor until `done` holds (the server's thread and the editor
/// task pool are real threads; a bounded wait, never a sleep for its own sake).
fn wait_for(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    cx.executor().allow_parking();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn diagnostic_of(file: &Path, message: &str) -> Value {
    let _ = file;
    json!({
        "range": { "start": { "line": 3, "character": 4 }, "end": { "line": 3, "character": 9 } },
        "severity": 1,
        "code": 2322,
        "source": "ts",
        "message": message,
    })
}

/// A tsserver on a pipe. On `initialize` it answers with its capabilities;
/// after `initialized` it reports its loading work as work-done progress (and
/// asks for the progress token first, as the real one does); on `didOpen` it
/// publishes `publish` for the document.
fn scripted_tsserver(publish: Vec<Value>) -> pulsar_lsp::testing::ScriptedServer {
    scripted_server(move |message, reply| match message["method"].as_str() {
        Some("initialize") => reply.result(
            &message["id"],
            json!({ "capabilities": {
                "textDocumentSync": 2,
                "completionProvider": { "triggerCharacters": [".", "\"", "'", "/", "@", "<"] },
            }}),
        ),
        Some("initialized") => {
            reply.request(
                json!(900),
                "window/workDoneProgress/create",
                json!({ "token": "init" }),
            );
            reply.notify(
                "$/progress",
                json!({ "token": "init", "value": {
                    "kind": "begin", "title": "Initializing JS/TS language features" } }),
            );
            reply.notify(
                "$/progress",
                json!({ "token": "init", "value": { "kind": "end" } }),
            );
        }
        Some("textDocument/didOpen") => {
            let uri = message["params"]["textDocument"]["uri"].clone();
            reply.notify(
                "textDocument/publishDiagnostics",
                json!({ "uri": uri, "diagnostics": publish }),
            );
        }
        _ => {}
    })
}

/// A server of the real plugin for `project`, rooted where the plugin roots a
/// class file, writing to `store`.
fn server_for(
    cx: &mut TestAppContext,
    project: &Project,
    settings: ServerSettings,
    store: gpui::Entity<DiagnosticsStore>,
) -> gpui::Entity<LanguageServer> {
    let root = project.root.join("src").join("classes");
    cx.new(|_| {
        LanguageServer::new(
            &LANGUAGE,
            &SPEC,
            root,
            project.root.clone(),
            settings,
            store,
        )
    })
}

#[gpui::test]
fn a_ts_class_shows_the_servers_diagnostics_under_the_servers_source(cx: &mut TestAppContext) {
    activate();
    let project = project(DOOR);
    let store = cx.new(|_| DiagnosticsStore::new());
    let server = server_for(cx, &project, ServerSettings::default(), store.clone());

    let tsserver = scripted_tsserver(vec![diagnostic_of(
        &project.class,
        "Type 'string' is not assignable to type 'number'.",
    )]);
    let received = tsserver.received.clone();
    server.update(cx, |server, cx| {
        server.attach(tsserver.reader, tsserver.writer, cx)
    });

    // The server is ready when its loading work has ended (the rule of the
    // spec), not before and not by a timer.
    wait_for(cx, "the server to be ready", |cx| {
        server.read_with(cx, |server, _| *server.status() == ServerStatus::Ready)
    });

    // The handshake: rooted at the class folder, no typings downloads.
    let initialize = received
        .try_iter()
        .find(|m| m["method"] == "initialize")
        .expect("initialize was sent");
    let root_uri = initialize["params"]["rootUri"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(root_uri.ends_with("/src/classes"), "{root_uri}");
    assert_eq!(
        initialize["params"]["initializationOptions"]["disableAutomaticTypingAcquisition"],
        true
    );
    assert_eq!(
        initialize["params"]["initializationOptions"]["hostInfo"],
        "Pulsar"
    );

    // The workspace was prepared before the server started: the tsconfig for
    // the class folder and the declarations it points at.
    let tsconfig = std::fs::read_to_string(project.root.join("src/classes/tsconfig.json")).unwrap();
    assert!(
        tsconfig.contains("../../Pulsar/types/*.d.ts")
            && tsconfig.contains("\"lib\": [\"ES2022\"]")
    );
    assert!(project.root.join("Pulsar/types/pulsar.d.ts").is_file());

    // Open the class: the server publishes, and the store holds it under the
    // server's source.
    let text = std::fs::read_to_string(&project.class).unwrap();
    server.read_with(cx, |server, _| {
        assert_eq!(server.language_id_of(&project.class), "typescript");
        server
            .did_open_file(&project.class, &text, "typescript")
            .unwrap();
    });
    wait_for(cx, "the server's diagnostics", |cx| {
        !store
            .read_with(cx, |store, _| store.for_file(&project.class))
            .is_empty()
    });
    let entries = store.read_with(cx, |store, _| store.for_file(&project.class));
    assert_eq!(entries.len(), 1);
    let (source, diagnostic) = &entries[0];
    let id = ServerId {
        server: "typescript-language-server",
        root: project.root.join("src/classes"),
    };
    assert_eq!(
        *source,
        DiagnosticSource::Server(id),
        "grouped by its server in the drawer"
    );
    assert_eq!(source.label(), "typescript-language-server");
    assert_eq!(diagnostic.source.as_deref(), Some("ts"));
    assert_eq!((diagnostic.line, diagnostic.column), (4, 5));
    assert!(diagnostic.message.contains("not assignable"));

    // Stopping the server takes its diagnostics with it.
    server.update(cx, |server, cx| {
        server.stop(pulsar_lsp::StopReason::Requested, cx)
    });
    assert!(store
        .read_with(cx, |store, _| store.for_file(&project.class))
        .is_empty());
}

#[gpui::test]
fn a_construct_the_server_accepts_and_the_compiler_rejects_shows_only_the_pulsar_error(
    cx: &mut TestAppContext,
) {
    activate();
    let project = project(DOOR_WITH_ARROW);
    let service = cx.new(LanguageServers::new);
    let store = service.read_with(cx, |service, _| service.diagnostics());
    service.update(cx, |service, cx| {
        service.set_project(project.root.clone(), cx);
        service.set_check_debounce(Duration::from_millis(10));
    });

    // The server accepts the arrow function: it publishes nothing.
    let server = server_for(cx, &project, ServerSettings::default(), store.clone());
    let tsserver = scripted_tsserver(Vec::new());
    server.update(cx, |server, cx| {
        server.attach(tsserver.reader, tsserver.writer, cx)
    });
    wait_for(cx, "the server to be ready", |cx| {
        server.read_with(cx, |server, _| *server.status() == ServerStatus::Ready)
    });
    let text = std::fs::read_to_string(&project.class).unwrap();
    server.read_with(cx, |server, _| {
        server
            .did_open_file(&project.class, &text, "typescript")
            .unwrap()
    });

    // The editor hands over the (unsaved) text: the service checks it.
    assert!(service.read_with(cx, |service, _| service.has_compiler_for(&project.class)));
    service.update(cx, |service, cx| {
        service.check_source(&project.class, &text, cx)
    });
    cx.executor().advance_clock(Duration::from_secs(1));
    wait_for(cx, "the compiler's diagnostics", |cx| {
        !store
            .read_with(cx, |store, _| store.for_file(&project.class))
            .is_empty()
    });

    let entries = store.read_with(cx, |store, _| store.for_file(&project.class));
    assert_eq!(
        entries.len(),
        1,
        "only the compiler has something to say: {entries:#?}"
    );
    let (source, diagnostic) = &entries[0];
    assert_eq!(*source, DiagnosticSource::Compiler("typescript"));
    assert_eq!(diagnostic.source.as_deref(), Some("Pulsar"));
    assert_eq!(
        diagnostic.severity,
        ui::diagnostics::DiagnosticSeverity::Error
    );
    assert_eq!(
        diagnostic.line, 5,
        "the arrow function is on line 5: {diagnostic:?}"
    );
    assert!(
        diagnostic.end_column > Some(diagnostic.column),
        "a range, not a point"
    );
    assert!(!diagnostic.message.is_empty());

    // Fixing the class clears the compiler's entry (an empty result clears).
    service.update(cx, |service, cx| {
        service.check_source(&project.class, DOOR, cx)
    });
    cx.executor().advance_clock(Duration::from_secs(1));
    wait_for(cx, "the compiler's diagnostics to clear", |cx| {
        store
            .read_with(cx, |store, _| store.for_file(&project.class))
            .is_empty()
    });

    // Closing the file forgets it.
    service.update(cx, |service, cx| {
        service.check_source(&project.class, DOOR_WITH_ARROW, cx)
    });
    cx.executor().advance_clock(Duration::from_secs(1));
    wait_for(cx, "the compiler's diagnostics again", |cx| {
        !store
            .read_with(cx, |store, _| store.for_file(&project.class))
            .is_empty()
    });
    service.update(cx, |service, cx| service.forget_source(&project.class, cx));
    assert!(store
        .read_with(cx, |store, _| store.for_file(&project.class))
        .is_empty());
}

#[gpui::test]
fn the_servers_and_the_compilers_diagnostics_are_two_sources_of_one_file(cx: &mut TestAppContext) {
    activate();
    let project = project(DOOR_WITH_ARROW);
    let service = cx.new(LanguageServers::new);
    let store = service.read_with(cx, |service, _| service.diagnostics());
    service.update(cx, |service, cx| {
        service.set_project(project.root.clone(), cx);
        service.set_check_debounce(Duration::from_millis(10));
    });
    let server = server_for(cx, &project, ServerSettings::default(), store.clone());
    let tsserver = scripted_tsserver(vec![diagnostic_of(
        &project.class,
        "'twice' is declared but its value is never read.",
    )]);
    server.update(cx, |server, cx| {
        server.attach(tsserver.reader, tsserver.writer, cx)
    });
    wait_for(cx, "the server to be ready", |cx| {
        server.read_with(cx, |server, _| *server.status() == ServerStatus::Ready)
    });
    let text = std::fs::read_to_string(&project.class).unwrap();
    server.read_with(cx, |server, _| {
        server
            .did_open_file(&project.class, &text, "typescript")
            .unwrap()
    });
    service.update(cx, |service, cx| {
        service.check_source(&project.class, &text, cx)
    });
    cx.executor().advance_clock(Duration::from_secs(1));
    wait_for(cx, "both sources", |cx| {
        store
            .read_with(cx, |store, _| store.for_file(&project.class))
            .len()
            == 2
    });
    let entries = store.read_with(cx, |store, _| store.for_file(&project.class));
    let labels: Vec<String> = entries.iter().map(|(source, _)| source.label()).collect();
    assert!(
        labels.contains(&"typescript-language-server".to_string()),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"typescript compiler".to_string()),
        "{labels:?}"
    );
}

#[gpui::test]
fn a_server_that_is_not_found_is_an_actionable_status_never_a_silent_no_op(
    cx: &mut TestAppContext,
) {
    activate();
    let project = project(DOOR);
    let store = cx.new(|_| DiagnosticsStore::new());
    let settings = ServerSettings {
        path: Some(project.root.join("no-such-server").join("cli.mjs")),
        ..ServerSettings::default()
    };
    let server = server_for(cx, &project, settings, store);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&server, move |_, event: &pulsar_lsp::ServerEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
    });
    server.update(cx, |server, cx| server.start(cx));
    wait_for(cx, "the missing status", |cx| {
        matches!(
            server.read_with(cx, |server, _| server.status().clone()),
            ServerStatus::Missing(_)
        )
    });
    let ServerStatus::Missing(message) = server.read_with(cx, |server, _| server.status().clone())
    else {
        unreachable!()
    };
    assert!(
        message.contains("does not exist") && message.contains("Settings"),
        "{message}"
    );
    assert!(
        events
            .borrow()
            .iter()
            .any(|event| matches!(event, pulsar_lsp::ServerEvent::Error(m) if *m == message)),
        "the toast carries the same words"
    );
    // The footer offers the managed install: it is on the spec, and never runs by itself.
    let installer = server
        .read_with(cx, |server, _| server.installer())
        .expect("an install is offered");
    assert_eq!(installer.label, "Install typescript-language-server");
    let described = (installer.describe)();
    assert!(
        described.contains("registry.npmjs.org") && described.contains("pinned"),
        "{described}"
    );
}

// ---- real spawns (need Node; skipped without it) -----------------------------

/// A typescript-language-server stand-in written in JavaScript: the framing,
/// the handshake, the progress, and one diagnostic per opened document.
const FAKE_CLI: &str = r#"
let buf = Buffer.alloc(0);
const send = (m) => { const s = JSON.stringify(m); process.stdout.write(`Content-Length: ${Buffer.byteLength(s)}\r\n\r\n${s}`); };
process.stdin.on('data', (d) => {
  buf = Buffer.concat([buf, d]);
  for (;;) {
    const i = buf.indexOf('\r\n\r\n'); if (i < 0) return;
    const n = Number(/Content-Length: (\d+)/i.exec(buf.slice(0, i).toString())[1]);
    if (buf.length < i + 4 + n) return;
    const m = JSON.parse(buf.slice(i + 4, i + 4 + n).toString());
    buf = buf.slice(i + 4 + n);
    if (m.method === 'initialize') {
      send({ jsonrpc: '2.0', id: m.id, result: { capabilities: { textDocumentSync: 2, completionProvider: { triggerCharacters: ['.'] } } } });
    } else if (m.method === 'initialized') {
      send({ jsonrpc: '2.0', id: 900, method: 'window/workDoneProgress/create', params: { token: 't' } });
      send({ jsonrpc: '2.0', method: '$/progress', params: { token: 't', value: { kind: 'begin', title: 'Initializing JS/TS language features' } } });
      setTimeout(() => send({ jsonrpc: '2.0', method: '$/progress', params: { token: 't', value: { kind: 'end' } } }), 30);
    } else if (m.method === 'textDocument/didOpen') {
      send({ jsonrpc: '2.0', method: 'textDocument/publishDiagnostics', params: { uri: m.params.textDocument.uri, diagnostics: [
        { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 6 } }, severity: 1, code: 2304, source: 'ts', message: `real node saw ${process.argv.slice(2).join(' ')}` } ] } });
    } else if (m.method === 'shutdown') {
      send({ jsonrpc: '2.0', id: m.id, result: null });
    } else if (m.method === 'exit') { process.exit(0); }
  }
});
"#;

fn node_is_installed() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Start the plugin's server for real with `path` as its configured
/// executable, wait until it is ready, open the class, and return what it
/// published.
fn smoke(cx: &mut TestAppContext, path: PathBuf, project: &Project) -> String {
    let store = cx.new(|_| DiagnosticsStore::new());
    let settings = ServerSettings {
        path: Some(path),
        ..ServerSettings::default()
    };
    let server = server_for(cx, project, settings, store.clone());
    server.update(cx, |server, cx| server.start(cx));
    wait_for(cx, "the real server to be ready", |cx| {
        let status = server.read_with(cx, |server, _| server.status().clone());
        assert!(
            !matches!(status, ServerStatus::Error(_) | ServerStatus::Missing(_)),
            "the server did not start: {status:?}"
        );
        status == ServerStatus::Ready
    });
    let text = std::fs::read_to_string(&project.class).unwrap();
    server.read_with(cx, |server, _| {
        server
            .did_open_file(&project.class, &text, "typescript")
            .unwrap()
    });
    wait_for(cx, "the real server's diagnostics", |cx| {
        !store
            .read_with(cx, |store, _| store.for_file(&project.class))
            .is_empty()
    });
    let message = store.read_with(cx, |store, _| store.for_file(&project.class))[0]
        .1
        .message
        .clone();
    server.update(cx, |server, cx| {
        server.stop(pulsar_lsp::StopReason::Requested, cx)
    });
    message
}

/// One real spawn: Node runs the package's entry point directly (the entry
/// point is the configured path), with `--stdio`, over real pipes.
#[gpui::test]
fn smoke_a_real_node_process_runs_the_entry_point_directly(cx: &mut TestAppContext) {
    if !node_is_installed() {
        eprintln!("skipped: Node is not installed (this test never runs in CI without it)");
        return;
    }
    activate();
    let project = project(DOOR);
    let package = project
        .root
        .join("fake")
        .join("node_modules")
        .join("typescript-language-server");
    std::fs::create_dir_all(package.join("lib")).unwrap();
    std::fs::write(package.join("lib").join("cli.mjs"), FAKE_CLI).unwrap();
    let message = smoke(cx, package.join("lib").join("cli.mjs"), &project);
    assert_eq!(
        message, "real node saw --stdio",
        "launched as `node cli.mjs --stdio`"
    );
}

/// On Windows, a configured `.cmd` shim with no entry point beside it is the
/// fallback: it still runs (no console window is opened for it).
#[cfg(windows)]
#[gpui::test]
fn smoke_the_cmd_shim_fallback_runs_on_windows(cx: &mut TestAppContext) {
    if !node_is_installed() {
        eprintln!("skipped: Node is not installed (this test never runs in CI without it)");
        return;
    }
    activate();
    let project = project(DOOR);
    let bin = project.root.join("shimmed");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("server.mjs"), FAKE_CLI).unwrap();
    // npm's own shim shape: a batch file that runs node on the script.
    std::fs::write(
        bin.join("typescript-language-server.cmd"),
        "@ECHO off\r\nnode \"%~dp0\\server.mjs\" %*\r\n",
    )
    .unwrap();
    let message = smoke(cx, bin.join("typescript-language-server.cmd"), &project);
    assert_eq!(message, "real node saw --stdio");
}
