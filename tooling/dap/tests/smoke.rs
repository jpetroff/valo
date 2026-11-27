//! # Smoke test: a debug session as VS Code drives it (`tests/smoke.rs`)
//!
//! Starts the real `valo-dap` binary (Cargo builds it for integration tests
//! and puts its path in `CARGO_BIN_EXE_valo-dap`), then plays the client's
//! part: the same requests, in the same order, that VS Code sends when you
//! press F5. Each test checks the events that come back.
//!
//! To watch a real session's traffic instead, see "Seeing the messages" in
//! `tooling/dap/README.md`.
//!
//! **Textbook chapter:** `docs/13-debugger.md`.

use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};
use valo_dap::protocol::{read_message, write_message};

/// The client's end of a running adapter.
struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    seq: i64,
    /// Every message received, in order.
    log: Vec<Value>,
}

impl Client {
    fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_valo-dap"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start valo-dap");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client {
            child,
            stdin,
            stdout,
            seq: 0,
            log: Vec::new(),
        }
    }

    fn request(&mut self, command: &str, arguments: Value) {
        self.seq += 1;
        let msg = json!({ "seq": self.seq, "type": "request", "command": command, "arguments": arguments });
        write_message(&mut self.stdin, &msg).unwrap();
        self.stdin.flush().unwrap();
    }

    /// Read until a message matches `pred`; returns it.
    fn wait_for(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            let msg = read_message(&mut self.stdout).unwrap().unwrap_or_else(|| {
                panic!(
                    "stream closed while waiting for {what}; got {:#?}",
                    self.log
                )
            });
            self.log.push(msg.clone());
            if pred(&msg) {
                return msg;
            }
        }
    }

    fn response(&mut self, command: &str) -> Value {
        self.wait_for(command, |m| {
            m["type"] == "response" && m["command"] == command
        })
    }

    fn event(&mut self, event: &str) -> Value {
        self.wait_for(event, |m| m["type"] == "event" && m["event"] == event)
    }

    /// All `output` events received so far with this category, concatenated.
    fn output(&self, category: &str) -> String {
        self.log
            .iter()
            .filter(|m| m["event"] == "output" && m["body"]["category"] == category)
            .filter_map(|m| m["body"]["output"].as_str())
            .collect()
    }

    /// `initialize` + `launch`; returns the launch response. After a
    /// successful launch the adapter sends `initialized` (ready for
    /// breakpoints), as VS Code expects.
    fn launch(&mut self, program: &str) -> Value {
        self.launch_with(json!({ "program": program }))
    }

    fn launch_with(&mut self, args: Value) -> Value {
        self.request(
            "initialize",
            json!({ "adapterID": "valo", "linesStartAt1": true }),
        );
        let init = self.response("initialize");
        assert_eq!(init["success"], true);
        assert_eq!(init["body"]["supportsConfigurationDoneRequest"], true);
        self.request("launch", args);
        let launch = self.response("launch");
        if launch["success"] == true {
            self.event("initialized");
        }
        launch
    }

    /// `setBreakpoints` for `file`; returns the verified breakpoints.
    fn breakpoints(&mut self, file: &str, lines: &[u32]) -> Vec<Value> {
        let bps: Vec<Value> = lines.iter().map(|l| json!({ "line": l })).collect();
        self.request(
            "setBreakpoints",
            json!({ "source": { "path": file }, "breakpoints": bps }),
        );
        let resp = self.response("setBreakpoints");
        resp["body"]["breakpoints"].as_array().unwrap().clone()
    }

    /// Wait for `stopped`; check its reason; return the stack as
    /// `(name, line)` pairs, innermost first.
    fn stopped(&mut self, reason: &str) -> Vec<(String, u64)> {
        let ev = self.event("stopped");
        assert_eq!(ev["body"]["reason"], reason, "{ev:#}");
        assert_eq!(ev["body"]["threadId"], 1);
        self.request("stackTrace", json!({ "threadId": 1 }));
        let st = self.response("stackTrace");
        assert_eq!(st["success"], true, "{st:#}");
        st["body"]["stackFrames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["name"].as_str().unwrap().to_string(),
                    f["line"].as_u64().unwrap(),
                )
            })
            .collect()
    }

    /// `next`, `stepIn` or `stepOut`.
    fn step(&mut self, command: &str) {
        self.request(command, json!({ "threadId": 1 }));
        assert_eq!(self.response(command)["success"], true);
    }

    fn resume(&mut self) {
        self.request("continue", json!({ "threadId": 1 }));
        assert_eq!(self.response("continue")["success"], true);
    }

    fn disconnect(mut self) {
        self.request("disconnect", json!({}));
        self.response("disconnect");
        let status = self.child.wait().unwrap();
        assert!(status.success());
    }
}

/// Write a program into Cargo's per-test temporary directory.
fn program(name: &str, text: &str) -> String {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, text).unwrap();
    path.display().to_string()
}

#[test]
fn runs_a_program_and_shows_its_output() {
    let file = program(
        "dap_ok.vl",
        "fn sq(x: int) -> int = x * x\n\n7 |> sq |> print\nprint(true)\n",
    );
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);
    c.request("configurationDone", json!({}));
    c.response("configurationDone");
    let exited = c.event("exited");
    assert_eq!(exited["body"]["exitCode"], 0);
    c.event("terminated");
    assert_eq!(c.output("stdout"), "49\ntrue\n");

    c.request("threads", json!({}));
    assert_eq!(c.response("threads")["body"]["threads"][0]["name"], "main");
    c.disconnect();
}

#[test]
fn runtime_error_goes_to_stderr_with_exit_code_1() {
    let file = program("dap_err.vl", "print(1)\nprint(1 // 0)\n");
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);
    c.request("configurationDone", json!({}));
    assert_eq!(c.event("exited")["body"]["exitCode"], 1);
    c.event("terminated");
    assert_eq!(c.output("stdout"), "1\n");
    let err = c.output("stderr");
    assert!(err.contains("error[R"), "stderr was: {err}");
    assert!(err.contains("dap_err.vl:2:"), "stderr was: {err}");
    c.disconnect();
}

#[test]
fn compile_error_fails_the_launch() {
    let file = program("dap_bad.vl", "print(3 *)\n");
    let mut c = Client::start();
    let launch = c.launch(&file);
    assert_eq!(launch["success"], false);
    assert!(c.output("stderr").contains("[E0101]"), "{:#?}", c.log);
    c.disconnect();
}

#[test]
fn missing_program_and_unknown_requests_get_error_responses() {
    let mut c = Client::start();
    let launch = c.launch("/definitely/not/here.vl");
    assert_eq!(launch["success"], false);
    c.request("stepBack", json!({ "threadId": 1 }));
    assert_eq!(c.response("stepBack")["success"], false);
    c.disconnect();
}

/// `fn fact` with its body on lines 2–3 and the call on line 5.
const FACT: &str =
    "fn fact(n: int) -> int:\n    if n <= 1: 1\n    else: n * fact(n - 1)\n\nprint(fact(3))\n";

fn names(stack: &[(String, u64)]) -> Vec<(&str, u64)> {
    stack.iter().map(|(n, l)| (n.as_str(), *l)).collect()
}

#[test]
fn breakpoints_stop_and_show_the_call_stack() {
    let file = program("dap_fact.vl", FACT);
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);

    // Line 4 is blank: that breakpoint moves to line 5. Line 99 has no code.
    let bps = c.breakpoints(&file, &[2, 4, 99]);
    assert_eq!(bps[0]["verified"], true);
    assert_eq!(bps[0]["line"], 2);
    assert_eq!(bps[1]["verified"], true);
    assert_eq!(bps[1]["line"], 5);
    assert_eq!(bps[2]["verified"], false);
    // Another file is not part of the program.
    let other = c.breakpoints("/elsewhere/x.vl", &[1]);
    assert_eq!(other[0]["verified"], false);

    c.request("configurationDone", json!({}));
    let stack = c.stopped("breakpoint");
    assert_eq!(names(&stack), [("<main>", 5)]);
    let hit = &c
        .log
        .iter()
        .rev()
        .find(|m| m["event"] == "stopped")
        .unwrap()["body"]["hitBreakpointIds"];
    assert_eq!(hit, &json!([bps[1]["id"]]));

    c.resume();
    assert_eq!(
        names(&c.stopped("breakpoint")),
        [("fact", 2), ("<main>", 5)]
    );
    c.resume();
    assert_eq!(
        names(&c.stopped("breakpoint")),
        [("fact", 2), ("fact", 3), ("<main>", 5)]
    );

    // Paging: VS Code first asks for a few frames, then for the rest.
    c.request(
        "stackTrace",
        json!({ "threadId": 1, "startFrame": 1, "levels": 1 }),
    );
    let page = c.response("stackTrace");
    assert_eq!(page["body"]["totalFrames"], 3);
    assert_eq!(page["body"]["stackFrames"].as_array().unwrap().len(), 1);
    assert_eq!(page["body"]["stackFrames"][0]["line"], 3);
    assert_eq!(
        page["body"]["stackFrames"][0]["source"]["name"],
        "dap_fact.vl"
    );

    // Removing all breakpoints lets the program finish.
    assert!(c.breakpoints(&file, &[]).is_empty());
    c.resume();
    assert_eq!(c.event("exited")["body"]["exitCode"], 0);
    assert_eq!(c.output("stdout"), "6\n");

    // Not stopped any more: no stack to show.
    c.request("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(c.response("stackTrace")["success"], false);
    c.disconnect();
}

#[test]
fn tail_calls_show_as_a_label_frame() {
    let file = program(
        "dap_count.vl",
        "fn count(n: int) -> int:\n    if n == 0: 0\n    else: count(n - 1)\n\nprint(count(3))\n",
    );
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);
    c.breakpoints(&file, &[2]);
    c.request("configurationDone", json!({}));
    assert_eq!(
        names(&c.stopped("breakpoint")),
        [("count", 2), ("<main>", 5)]
    );
    c.resume();
    assert_eq!(
        names(&c.stopped("breakpoint")),
        [("count", 2), ("… 1 tail call elided …", 0), ("<main>", 5)]
    );
    // Stop button: the program ends without an error message.
    c.request("terminate", json!({}));
    c.response("terminate");
    assert_eq!(c.event("exited")["body"]["exitCode"], 130);
    c.event("terminated");
    assert_eq!(c.output("stderr"), "");
    c.disconnect();
}

#[test]
fn stop_on_entry_then_pause_a_long_run() {
    let file = program(
        "dap_spin.vl",
        "fn spin(n: int) -> int:\n    if n == 0: 0\n    else: spin(n - 1)\n\nprint(spin(100000000))\n",
    );
    let mut c = Client::start();
    let launch = c.launch_with(json!({ "program": file, "stopOnEntry": true }));
    assert_eq!(launch["success"], true);
    c.request("configurationDone", json!({}));
    // Line 1: `<main>` defines `spin` first.
    assert_eq!(names(&c.stopped("entry")), [("<main>", 1)]);
    c.resume();
    c.request("pause", json!({ "threadId": 1 }));
    c.response("pause");
    let stack = c.stopped("pause");
    assert_eq!(stack[0].0, "spin");
    c.request("terminate", json!({}));
    c.response("terminate");
    assert_eq!(c.event("exited")["body"]["exitCode"], 130);
    c.disconnect();
}

#[test]
fn run_without_debugging_ignores_breakpoints() {
    let file = program("dap_nodebug.vl", FACT);
    let mut c = Client::start();
    let launch = c.launch_with(json!({ "program": file, "noDebug": true }));
    assert_eq!(launch["success"], true);
    c.breakpoints(&file, &[2]);
    c.request("configurationDone", json!({}));
    assert_eq!(c.event("exited")["body"]["exitCode"], 0);
    assert_eq!(c.output("stdout"), "6\n");
    assert!(!c.log.iter().any(|m| m["event"] == "stopped"));
    c.disconnect();
}

/// `sq` with a two-line body, called from line 5.
const SQ: &str = "fn sq(x: int) -> int:\n    let y = x * x\n    y\n\nlet a = sq(3)\nprint(a)\n";

#[test]
fn step_over_in_and_out() {
    let file = program("dap_sq.vl", SQ);
    let mut c = Client::start();
    let launch = c.launch_with(json!({ "program": file, "stopOnEntry": true }));
    assert_eq!(launch["success"], true);
    // Line 7 is the implicit end of `<main>`: no breakpoint there.
    let bps = c.breakpoints(&file, &[7]);
    assert_eq!(bps[0]["verified"], false);
    c.request("configurationDone", json!({}));
    assert_eq!(names(&c.stopped("entry")), [("<main>", 1)]);

    c.step("next");
    assert_eq!(names(&c.stopped("step")), [("<main>", 5)]);
    c.step("stepIn");
    assert_eq!(names(&c.stopped("step")), [("sq", 2), ("<main>", 5)]);
    c.step("next");
    assert_eq!(names(&c.stopped("step")), [("sq", 3), ("<main>", 5)]);
    // Out of `sq`: the rest of line 5 (storing `a`) runs, then line 6.
    c.step("stepOut");
    assert_eq!(names(&c.stopped("step")), [("<main>", 6)]);
    // `print` is a built-in: nothing to step into. The end of `<main>` is
    // not a stop, so the program finishes.
    c.step("stepIn");
    assert_eq!(c.event("exited")["body"]["exitCode"], 0);
    assert_eq!(c.output("stdout"), "9\n");
    c.disconnect();
}

#[test]
fn a_breakpoint_inside_a_stepped_over_call_wins() {
    let file = program("dap_over_bp.vl", FACT);
    let mut c = Client::start();
    let launch = c.launch_with(json!({ "program": file, "stopOnEntry": true }));
    assert_eq!(launch["success"], true);
    c.breakpoints(&file, &[3]);
    c.request("configurationDone", json!({}));
    assert_eq!(names(&c.stopped("entry")), [("<main>", 1)]);
    c.step("next");
    assert_eq!(names(&c.stopped("step")), [("<main>", 5)]);
    // Stepping over `fact(3)` meets the breakpoint in the first call.
    c.step("next");
    assert_eq!(
        names(&c.stopped("breakpoint")),
        [("fact", 3), ("<main>", 5)]
    );
    // That stop ended the step; without breakpoints, stepping out of the
    // only `fact` frame on the stack runs to the end of the program.
    c.breakpoints(&file, &[]);
    c.step("stepOut");
    assert_eq!(c.event("exited")["body"]["exitCode"], 0);
    assert_eq!(c.output("stdout"), "6\n");
    c.disconnect();
}

#[test]
fn stepping_over_a_tail_call_lands_in_the_callee() {
    let file = program(
        "dap_tail_step.vl",
        "fn count(n: int) -> int:\n    if n == 0: 0\n    else: count(n - 1)\n\nprint(count(2))\n",
    );
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);
    c.breakpoints(&file, &[3]);
    c.request("configurationDone", json!({}));
    assert_eq!(
        names(&c.stopped("breakpoint")),
        [("count", 3), ("<main>", 5)]
    );
    c.breakpoints(&file, &[]);
    // The tail call replaced the frame: same depth, so `next` stops in it.
    c.step("next");
    assert_eq!(
        names(&c.stopped("step")),
        [("count", 2), ("… 1 tail call elided …", 0), ("<main>", 5)]
    );
    c.resume();
    assert_eq!(c.event("exited")["body"]["exitCode"], 0);
    c.disconnect();
}

/// A global, a defaulted parameter, and a shadowed `let`.
const VARS: &str = "let base = 10\nfn f(a: int, b: int = 2) -> int:\n    let x = a + b\n    let x = x * base\n    x\n\nprint(f(1))\n";

impl Client {
    /// `variables` of a reference as `(name, value, type)`.
    fn variables(&mut self, reference: i64) -> Vec<(String, String, String)> {
        self.request("variables", json!({ "variablesReference": reference }));
        let resp = self.response("variables");
        assert_eq!(resp["success"], true, "{resp:#}");
        resp["body"]["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                let s = |k: &str| v[k].as_str().unwrap().to_string();
                (s("name"), s("value"), s("type"))
            })
            .collect()
    }

    /// `evaluate` in frame `frame_id`; `Ok(result)` or `Err(message)`.
    fn evaluate(&mut self, expression: &str, frame_id: i64) -> Result<String, String> {
        self.request(
            "evaluate",
            json!({ "expression": expression, "frameId": frame_id, "context": "hover" }),
        );
        let resp = self.response("evaluate");
        if resp["success"] == true {
            Ok(resp["body"]["result"].as_str().unwrap().to_string())
        } else {
            Err(resp["message"].as_str().unwrap().to_string())
        }
    }
}

fn vars(list: &[(String, String, String)]) -> Vec<(&str, &str, &str)> {
    list.iter()
        .map(|(n, v, t)| (n.as_str(), v.as_str(), t.as_str()))
        .collect()
}

#[test]
fn variables_show_locals_and_globals() {
    let file = program("dap_vars.vl", VARS);
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);
    c.breakpoints(&file, &[5]);
    c.request("configurationDone", json!({}));
    assert_eq!(names(&c.stopped("breakpoint")), [("f", 5), ("<main>", 7)]);

    // Frame ids are 0 (f) and 2 (<main>); each has Locals and Globals.
    c.request("scopes", json!({ "frameId": 0 }));
    let scopes = c.response("scopes");
    let refs: Vec<(&str, i64)> = scopes["body"]["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap(),
                s["variablesReference"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(refs[0].0, "Locals");
    assert_eq!(refs[1].0, "Globals");

    // The second `x` shadows the first: only one `x` is shown.
    assert_eq!(
        vars(&c.variables(refs[0].1)),
        [("a", "1", "int"), ("b", "2", "int"), ("x", "30", "int")]
    );
    // Top-level bindings, without the built-ins.
    assert_eq!(
        vars(&c.variables(refs[1].1)),
        [("base", "10", "int"), ("f", "<fn f/2>", "function")]
    );
    // `<main>` has no locals (its `let`s are globals).
    c.request("scopes", json!({ "frameId": 2 }));
    let main_locals = c.response("scopes")["body"]["scopes"][0]["variablesReference"]
        .as_i64()
        .unwrap();
    assert!(c.variables(main_locals).is_empty());

    // Hover: names resolve in the frame, then in the globals.
    assert_eq!(c.evaluate("x", 0), Ok("30".to_string()));
    assert_eq!(c.evaluate("base", 0), Ok("10".to_string()));
    assert!(c.evaluate("a", 2).unwrap_err().contains("not defined"));
    assert!(c.evaluate("x + 1", 0).unwrap_err().contains("D5"));

    c.resume();
    assert_eq!(c.event("exited")["body"]["exitCode"], 0);
    assert_eq!(c.output("stdout"), "30\n");
    // Not stopped: nothing to show.
    c.request("variables", json!({ "variablesReference": 1 }));
    assert_eq!(c.response("variables")["success"], false);
    c.disconnect();
}

#[test]
fn locals_appear_as_their_lines_run() {
    let file = program("dap_vars_step.vl", VARS);
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);
    c.breakpoints(&file, &[7]);
    c.request("configurationDone", json!({}));
    c.stopped("breakpoint");
    // Into `f`: it stops in the prologue (the header line), before the
    // default of `b` is computed; `x` does not exist yet.
    c.step("stepIn");
    assert_eq!(names(&c.stopped("step")), [("f", 2), ("<main>", 7)]);
    assert_eq!(
        vars(&c.variables(2)),
        [
            ("a", "1", "int"),
            ("b", "(default not computed yet)", "<missing>")
        ]
    );
    c.step("next");
    assert_eq!(names(&c.stopped("step")), [("f", 3), ("<main>", 7)]);
    assert_eq!(vars(&c.variables(2)).len(), 2);
    c.step("next");
    c.stopped("step");
    assert_eq!(vars(&c.variables(2))[2], ("x", "3", "int"));
    c.step("next");
    c.stopped("step");
    assert_eq!(vars(&c.variables(2))[2], ("x", "30", "int"));
    c.resume();
    assert_eq!(c.event("exited")["body"]["exitCode"], 0);
    c.disconnect();
}

impl Client {
    /// `setExceptionBreakpoints` with the given filters.
    fn exception_filters(&mut self, filters: &[&str]) {
        self.request("setExceptionBreakpoints", json!({ "filters": filters }));
        assert_eq!(self.response("setExceptionBreakpoints")["success"], true);
    }
}

const DIV: &str = "fn div(a: int, b: int) -> int:\n    let q = a // b\n    q\n\nprint(div(7, 0))\n";

#[test]
fn stops_at_a_runtime_error_with_frames_and_variables_intact() {
    let file = program("dap_div.vl", DIV);
    let mut c = Client::start();
    let init = {
        c.request("initialize", json!({ "adapterID": "valo" }));
        c.response("initialize")
    };
    let filters: Vec<&str> = init["body"]["exceptionBreakpointFilters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["filter"].as_str().unwrap())
        .collect();
    assert_eq!(filters, ["runtime", "assert"]);
    c.request("launch", json!({ "program": file }));
    assert_eq!(c.response("launch")["success"], true);
    c.event("initialized");
    c.exception_filters(&["runtime"]);
    c.request("configurationDone", json!({}));

    // Stopped *on* the failing line, inside `div`, before unwinding.
    let ev = c.wait_for("stopped", |m| m["event"] == "stopped");
    assert_eq!(ev["body"]["reason"], "exception");
    assert!(
        ev["body"]["text"]
            .as_str()
            .unwrap()
            .contains("division by zero")
    );
    c.request("stackTrace", json!({ "threadId": 1 }));
    let st = c.response("stackTrace");
    let frames = st["body"]["stackFrames"].as_array().unwrap();
    assert_eq!(
        (frames[0]["name"].as_str(), frames[0]["line"].as_u64()),
        (Some("div"), Some(2))
    );
    assert_eq!(frames[1]["line"], 5);
    // `q` is not set yet: its `let` is the line that failed.
    assert_eq!(
        vars(&c.variables(2)),
        [("a", "7", "int"), ("b", "0", "int")]
    );

    c.request("exceptionInfo", json!({ "threadId": 1 }));
    let info = c.response("exceptionInfo");
    assert_eq!(info["body"]["exceptionId"], "R0102");
    assert_eq!(info["body"]["breakMode"], "unhandled");
    assert!(
        info["body"]["description"]
            .as_str()
            .unwrap()
            .contains("division by zero")
    );

    // Continuing lets the error end the program, as `valo run` would.
    c.resume();
    assert_eq!(c.event("exited")["body"]["exitCode"], 1);
    assert!(c.output("stderr").contains("error[R0102]"));
    c.request("exceptionInfo", json!({ "threadId": 1 }));
    assert_eq!(c.response("exceptionInfo")["success"], false);
    c.disconnect();
}

#[test]
fn failed_asserts_have_their_own_filter() {
    let src = "let a = 2\nassert a + a == 5\n";
    // Only "runtime" ticked: the failed assert does not stop.
    let file = program("dap_assert_off.vl", src);
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);
    c.exception_filters(&["runtime"]);
    c.request("configurationDone", json!({}));
    assert_eq!(c.event("exited")["body"]["exitCode"], 1);
    assert!(!c.log.iter().any(|m| m["event"] == "stopped"));
    c.disconnect();

    // "assert" ticked: it stops, quoting the assertion and both sides.
    let file = program("dap_assert_on.vl", src);
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);
    c.exception_filters(&["assert"]);
    c.request("configurationDone", json!({}));
    assert_eq!(names(&c.stopped("exception")), [("<main>", 2)]);
    c.request("exceptionInfo", json!({ "threadId": 1 }));
    let info = c.response("exceptionInfo");
    assert_eq!(info["body"]["exceptionId"], "R0701");
    assert_eq!(
        info["body"]["description"],
        "assertion failed: a + a == 5\nleft: 4\nright: 5"
    );
    // Stop button at an error: the run ends, the error is still reported.
    c.request("terminate", json!({}));
    c.response("terminate");
    assert_eq!(c.event("exited")["body"]["exitCode"], 1);
    c.disconnect();
}

#[test]
fn stack_overflow_stops_and_pages_its_frames() {
    let file = program(
        "dap_overflow.vl",
        "fn f(n: int) -> int = 1 + f(n - 1)\n\nprint(f(0))\n",
    );
    let mut c = Client::start();
    assert_eq!(c.launch(&file)["success"], true);
    c.exception_filters(&["runtime"]);
    c.request("configurationDone", json!({}));
    let ev = c.wait_for("stopped", |m| m["event"] == "stopped");
    assert_eq!(ev["body"]["reason"], "exception");
    // VS Code asks for the first 20 frames, then more on demand.
    c.request(
        "stackTrace",
        json!({ "threadId": 1, "startFrame": 0, "levels": 20 }),
    );
    let st = c.response("stackTrace");
    assert_eq!(st["body"]["stackFrames"].as_array().unwrap().len(), 20);
    assert!(st["body"]["totalFrames"].as_u64().unwrap() > 100_000);
    c.request("exceptionInfo", json!({ "threadId": 1 }));
    assert_eq!(c.response("exceptionInfo")["body"]["exceptionId"], "R0301");
    c.resume();
    assert_eq!(c.event("exited")["body"]["exitCode"], 1);
    c.disconnect();
}
