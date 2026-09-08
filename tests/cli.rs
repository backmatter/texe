use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn texe(current_dir: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_texe"))
        .args(arguments)
        .current_dir(current_dir)
        .output()
        .expect("texe starts")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "texe failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture(text: &str) -> String {
    text.lines().collect::<Vec<_>>().join("\n")
}

#[test]
fn init_creates_a_loadable_nested_project_from_the_real_cli() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = texe(
        directory.path(),
        &[
            "init",
            ".",
            "--yes",
            "--entry",
            "paper/main.tex",
            "--engine",
            "lualatex",
        ],
    );
    assert_success(&output);

    let manifest =
        texe::ProjectManifest::load(&directory.path().join("texe.toml")).expect("manifest loads");
    assert_eq!(manifest.project.entry, Path::new("paper/main.tex"));
    assert_eq!(manifest.toolchain.engine, "lualatex");
    assert_eq!(manifest.toolchain.provider, "managed");
    assert!(directory.path().join("paper/main.tex").is_file());
}

#[test]
fn init_creates_a_basic_paper_and_gives_the_next_command() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = texe(
        directory.path(),
        &[
            "init",
            "research-paper",
            "--yes",
            "--template",
            "basic",
            "--title",
            "Reliable Results",
            "--author",
            "Ada Researcher",
            "--engine",
            "pdflatex",
        ],
    );
    assert_success(&output);

    let root = directory.path().join("research-paper");
    let manifest = texe::ProjectManifest::load(&root.join("texe.toml")).expect("manifest loads");
    assert_eq!(manifest.project.entry, Path::new("main.tex"));
    let source = fs::read_to_string(root.join("main.tex")).expect("starter source");
    assert!(source.contains("\\title{Reliable Results}"));
    assert!(source.contains("\\author{Ada Researcher}"));
    assert!(source.contains("\\section{Methods}"));
    assert!(root.join("references.bib").is_file());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let created_source = format!(
        "created {}",
        Path::new("research-paper").join("main.tex").display()
    );
    assert!(stdout.contains(&created_source), "{stdout}");
    assert!(
        stdout.contains("next: run `texe build --project \"research-paper\"`"),
        "{stdout}"
    );
}

#[test]
fn clean_discovers_the_manifest_and_preserves_identity_and_outputs() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path();
    fs::create_dir_all(root.join("chapters")).expect("nested directory");
    fs::write(
        root.join("texe.toml"),
        r#"schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
"#,
    )
    .expect("manifest");
    fs::write(root.join("main.tex"), b"source").expect("source");
    fs::write(root.join("main.pdf"), b"%PDF").expect("artifact");
    fs::write(root.join("texe.lock"), b"composite").expect("composite lock");
    for path in [
        ".texe/build/build-state.json",
        ".texe/build/output/main.aux",
        ".texe/state/pqty.lock",
        ".texe/texmf/tex/latex/base/article.cls",
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("derived directory");
        fs::write(path, b"derived").expect("derived file");
    }

    let output = texe(&root.join("chapters"), &["clean", "--json"]);
    assert_success(&output);
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("clean report JSON");
    assert_eq!(report["schema"], "texe.clean-report/v1");
    assert!(
        report["freed_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
    );
    assert!(!root.join(".texe").exists());
    assert!(root.join("main.tex").is_file());
    assert!(root.join("main.pdf").is_file());
    assert!(root.join("texe.lock").is_file());
}

#[test]
fn clean_dry_run_reports_exact_project_targets_without_removing_them() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path();
    fs::write(
        root.join("texe.toml"),
        r#"schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
"#,
    )
    .expect("manifest");
    fs::write(root.join("main.tex"), b"source").expect("source");
    let derived = root.join(".texe/build/output/main.aux");
    fs::create_dir_all(derived.parent().expect("parent")).expect("derived directory");
    fs::write(&derived, b"aux").expect("derived file");

    let output = texe(root, &["clean", "--dry-run", "--json"]);
    assert_success(&output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("dry-run JSON");
    assert_eq!(report["schema"], "texe.clean-dry-run/v1");
    assert_eq!(report["project"][0], ".texe/build");
    assert!(derived.is_file(), "dry-run must not remove a byte");
}

#[test]
fn invalid_manifest_commands_fail_before_any_external_tool_is_needed() {
    let directory = tempfile::tempdir().expect("temporary directory");
    fs::write(
        directory.path().join("texe.toml"),
        r#"schema = "texe.project/v1"
[project]
entry = "../outside.tex"
[toolchain]
engine = "pdflatex"
"#,
    )
    .expect("manifest");
    let output = texe(directory.path(), &["doctor"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("project.entry must contain only portable project-relative"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn watch_is_a_first_class_cli_workflow() {
    let output = texe(Path::new("."), &["watch", "--help"]);
    assert_success(&output);
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--poll-ms"));
    assert!(help.contains("--debounce-ms"));
    assert!(help.contains("--frozen"));
}

#[test]
fn redirected_bare_texe_never_prompts_and_gives_a_scriptable_next_step() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = texe(directory.path(), &[]);
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim_end(),
        fixture(include_str!("fixtures/transcripts/bare-redirected.txt"))
    );
    assert!(output.stderr.is_empty());
    assert!(!directory.path().join("texe.toml").exists());
}

#[test]
fn bare_json_is_machine_readable_and_never_enters_the_wizard() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = texe(directory.path(), &["--json"]);
    assert_success(&output);
    assert!(output.stderr.is_empty());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("bare JSON report");
    assert_eq!(report["schema"], "texe.bare-report/v1");
    assert_eq!(report["status"], "command-required");
    assert!(!directory.path().join("texe.toml").exists());
}

#[test]
fn json_errors_keep_stdout_machine_readable_and_share_the_human_category() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let json = texe(directory.path(), &["doctor", "--json"]);
    assert_eq!(json.status.code(), Some(3));
    assert!(json.stderr.is_empty());
    let envelope: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("versioned JSON error");
    assert_eq!(envelope["schema"], "texe.error/v1");
    assert_eq!(envelope["error"]["category"], "project");
    assert_eq!(envelope["error"]["code"], 3);

    let human = texe(directory.path(), &["doctor"]);
    assert_eq!(human.status.code(), Some(3));
    assert!(human.stdout.is_empty());
    let canonical = directory
        .path()
        .canonicalize()
        .expect("canonical temporary directory");
    let human_error = String::from_utf8_lossy(&human.stderr)
        .replace(&canonical.display().to_string(), "<PROJECT>")
        .replace(&directory.path().display().to_string(), "<PROJECT>");
    assert_eq!(
        human_error.trim_end(),
        fixture(include_str!("fixtures/transcripts/missing-project.txt"))
    );
}

#[test]
fn quiet_init_has_no_success_output_but_still_creates_the_project() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = texe(
        directory.path(),
        &["--quiet", "init", "paper", "--yes", "--template", "empty"],
    );
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        fixture(include_str!("fixtures/transcripts/init-quiet.txt"))
    );
    assert!(output.stderr.is_empty());
    assert!(directory.path().join("paper/texe.toml").is_file());
}

#[test]
fn invalid_json_command_returns_the_versioned_usage_envelope() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = texe(directory.path(), &["--json", "not-a-command"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON usage error");
    assert_eq!(envelope["schema"], "texe.error/v1");
    assert_eq!(envelope["error"]["category"], "usage");
    assert_eq!(envelope["error"]["code"], 2);
    assert!(envelope["error"]["action"].as_str().is_some());
}

#[test]
fn version_identifies_the_native_target_and_suite_protocol() {
    let output = texe(Path::new("."), &["--version"]);
    assert_success(&output);
    let version = String::from_utf8_lossy(&output.stdout);
    assert!(version.contains(env!("CARGO_PKG_VERSION")));
    assert!(version.contains("target:"));
    assert!(version.contains("pqty.capabilities/v1"));
}

#[test]
fn help_describes_the_paper_workflow_in_user_language() {
    let output = texe(Path::new("."), &["--help"]);
    assert_success(&output);
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.starts_with("Create, build, and work on a LaTeX paper"),
        "{help}"
    );
    assert!(!help.contains("developer environment"), "{help}");
}

#[test]
fn watch_rejects_out_of_range_debounce_before_loading_a_project() {
    let directory = tempfile::tempdir().expect("temporary directory");
    for value in ["0", "49", "60001", "invalid"] {
        let output = texe(directory.path(), &["watch", "--debounce-ms", value]);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("--debounce-ms"));
    }
}

#[test]
fn adoption_preflight_is_read_only_and_ambiguous_roots_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = "% !TeX program = lualatex\n\\documentclass{article}\n\\begin{document}Original\\end{document}\n";
    fs::write(root.join("paper.v2.tex"), source).unwrap();
    fs::write(root.join("notes.tex"), "% \\documentclass{article}\n").unwrap();
    let output = texe(root, &["adopt", "--check", "--json"]);
    assert_success(&output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["engine"], "lualatex");
    assert_eq!(report["entry"], "paper.v2.tex");
    assert!(!root.join("texe.toml").exists());
    assert!(!root.join(".vscode").exists());
    fs::write(root.join("other.tex"), source).unwrap();
    let output = texe(root, &["adopt", "--yes", "--no-build", "--no-editor"]);
    assert!(!output.status.success());
    assert!(!root.join("texe.toml").exists());
    let output = texe(
        root,
        &[
            "adopt",
            "--yes",
            "--entry",
            "paper.v2.tex",
            "--no-build",
            "--no-editor",
        ],
    );
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(root.join("paper.v2.tex")).unwrap(),
        source
    );
    let output = texe(root, &["editor", "--inspect"]);
    assert_success(&output);
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info["pdf"], "paper.v2.pdf");
    assert_eq!(info["log"], ".texe/build/output/paper.v2.log");
}

#[test]
fn adoption_blockers_precede_project_mutations() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(
        root.join("main.tex"),
        "\\documentclass{article}\n\\usepackage{minted}\n",
    )
    .unwrap();
    let output = texe(root, &["adopt", "--yes", "--no-build", "--no-editor"]);
    assert!(!output.status.success());
    assert!(!root.join("texe.toml").exists());
    assert!(!root.join(".vscode").exists());
}

#[test]
fn adoption_explains_compatibility_and_scans_real_declarations() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(
        root.join("main.tex"),
        "\\documentclass{article}\n\\begin{document}The newly minted coin.\\end{document}",
    )
    .unwrap();
    let output = texe(root, &["adopt", "--check"]);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Ready for the first build"));
    assert!(!root.join("texe.toml").exists());
    fs::write(
        root.join("main.tex"),
        "\\documentclass{article}\n\\input{preamble}\n",
    )
    .unwrap();
    fs::write(root.join("preamble.tex"), "\\usepackage{fontspec}").unwrap();
    let output = texe(root, &["adopt", "--check", "--json"]);
    assert_success(&output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["engine"], "lualatex");
    let output = texe(
        root,
        &["adopt", "--engine", "pdflatex", "--check", "--json"],
    );
    assert_success(&output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["blockers"][0].as_str().unwrap().contains("LuaLaTeX"));
    assert!(!root.join("texe.toml").exists());
}

#[test]
fn adoption_can_accept_previewed_editor_conflicts_without_a_manifest_workaround() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("main.tex"), "\\documentclass{article}").unwrap();
    fs::create_dir(root.join(".vscode")).unwrap();
    let original = "{\"latex-workshop.latex.autoBuild.run\":\"onSave\",\"editor.fontSize\":19}";
    fs::write(root.join(".vscode/settings.json"), original).unwrap();
    let output = texe(root, &["adopt", "--yes", "--no-build"]);
    assert!(!output.status.success());
    assert!(!root.join("texe.toml").exists());
    let output = Command::new(env!("CARGO_BIN_EXE_texe"))
        .args(["adopt", "--yes", "--no-build", "--replace-conflicts"])
        .current_dir(root)
        .env("PATH", "")
        .output()
        .unwrap();
    assert_success(&output);
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".vscode/settings.json")).unwrap()).unwrap();
    assert_eq!(settings["editor.fontSize"], 19);
    assert_eq!(
        settings["latex-workshop.latex.external.build.args"][0],
        "editor-build"
    );
}
