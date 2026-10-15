@edition("2026");

// DHE Showcase 唯一入口：人工构建确认、证据校验和双次 Player 验证。
import "dhe_common.tl" as common;
import "std/agent.tl" as agent_api;
import "std/crypto.tl" as crypto;
import "std/fs.tl" as fs;
import "std/json.tl" as json;
import "std/process.tl" as process;
import "std/system.tl" as system;

static baseline_template = "#66";
static hotfix_template = "#67";
static max_same_failure = 3;
static observation_ms = 30000;

class RestoreState {
    pub found: bool,
    pub run_id: string,
    pub phase: string,
    pub sequence: int,
    pub pending_effect: string,
    pub last_code: string,
    pub same_failure_count: int,
    pub baseline_build: string,
    pub hotfix_build: string,
    pub baseline_version: string,
}

fn default_options() -> common::Options {
    return common::Options {
        mode: "release",
        run_root: "",
        plugin_root: common::default_plugin_root,
        libil2cpp_root: common::default_libil2cpp_root,
        baseline_root: common::default_baseline_root,
        hotfix_root: common::default_hotfix_root,
        demo_root: "",
        player_path: "",
        player_log: "",
        build_inputs_root: "",
        success_marker: common::default_success_marker,
        dhe_load_min: common::default_dhe_load_min,
        fatal_patterns: common::fatal_patterns(),
        demo_skip_build: false,
    };
}

fn require_value(arguments: string[], index: int, option: string) -> string | Error {
    if index >= arguments.len() {
        return Error::make("usage", "Option " + option + " requires a value", ErrorOrigin.user, false);
    }
    return arguments[index];
}

fn parse_options() -> common::Options | Error {
    let options = default_options();
    let arguments = argv();
    let index = 0;
    loop {
        if index >= arguments.len() {
            break;
        }
        let argument = arguments[index];
        if argument == "--help" || argument == "-h" {
            options.mode = "help";
        } else if argument == "release" || argument == "inspect" || argument == "demo" || argument == "player" || argument == "all" || argument == "resume" {
            options.mode = argument;
        } else if argument == "--run-root" {
            index += 1;
            options.run_root = require_value(arguments, index, argument) ?;
        } else if argument == "--plugin-root" {
            index += 1;
            options.plugin_root = require_value(arguments, index, argument) ?;
        } else if argument == "--libil2cpp-root" {
            index += 1;
            options.libil2cpp_root = require_value(arguments, index, argument) ?;
        } else if argument == "--baseline-root" {
            index += 1;
            options.baseline_root = require_value(arguments, index, argument) ?;
        } else if argument == "--hotfix-root" {
            index += 1;
            options.hotfix_root = require_value(arguments, index, argument) ?;
        } else if argument == "--demo-root" {
            index += 1;
            options.demo_root = require_value(arguments, index, argument) ?;
        } else if argument == "--player" {
            index += 1;
            options.player_path = require_value(arguments, index, argument) ?;
        } else if argument == "--log" {
            index += 1;
            options.player_log = require_value(arguments, index, argument) ?;
        } else if argument == "--build-inputs-root" {
            index += 1;
            options.build_inputs_root = require_value(arguments, index, argument) ?;
        } else if argument == "--success-marker" {
            index += 1;
            options.success_marker = require_value(arguments, index, argument) ?;
        } else if argument == "--dhe-load-min" {
            index += 1;
            options.dhe_load_min = int(require_value(arguments, index, argument) ?);
        } else if argument == "--fatal" {
            index += 1;
            options.fatal_patterns.push(require_value(arguments, index, argument) ?.lowercase());
        } else if argument == "--demo-skip-build" {
            options.demo_skip_build = true;
        } else {
            return Error::make("usage", "Unknown option: " + argument, ErrorOrigin.user, false);
        }
        index += 1;
    }
    if options.mode == "help" {
        return options;
    }
    if options.run_root == "" {
        return Error::make("runtime", "--run-root is required; it is the version evidence directory", ErrorOrigin.user, false);
    }
    if options.mode == "release" || options.mode == "resume" || options.mode == "player" || options.mode == "all" {
        if options.player_path == "" || options.player_log == "" {
            return Error::make("runtime", "--player and --log are required for Player modes", ErrorOrigin.user, false);
        }
    }
    return options;
}

fn usage() -> string {
    return "Usage: teal run --tier agentic examples/showcases/dhe/dhe.tl -- [release|inspect|demo|player|all|resume] --run-root PATH [options]\n" + "Options: --player PATH --log PATH --run-root PATH --plugin-root PATH --libil2cpp-root PATH --baseline-root PATH --hotfix-root PATH --demo-root PATH --build-inputs-root PATH --success-marker TEXT --dhe-load-min N --fatal TEXT\n" + "Release commands: begin, baseline-success|BUILD|COMMIT, hotfix-success|BUILD|BASE_VERSION|COMMIT, artifacts-ready|ARCHIVE|PLAYER_DIR|SOURCE, verify-player, failure-artifacts|SYMBOLS|INPUTS, status, reconcile|EFFECT|RESULT, review, failure|CODE|DETAIL, quit";
}

fn build_id(value: string) -> bool {
    if value == "" {
        return false;
    }
    for character in value {
        if !"0123456789".contains(character) {
            return false;
        }
    }
    return true;
}

fn event_field(parts: string[], key: string) -> string {
    let prefix = key + "=";
    for part in parts {
        if part.starts_with(prefix) {
            return part[prefix.len()..].replace("%0A", "\n").replace("%0D", "\r").replace("%7C", "|").replace("%25", "%");
        }
    }
    return "";
}

task restore_report(report: string) -> RestoreState | Error {
    let restored = RestoreState { found: false, run_id: "", phase: "idle", sequence: 0, pending_effect: "", last_code: "", same_failure_count: 0, baseline_build: "", hotfix_build: "", baseline_version: "" };
    if !(fs::exists(report).await ?) {
        return restored;
    }
    for line in fs::read(report).await ?.split("\n") {
        if !line.starts_with("DHE-EVENT|") {
            continue;
        }
        let parts = line.split("|");
        let schema = event_field(parts, "schema");
        if schema != "1" {
            return Error::make("runtime", "Unsupported DHE evidence schema", ErrorOrigin.user, false);
        }
        let event_run = event_field(parts, "run");
        let seq = event_field(parts, "seq");
        if seq == "" {
            return Error::make("runtime", "DHE evidence event has no sequence", ErrorOrigin.user, false);
        }
        let event_sequence = int(seq);
        if restored.found && event_run != restored.run_id {
            return Error::make("runtime", "DHE evidence contains multiple run ids", ErrorOrigin.user, false);
        }
        if restored.found && event_sequence != restored.sequence + 1 {
            return Error::make("runtime", "DHE evidence sequence is not contiguous", ErrorOrigin.user, false);
        }
        restored.found = true;
        restored.run_id = event_run;
        restored.sequence = event_sequence;
        restored.phase = event_field(parts, "phase");
        let kind = event_field(parts, "kind");
        let effect = event_field(parts, "effect");
        let code = event_field(parts, "code");
        let detail = event_field(parts, "detail");
        if kind == "intent" && restored.pending_effect != "" {
            return Error::make("runtime", "DHE evidence contains a second effect intent before the first result", ErrorOrigin.user, false);
        }
        if kind == "intent" {
            restored.pending_effect = effect;
        }
        if kind == "result" && effect == restored.pending_effect {
            restored.pending_effect = "";
        }
        if kind == "failure" {
            if code == restored.last_code {
                restored.same_failure_count += 1;
            } else {
                restored.last_code = code;
                restored.same_failure_count = 1;
            }
        }
        if kind == "baseline" {
            restored.baseline_build = detail;
            restored.baseline_version = detail + ".0.0." + detail;
        }
        if kind == "hotfix" {
            restored.hotfix_build = detail;
        }
    }
    if restored.pending_effect != "" {
        restored.phase = "needs_attention";
    }
    return restored;
}

class DheRun {
    phase: string,
    config: common::Options | none,
    report: string,
    run_id: string,
    sequence: int,
    attempt: int,
    last_failure_code: string,
    same_failure_count: int,
    baseline_head: string,
    hotfix_head: string,
    baseline_build: string,
    hotfix_build: string,
    baseline_version: string,
    baseline_id: string,
    pending_effect: string,
}

impl DheRun {
    task event(kind: string, effect: string, code: string, detail: string) -> none | Error {
        self.sequence += 1;
        common::append_event(self.report, self.sequence, self.run_id, self.phase, kind, effect, code, detail).await ?;
    }

    task fail(code: string, detail: string) -> string | Error {
        if code == self.last_failure_code {
            self.same_failure_count += 1;
        } else {
            self.last_failure_code = code;
            self.same_failure_count = 1;
        }
        self.event("failure", "", code, detail).await ?;
        if self.same_failure_count >= max_same_failure {
            self.phase = "stopped";
            self.event("transition", "", "stopped", "same failure code reached three attempts").await ?;
            return "Stopped after three identical failures: " + code;
        }
        return "Failure recorded (" + string(self.same_failure_count) + "/3): " + code + "|" + detail;
    }

    task require_phase(expected: string) -> string {
        if self.phase == "needs_attention" {
            return "Run needs explicit reconcile for effect " + self.pending_effect;
        }
        if self.phase == "stopped" {
            return "Run is stopped; begin a new run root";
        }
        if self.phase != expected {
            return "Command is not expected in phase " + self.phase;
        }
        return "";
    }

    task initialize(options: common::Options) -> string | Error {
        self.config = options;
        self.run_id = "run-" + system::utc_now();
        self.report = common::start_report(options.run_root, self.run_id).await ?;
        let report_text = fs::read(self.report).await ?;
        if !(report_text.contains("Schema: 1")) && report_text != "" {
            return Error::make("runtime", "Existing DHE report has no supported schema; choose a new run root", ErrorOrigin.user, false);
        }
        let restored = restore_report(self.report).await ?;
        if restored.found {
            self.run_id = restored.run_id;
            self.phase = restored.phase;
            self.sequence = restored.sequence;
            self.pending_effect = restored.pending_effect;
            self.last_failure_code = restored.last_code;
            self.same_failure_count = restored.same_failure_count;
            self.baseline_build = restored.baseline_build;
            self.hotfix_build = restored.hotfix_build;
            self.baseline_version = restored.baseline_version;
            return "Restored run " + self.run_id + " at phase " + self.phase;
        }
        self.phase = "idle";
        self.sequence = 0;
        self.attempt = 0;
        self.last_failure_code = "";
        self.same_failure_count = 0;
        self.baseline_head = "";
        self.hotfix_head = "";
        self.baseline_build = "";
        self.hotfix_build = "";
        self.baseline_version = "";
        self.baseline_id = "";
        self.pending_effect = "";
        return "Initialized run " + self.run_id;
    }

    task begin() -> string | Error {
        let phase_error = self.require_phase("idle").await;
        if phase_error != "" {
            return phase_error;
        }
        let options = self.config;
        if options == none {
            return "Run has not been configured";
        }
        let source_specs = [["plugin", options.plugin_root], ["libil2cpp", options.libil2cpp_root], ["baseline", options.baseline_root], ["hotfix", options.hotfix_root]];
        let plugin_head = "";
        let libil2cpp_head = "";
        for spec in source_specs {
            let state = common::source_state(spec[0], spec[1]).await ?;
            self.event("source", "", if common::source_ready(state) { "ok" } else { "source_mismatch" }, "role=" + state.role + ";path=" + state.path + ";head=" + state.head + ";upstream=" + state.upstream + ";branch=" + state.branch + ";clean=" + string(state.clean)).await ?;
            if !common::source_ready(state) {
                self.phase = "stopped";
                return "Source identity is not clean and synchronized: " + state.role;
            }
            if spec[0] == "plugin" {
                plugin_head = state.head;
            }
            if spec[0] == "libil2cpp" {
                libil2cpp_head = state.head;
            }
        }
        let baseline_revision = common::read_revision(options.baseline_root).await ?;
        let hotfix_revision = common::read_revision(options.hotfix_root).await ?;
        let revision_ok = baseline_revision.contains("commit=" + plugin_head) && baseline_revision.contains("commit=" + libil2cpp_head) && hotfix_revision.contains("commit=" + plugin_head) && hotfix_revision.contains("commit=" + libil2cpp_head);
        self.event("source", "", if revision_ok { "revision_ok" } else { "revision_mismatch" }, "baseline_revision=" + baseline_revision + ";hotfix_revision=" + hotfix_revision).await ?;
        if !revision_ok {
            self.phase = "stopped";
            return "Project revision.txt does not match the canonical HybridCLR source HEAD";
        }
        self.baseline_head = common::require_command("baseline HEAD", common::git(options.baseline_root, ["rev-parse", "HEAD"]).await ?) ?;
        self.hotfix_head = common::require_command("hotfix HEAD", common::git(options.hotfix_root, ["rev-parse", "HEAD"]).await ?) ?;
        self.phase = "baseline-build";
        self.event("transition", "", "sources", "Replay #66, then send baseline-success").await ?;
        return "Sources verified. Replay " + baseline_template + "; send baseline-success|BUILD|COMMIT";
    }

    task baseline_success(fields: string[]) -> string | Error {
        let phase_error = self.require_phase("baseline-build").await;
        if phase_error != "" {
            return phase_error;
        }
        if fields.len() != 3 || !build_id(fields[1]) {
            return "Usage: baseline-success|BUILD|COMMIT";
        }
        if fields[2] != self.baseline_head {
            return self.fail("baseline_commit_mismatch", "Jenkins commit does not match baseline HEAD").await ?;
        }
        self.baseline_build = fields[1];
        self.baseline_version = fields[1] + ".0.0." + fields[1];
        self.phase = "hotfix-build";
        self.event("baseline", "", "success", self.baseline_build).await ?;
        return "Baseline accepted as " + self.baseline_version + ". Rebuild " + hotfix_template + " with DHE_CUSTOM_BASE_VERSION=" + self.baseline_version + "; send hotfix-success|BUILD|BASE_VERSION|COMMIT";
    }

    task hotfix_success(fields: string[]) -> string | Error {
        let phase_error = self.require_phase("hotfix-build").await;
        if phase_error != "" {
            return phase_error;
        }
        if fields.len() != 4 || !build_id(fields[1]) {
            return "Usage: hotfix-success|BUILD|BASE_VERSION|COMMIT";
        }
        if fields[2] != self.baseline_version {
            return self.fail("hotfix_base_version_mismatch", "DHE_CUSTOM_BASE_VERSION does not match the accepted baseline").await ?;
        }
        if fields[3] != self.hotfix_head {
            return self.fail("hotfix_commit_mismatch", "Jenkins commit does not match hotfix HEAD").await ?;
        }
        self.hotfix_build = fields[1];
        self.phase = "artifacts";
        self.event("hotfix", "", "success", self.hotfix_build).await ?;
        return "Builds accepted. Put the archive and extracted Player below run_root, then send artifacts-ready|ARCHIVE|PLAYER_DIR|SOURCE";
    }

    task check_player_identity(player_dir: string) -> none | Error {
        let version_path = common::join_path(player_dir, "tspc_Data/StreamingAssets/GameAssets/version.json");
        if !(fs::exists(version_path).await ?) {
            return Error::make("filesystem", "Player version.json is missing: " + version_path, ErrorOrigin. host, false);
        }
        let document = fs::read(version_path).await ?;
        let baseline_id = json::read_string(document, "/baselineId");
        if baseline_id == "" {
            return Error::make("runtime", "Player version.json has no baselineId", ErrorOrigin.user, false);
        }
        self.baseline_id = baseline_id;
        let version = if json::has(document, "/version") { json::read_string(document, "/version") } else { "" };
        self.event("evidence", "", "player_identity", "version=" + version + ";baselineId=" + baseline_id + ";path=" + version_path).await ?;
    }

    task check_build_inputs(options: common::Options) -> none | Error {
        if options.build_inputs_root == "" {
            return;
        }
        let manifest_path = common::join_path(options.build_inputs_root, "baselines/" + self.baseline_version + "/DheBaseline/StandaloneWindows64/manifest.json");
        let update_root = common::join_path(options.build_inputs_root, "update/AOTDllOutput/StandaloneWindows64");
        if !(fs::exists(manifest_path).await ?) {
            return Error::make("filesystem", "SnapshotManifest is missing from build inputs: " + manifest_path, ErrorOrigin. host, false);
        }
        if !(fs::exists(update_root).await ?) {
            return Error::make("filesystem", "Update AOT input directory is missing: " + update_root, ErrorOrigin. host, false);
        }
        let manifest = fs::read(manifest_path).await ?;
        let format = json::read_number(manifest, "/formatVersion");
        let baseline_id = json::read_string(manifest, "/baselineId");
        let names = json::items(manifest, "/differentialHybridAssemblyNames");
        if format != "1" || baseline_id == "" || names.len() == 0 {
            return Error::make("runtime", "SnapshotManifest has an invalid format, baselineId, or assembly set", ErrorOrigin.user, false);
        }
        if self.baseline_id != "" && self.baseline_id != baseline_id {
            return Error::make("runtime", "Player version.json baselineId differs from SnapshotManifest baselineId", ErrorOrigin.user, false);
        }
        self.event("evidence", "", "build_inputs", "manifest=" + manifest_path + ";formatVersion=" + format + ";baselineId=" + baseline_id + ";assembly_count=" + string(names.len()) + ";update_root=" + update_root).await ?;
    }

    task artifacts_ready(fields: string[]) -> string | Error {
        let phase_error = self.require_phase("artifacts").await;
        if phase_error != "" {
            return phase_error;
        }
        if fields.len() != 4 {
            return "Usage: artifacts-ready|ARCHIVE|PLAYER_DIR|SOURCE";
        }
        let options = self.config;
        if options == none {
            return "Run has not been configured";
        }
        let archive = fields[1];
        let player_dir = fields[2];
        let expected = self.baseline_version + ".ts_pc_global.7z";
        if common::file_name(archive) != expected {
            return self.fail("archive_name_mismatch", "Expected " + expected).await ?;
        }
        let canonical_archive = common::require_inside(options.run_root, archive, "baseline archive").await ?;
        let canonical_player_dir = common::require_inside(options.run_root, player_dir, "Player directory").await ?;
        let player = common::canonical_existing(options.player_path, "Player executable").await ?;
        if common::parent_path(player) != canonical_player_dir {
            return self.fail("player_path_mismatch", "Player must be directly below the extracted directory").await ?;
        }
        self.check_player_identity(canonical_player_dir).await ?;
        self.check_build_inputs(options).await ?;
        let hash = crypto::sha256_file(canonical_archive).await;
        self.phase = "player-first";
        self.event("evidence", "", "artifact", "archive=" + canonical_archive + ";sha256=" + hash.sha256 + ";size=" + hash.size + ";source=" + fields[3]).await ?;
        return "Artifacts accepted. send verify-player and confirm first-entered|<effect-id>, second-entered|<effect-id>";
    }

    task failure_artifacts(fields: string[]) -> string | Error {
        if self.report == "" {
            return "Run has not been initialized";
        }
        if fields.len() != 3 {
            return "Usage: failure-artifacts|SYMBOLS|BUILD_INPUTS";
        }
        let options = self.config;
        if options == none {
            return "Run has not been configured";
        }
        let symbols = common::require_inside(options.run_root, fields[1], "baseline symbols").await ?;
        let inputs = common::require_inside(options.run_root, fields[2], "hotfix inputs").await ?;
        let expected_symbols = self.baseline_version + ".dhe-baseline-symbols.zip";
        let expected_inputs = self.hotfix_build + ".dhe-build-inputs.zip";
        if common::file_name(symbols) != expected_symbols || common::file_name(inputs) != expected_inputs {
            return self.fail("failure_artifact_name_mismatch", "Failure artifacts do not match this run").await ?;
        }
        let symbol_hash = crypto::sha256_file(symbols).await;
        let input_hash = crypto::sha256_file(inputs).await;
        self.event("evidence", "", "failure_artifacts", "symbols=" + symbols + ";symbols_sha256=" + symbol_hash.sha256 + ";inputs=" + inputs + ";inputs_sha256=" + input_hash.sha256).await ?;
        return "Failure artifacts recorded";
    }

    task confirm(expected: string, effect: string) -> bool | Error {
        agent_api::emit("status", "When the game screen is loaded, send " + expected + "|" + effect);
        let answer = agent_api::receive().await ?;
        let value = "";
        match answer {
            agent_api::Input.text { value: text_value } => {
                value = text_value;
            }
            agent_api::Input.command { value: command_value } => {
                value = command_value;
            } _ => {}
        }
        return value == expected + "|" + effect;
    }

    task launch(label: string, effect: string, options: common::Options, before: string) -> common::LogCheck | Error {
        let executable = fs::canonicalize(options.player_path).await ?;
        if common::process_count(executable).await ? != 0 {
            return Error::make("runtime", "A Player with this executable is already running", ErrorOrigin.user, false);
        }
        self.pending_effect = effect;
        self.event("intent", effect, "player_launch", "label=" + label + ";path=" + executable + ";args=--index=0,--gameid=8612").await ?;
        let handle = common::spawn_command(options.player_path, ["--index=0", "--gameid=8612"], common::parent_path(options.player_path)).await ?;
        if !(self.confirm(label + "-entered", effect).await ?) {
            _ = common::terminate_command(handle).await ?;
            return Error::make("example_failure", label + " game screen was not confirmed for effect " + effect, ErrorOrigin.user, false);
        }
        let waited = common::wait_command(handle, observation_ms).await ?;
        if waited.state != process::ProcessState.running {
            return Error::make("example_failure", label + " Player exited before the 30 second observation", ErrorOrigin.user, false);
        }
        let terminated = common::terminate_command(handle).await ?;
        let lingering = common::process_count(executable).await ?;
        if lingering != 0 {
            return Error::make("example_failure", label + " Player did not exit after terminate: " + string(lingering), ErrorOrigin.user, false);
        }
        if !(fs::exists(options.player_log).await ?) {
            return Error::make("filesystem", "Player.log was not produced by " + label + " launch", ErrorOrigin. host, false);
        }
        let source_metadata = fs::metadata(options.player_log).await ?;
        let after = source_metadata.modified_unix_ms + ":" + source_metadata.size;
        if before != "" && before == after {
            return Error::make("example_failure", label + " launch did not produce a fresh Player.log", ErrorOrigin.user, false);
        }
        let destination = common::join_path(options.run_root, "Player." + string(self.attempt) + "." + label + ".log");
        let copied = fs::copy(options.player_log, destination, true).await ?;
        if copied == "" {
            return Error::make("runtime", "Player log copy was not acknowledged", ErrorOrigin.user, false);
        }
        let check = common::scan_log(destination, "", options.success_marker, options.fatal_patterns).await ?;
        check.fresh = before == "" || before != after;
        self.pending_effect = "";
        self.event("result", effect, "player_result", "pid=" + string(handle.pid()) + ";wait=" + common::process_state(waited) + ";terminate=" + common::process_state(terminated) + ";log=" + destination + ";sha256=" + check.sha256 + ";fresh=" + string(check.fresh) + ";fatal=" + string(check.fatal_count) + ";dhe_load=" + string(check.dhe_load_count) + ";success=" + string(check.success_count)).await ?;
        return check;
    }

    task verify_player() -> string | Error {
        let phase_error = self.require_phase("player-first").await;
        if phase_error != "" {
            return phase_error;
        }
        let options = self.config;
        if options == none {
            return "Run has not been configured";
        }
        self.attempt += 1;
        let before = "";
        if fs::exists(options.player_log).await ? {
            let entry = fs::metadata(options.player_log).await ?;
            before = entry.modified_unix_ms + ":" + entry.size;
        }
        let first_effect = self.run_id + ":player:" + string(self.attempt) + ":first";
        let first = self.launch("first", first_effect, options, before).await ?;
        if first.fatal_count != 0 {
            return self.fail("first_player_fatal", "First log contains fatal patterns").await ?;
        }
        let before_second_entry = fs::metadata(options.player_log).await ?;
        let before_second = before_second_entry.modified_unix_ms + ":" + before_second_entry.size;
        let second_effect = self.run_id + ":player:" + string(self.attempt) + ":second";
        let second = self.launch("second", second_effect, options, before_second).await ?;
        if second.fatal_count != 0 {
            return self.fail("second_player_fatal", "Second log contains fatal patterns").await ?;
        }
        if second.dhe_load_count < options.dhe_load_min {
            return self.fail("dhe_load_missing", "Second log has only " + string(second.dhe_load_count) + " DHE load markers").await ?;
        }
        if options.success_marker != "" && second.success_count == 0 {
            return self.fail("business_marker_missing", "Second log lacks the configured business marker").await ?;
        }
        self.phase = "verified";
        self.event("transition", "", "verified", "two Player launches passed deterministic log gates").await ?;
        return "Player verification succeeded";
    }

    task inspect() -> string | Error {
        let options = self.config;
        if options == none {
            return "Run has not been configured";
        }
        if self.report == "" {
            self.report = common::start_report(options.run_root, self.run_id).await ?;
        }
        let source_specs = [["plugin", options.plugin_root], ["libil2cpp", options.libil2cpp_root], ["baseline", options.baseline_root], ["hotfix", options.hotfix_root]];
        let ok = true;
        for spec in source_specs {
            let state = common::source_state(spec[0], spec[1]).await ?;
            self.event("source", "", if common::source_ready(state) { "ok" } else { "source_mismatch" }, "role=" + state.role + ";path=" + state.path + ";head=" + state.head + ";upstream=" + state.upstream + ";branch=" + state.branch + ";clean=" + string(state.clean)).await ?;
            if !common::source_ready(state) {
                ok = false;
            }
        }
        if options.player_log != "" && fs::exists(options.player_log).await ? {
            let check = common::scan_log(options.player_log, "", options.success_marker, options.fatal_patterns).await ?;
            self.event("evidence", "", "existing_log", "path=" + check.path + ";sha256=" + check.sha256 + ";fatal=" + string(check.fatal_count) + ";dhe_load=" + string(check.dhe_load_count) + ";success=" + string(check.success_count)).await ?;
        }
        return if ok { "Inspection completed" } else { "Inspection found source mismatches; see evidence" };
    }

    task review() -> string | Error {
        if self.report == "" || !(fs::exists(self.report).await ?) {
            return "No evidence report is available";
        }
        return "Review is read-only and external; inspect DHE-EVENT lines and the recorded hashes before accepting a release";
    }

    task handle(input: string) -> string | Error {
        let fields = input.split("|");
        let command = fields[0];
        if command == "status" {
            return self.phase + "|run=" + self.run_id + "|seq=" + string(self.sequence) + "|pending=" + self.pending_effect + "|attempt=" + string(self.attempt);
        }
        if command == "begin" {
            return self.begin().await ?;
        }
        if command == "baseline-success" {
            return self.baseline_success(fields).await ?;
        }
        if command == "hotfix-success" {
            return self.hotfix_success(fields).await ?;
        }
        if command == "artifacts-ready" {
            return self.artifacts_ready(fields).await ?;
        }
        if command == "verify-player" {
            return self.verify_player().await ?;
        }
        if command == "failure-artifacts" {
            return self.failure_artifacts(fields).await ?;
        }
        if command == "reconcile" && fields.len() == 3 && fields[1] == self.pending_effect {
            self.pending_effect = "";
            self.phase = "player-first";
            self.event("result", fields[1], "reconciled", fields[2]).await ?;
            return "Effect reconciled; retry the current gate explicitly";
        }
        if command == "review" {
            return self.review().await ?;
        }
        if command == "failure" && fields.len() >= 3 {
            return self.fail(fields[1], fields[2]).await ?;
        }
        return "Unknown command. " + usage();
    }
}

task run_agent(options: common::Options) -> int | Error {
    let worker = DheRun { phase: "idle", config: options, report: "", run_id: "", sequence: 0, attempt: 0, last_failure_code: "", same_failure_count: 0, baseline_head: "", hotfix_head: "", baseline_build: "", hotfix_build: "", baseline_version: "", baseline_id: "", pending_effect: "" };
    worker.initialize(options).await ?;
    loop {
        let input = agent_api::receive().await ?;
        match input {
            agent_api::Input.text { value } => {
                if value == ":quit" {
                    break;
                }
                agent_api::emit("status", worker.handle(value).await ?);
            }
            agent_api::Input.command { value } => {
                if value == "quit" {
                    break;
                }
                agent_api::emit("status", worker.handle(value).await ?);
            }
            agent_api::Input.ui { event: _ } => {
                agent_api::emit("status", "UI event received; send a textual workflow command");
            }
            agent_api::Input.skipped => {
                agent_api::emit("status", "Input skipped");
            }
        }
    }
    return 0;
}

task application() -> int | Error {
    let options = parse_options() ?;
    if options.mode == "help" {
        print(usage());
        return 0;
    }
    if system::platform() != "windows" {
        print("The DHE workflow currently accepts only Windows target paths.");
        return 2;
    }
    if options.mode == "release" || options.mode == "resume" || options.mode == "player" || options.mode == "all" {
        return run_agent(options).await ?;
    }
    let worker = DheRun { phase: "idle", config: options, report: "", run_id: "run-" + system::utc_now(), sequence: 0, attempt: 0, last_failure_code: "", same_failure_count: 0, baseline_head: "", hotfix_head: "", baseline_build: "", hotfix_build: "", baseline_version: "", baseline_id: "", pending_effect: "" };
    worker.initialize(options).await ?;
    if options.mode == "inspect" {
        print(worker.inspect().await ?);
        return 0;
    }
    if options.mode == "demo" {
        if options.demo_root == "" {
            return Error::make("runtime", "--demo-root is required for demo mode", ErrorOrigin.user, false);
        }
        let args = if options.demo_skip_build { ["run_test.py", "--poll-timeout", "180", "--skip-build"] } else { ["run_test.py", "--poll-timeout", "180"] };
        let result = common::run_command("python", args, options.demo_root, 3600000).await ?;
        common::append_event(worker.report, 1, worker.run_id, "demo", "result", "demo", if common::process_succeeded(result) { "ok" } else { "failed" }, result.stdout + "\n" + result.stderr).await ?;
        if !common::process_succeeded(result) {
            return 1;
        }
        print("Demo completed");
        return 0;
    }
    print("Unsupported mode: " + options.mode);
    return 2;
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
task main() -> int {
    let result = application().await;
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return result;
        }
    }
}
