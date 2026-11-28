//! Each language's `runnables.scm` over a real fixture, through the task builder its
//! `ContextProvider::runnables` uses.

use std::collections::BTreeMap;

use language::RunContext;
use languages::runnables::PackageJsonData;
use languages::runnables::bash_tasks;
use languages::runnables::composer_script_task;
use languages::runnables::go_tasks;
use languages::runnables::js_test_tasks;
use languages::runnables::package_script_task;
use languages::runnables::python_tasks;
use languages::runnables::shell_block_task;
use task::SpawnInTerminal;
use task_test::assets;
use task_test::relative;
use task_test::runnable_contexts;

/// `row: command line (cwd)`, with the assets path written as `$ASSETS`.
fn line(row: usize, task: &SpawnInTerminal) -> String {
    let cwd = task.cwd.as_deref().map_or_else(|| "none".to_owned(), relative);
    format!("{row}: {} ({cwd})", task.command_label).replace(&*assets().to_string_lossy(), "$ASSETS")
}

fn package_json(file: &str) -> PackageJsonData {
    let path = assets().join(file);
    let contents = std::fs::read_to_string(&path).expect("fixture exists");
    PackageJsonData::new(path.as_path().into(), serde_json::from_str(&contents).expect("valid package.json"))
}

#[test]
fn markdown_shell_blocks_run_their_code_from_the_file_folder() {
    let contexts = runnable_contexts("markdown", tree_sitter_md::LANGUAGE.into(), "markdown/README.md");
    let tasks = contexts
        .iter()
        .filter_map(|(row, cx)| {
            let task = shell_block_task(cx)?;
            Some((line(*row, &task), task.env["CODE_BLOCK"].clone()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tasks,
        [
            ("2: sh -c \"$CODE_BLOCK\" (markdown)", "echo \"hi from sh\"\n"),
            ("6: bash -c \"$CODE_BLOCK\" (markdown)", "ls\n"),
            ("10: zsh -c \"$CODE_BLOCK\" (markdown)", "pwd\n"),
            ("14: sh -c \"$CODE_BLOCK\" (markdown)", "id\n"),
            ("18: fish -c \"$CODE_BLOCK\" (markdown)", "set -l x 1\necho $x\n"),
        ]
        .map(|(line, code)| (line.to_owned(), code.to_owned()))
    );
}

#[test]
fn python_offers_pytest_targets_and_module_main_from_the_worktree_root() {
    let contexts = runnable_contexts("python", tree_sitter_python::LANGUAGE.into(), "python/test_calc.py");
    let mut tasks = contexts.iter().filter_map(|(row, cx)| Some(line(*row, &python_tasks(cx)?))).collect::<Vec<_>>();
    tasks.sort();
    assert_eq!(
        tasks,
        [
            "0: uv run python -m pytest \"python/test_calc.py::test_add\" (.)",
            "4: uv run python -m pytest \"python/test_calc.py::TestCalc\" (.)",
            "5: uv run python -m pytest \"python/test_calc.py::TestCalc::test_sub\" (.)",
            "9: uv run python -m python.test_calc (.)",
        ]
    );
}

#[test]
fn package_json_scripts_run_with_its_package_manager() {
    let contexts = runnable_contexts("json", tree_sitter_json::LANGUAGE.into(), "json/package.json");
    let tags = contexts.iter().map(|(row, cx)| (*row, cx.tag.as_ref())).collect::<Vec<_>>();
    assert_eq!(tags, [(3, "package-script"), (3, "composer-script")]);
    let (row, cx) = &contexts[0];
    let package_manager = package_json("json/package.json").package_manager.unwrap();
    let task = package_script_task(cx, package_manager, cx.capture("script").unwrap().to_owned());
    assert_eq!(line(*row, &task), "3: pnpm run build (json)");
}

#[test]
fn composer_scripts_run_against_the_manifest_folder() {
    let contexts = runnable_contexts("json", tree_sitter_json::LANGUAGE.into(), "json/php/composer.json");
    let (row, cx) = contexts.iter().find(|(_, cx)| cx.tag.as_ref() == "composer-script").unwrap();
    let task = composer_script_task(cx, cx.capture("script").unwrap().to_owned());
    assert_eq!(line(*row, &task), "2: composer -d $ASSETS/json/php lint (.)");
}

#[test]
fn go_offers_package_test_subtest_and_benchmark_runs() {
    let contexts = runnable_contexts("go", tree_sitter_go::LANGUAGE.into(), "go/calc/calc_test.go");
    // The innermost outline symbol the editor passes for each row.
    let symbols = BTreeMap::from([(4, "TestAdd"), (5, "TestAdd"), (8, "BenchmarkAdd")]);
    let mut tasks = contexts
        .into_iter()
        .filter_map(|(row, cx)| {
            let symbol = symbols.get(&row).map(|symbol| (*symbol).to_owned());
            Some(line(row, &go_tasks(&RunContext { symbol, ..cx })?))
        })
        .collect::<Vec<_>>();
    tasks.sort();
    assert_eq!(
        tasks,
        [
            "0: go test (go/calc)",
            "4: go test -run \\^TestAdd\\$ (go/calc)",
            "5: go test -v -run \\^TestAdd\\$/\\^small_numbers\\$ (go/calc)",
            "8: go test -benchmem -run='^$' -bench \\^BenchmarkAdd\\$ (go/calc)",
        ]
    );
}

#[test]
fn bash_scripts_run_from_the_worktree_root() {
    let contexts = runnable_contexts("bash", tree_sitter_bash::LANGUAGE.into(), "bash/build.sh");
    let tasks = contexts
        .iter()
        .flat_map(|(row, cx)| bash_tasks(cx).into_iter().map(|task| line(*row, &task)))
        .collect::<Vec<_>>();
    assert_eq!(tasks, ["0: $ASSETS/bash/build.sh (.)"]);
}

#[test]
fn js_tests_run_with_the_runner_in_package_json() {
    let contexts =
        runnable_contexts("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), "ts/calc.test.ts");
    let data = package_json("ts/package.json");
    let tasks = contexts
        .into_iter()
        .flat_map(|(row, cx)| {
            // The outline symbol the editor passes for a `test(...)` call.
            let cx = RunContext { symbol: Some("adds".to_owned()), ..cx };
            js_test_tasks(&cx, &data, "npm").into_iter().map(move |task| line(row, &task))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tasks,
        ["2: npm exec -- vitest run --no-file-parallelism --testNamePattern \"adds\" $ASSETS/ts/calc.test.ts (ts)"]
    );
}
