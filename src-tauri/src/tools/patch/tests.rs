use super::hunk::apply_hunks;
use super::parser::{Hunk, HunkLine};
use super::support::sha256_hex;
use super::{apply_patch, edit, edit_file, edit_many, file_ops, patch_check};
use crate::tools::context::ToolContext;
use serde_json::{json, Value};
use tempfile::tempdir;

fn context_with_file() -> (tempfile::TempDir, tempfile::TempDir, ToolContext) {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    std::fs::write(workspace.path().join("main.rs"), "old\n").expect("file");
    let context =
        ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
            .expect("context");
    (workspace, harness, context)
}

fn patch() -> Value {
    json!({
        "patch": "--- a/main.rs\n+++ b/main.rs\n@@\n-old\n+new\n"
    })
}

#[test]
fn patch_check_does_not_modify_workspace() {
    let (_workspace, _harness, context) = context_with_file();
    let result = patch_check(&context, &patch()).expect("patch check");
    assert_eq!(result["preflight"], true);
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "old\n"
    );
}

#[test]
fn preserves_crlf_when_inserting_multiple_lines() {
    let input = "one\r\ntwo\r\n";
    let hunk = Hunk {
        old_start: Some(1),
        lines: vec![
            HunkLine::Context("one".into()),
            HunkLine::Add("insert-a".into()),
            HunkLine::Add("insert-b".into()),
            HunkLine::Context("two".into()),
        ],
    };
    assert_eq!(
        apply_hunks(input, &[hunk], None).expect("patch"),
        "one\r\ninsert-a\r\ninsert-b\r\ntwo\r\n"
    );
}

#[test]
fn delete_then_add_same_path_replaces_instead_of_concatenating_old_content() {
    let (_workspace, _harness, context) = context_with_file();
    let result = apply_patch(
            &context,
            &json!({
                "patch": "*** Begin Patch\n*** Delete File: main.rs\n*** Add File: main.rs\n+fresh\n*** End Patch\n"
            }),
        )
        .expect("replace file");
    assert_eq!(result["files_modified"], json!(["main.rs"]));
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "fresh\n"
    );
}

#[test]
fn validation_failure_in_later_file_keeps_all_files_unchanged() {
    let (_workspace, _harness, context) = context_with_file();
    let error = apply_patch(
            &context,
            &json!({
                "patch": "--- a/main.rs\n+++ b/main.rs\n@@\n-old\n+new\n--- a/missing.rs\n+++ b/missing.rs\n@@\n-old\n+new\n"
            }),
        )
        .expect_err("later file fails preflight");
    assert_eq!(error.to_error_value()["code"], "NOT_FOUND");
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "old\n"
    );
}

#[test]
fn edit_file_requires_exact_match_and_returns_diff() {
    let (_workspace, _harness, context) = context_with_file();
    let before = sha256_hex(b"old\n");
    let result = edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "expected_sha256": before,
            "edits": [{
                "type": "replace",
                "old_text": "old",
                "new_text": "new",
                "expected_occurrences": 1
            }]
        }),
    )
    .expect("edit file");
    assert!(result["diff"].as_str().unwrap().contains("+new"));
    assert_eq!(result["before_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "new\n"
    );
}

#[test]
fn dry_run_edit_plan_replays_once_and_rejects_stale_reuse() {
    let (_workspace, _harness, context) = context_with_file();
    let planned = edit(
        &context,
        &json!({
            "files": [{
                "path": "main.rs",
                "edits": [{
                    "type": "replace",
                    "old_text": "old",
                    "new_text": "new"
                }]
            }],
            "dry_run": true,
            "reason": "guarded replay test"
        }),
    )
    .expect("dry-run plan");
    let plan = planned["edit_plan"].clone();
    assert_eq!(plan["tool"], "edit");
    assert_eq!(plan["arguments"]["dry_run"], false);
    assert_eq!(plan["arguments"]["reason"], "guarded replay test");
    assert_eq!(
        plan["arguments"]["files"][0]["expected_sha256"],
        planned["before_sha256"]
    );
    assert_eq!(plan["stateful_dependencies"], json!([]));
    assert_eq!(plan["plan_sha256"].as_str().unwrap().len(), 64);

    let replayed = edit(&context, &plan["arguments"]).expect("replay plan");
    assert_eq!(replayed["applied"], true);
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "new\n"
    );

    let stale = edit(&context, &plan["arguments"]).expect_err("stale replay");
    assert_eq!(stale.to_error_value()["code"], "FILE_VERSION_MISMATCH");
}

#[test]
fn dry_run_edit_many_plan_replays_atomically() {
    let (_workspace, _harness, context) = context_with_file();
    std::fs::write(context.workspace.root().join("second.rs"), "second\n").expect("second fixture");
    let planned = edit(
        &context,
        &json!({
            "dry_run": true,
            "files": [
                {
                    "path": "main.rs",
                    "edits": [{ "type": "replace", "old_text": "old", "new_text": "NEW" }]
                },
                {
                    "path": "second.rs",
                    "edits": [{ "type": "replace", "old_text": "second", "new_text": "SECOND" }]
                }
            ]
        }),
    )
    .expect("edit-many plan");
    let plan = planned["edit_plan"].clone();
    assert_eq!(plan["tool"], "edit");
    assert_eq!(plan["arguments"]["files"].as_array().unwrap().len(), 2);
    assert_eq!(plan["arguments"]["dry_run"], false);
    assert_eq!(plan["plan_sha256"].as_str().unwrap().len(), 64);

    let replayed = edit(&context, &plan["arguments"]).expect("replay edit");
    assert_eq!(replayed["applied"], true);
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "NEW\n"
    );
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("second.rs")).unwrap(),
        "SECOND\n"
    );
}

#[test]
fn edit_reports_ambiguous_candidates_with_context_without_writing() {
    let (_workspace, _harness, context) = context_with_file();
    std::fs::write(
        context.workspace.root().join("main.rs"),
        "fn first() {\n    return value;\n}\n\nfn second() {\n    return value;\n}\n",
    )
    .expect("fixture");

    let error = edit(
        &context,
        &json!({
            "files": [{
                "path": "main.rs",
                "edits": [{
                    "type": "replace",
                    "old_text": "return value;",
                    "new_text": "return result;"
                }]
            }]
        }),
    )
    .expect_err("ambiguous target");
    let value = error.to_error_value();
    assert_eq!(value["code"], "EDIT_MATCH_COUNT_MISMATCH");
    assert_eq!(value["details"]["actual_occurrences"], 2);
    assert_eq!(value["details"]["candidate_lines"], json!([2, 6]));
    assert_eq!(
        value["details"]["candidate_contexts"]
            .as_array()
            .expect("candidate contexts")
            .len(),
        2
    );
    assert_eq!(value["details"]["candidate_contexts_truncated"], false);
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "fn first() {\n    return value;\n}\n\nfn second() {\n    return value;\n}\n"
    );
}

#[test]
fn edit_rejects_proposal_mode_for_multiple_files() {
    let (_workspace, _harness, context) = context_with_file();
    std::fs::write(context.workspace.root().join("second.rs"), "second\n").expect("second fixture");
    let error = edit(
        &context,
        &json!({
            "files": [
                {
                    "path": "main.rs",
                    "apply_proposal": { "proposal_id": "proposal" }
                },
                {
                    "path": "second.rs",
                    "edits": [{ "type": "replace", "old_text": "second", "new_text": "SECOND" }]
                }
            ]
        }),
    )
    .expect_err("proposal requires one file");
    assert_eq!(error.to_error_value()["code"], "EDIT_CONTRACT_INVALID");
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "old\n"
    );
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("second.rs")).unwrap(),
        "second\n"
    );
}

#[test]
fn edit_file_exact_mode_tolerates_newline_style_and_preserves_crlf() {
    let (_workspace, _harness, context) = context_with_file();
    std::fs::write(
        context.workspace.root().join("main.rs"),
        "fn main() {\r\n    let first = 1;\r\n    let second = 2;\r\n}\r\n",
    )
    .expect("fixture");

    let result = edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "edits": [{
                "type": "replace",
                "old_text": "    let first = 1;\n    let second = 2;",
                "new_text": "    let first = 10;\n    let second = 20;",
                "before_context": "fn main() {\n",
                "after_context": "\n}\n"
            }]
        }),
    )
    .expect("newline-compatible exact edit");

    assert_eq!(result["applied"], true);
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "fn main() {\r\n    let first = 10;\r\n    let second = 20;\r\n}\r\n"
    );
}

#[test]
fn edit_file_normalizes_replacement_and_insert_text_to_file_style() {
    let (_workspace, _harness, context) = context_with_file();
    std::fs::write(
        context.workspace.root().join("main.rs"),
        "alpha\r\nomega\r\n",
    )
    .expect("fixture");

    edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "edits": [{
                "type": "insert_after",
                "anchor": "alpha\n",
                "text": "inserted\n"
            }]
        }),
    )
    .expect("newline-compatible insert");
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "alpha\r\ninserted\r\nomega\r\n"
    );

    std::fs::write(context.workspace.root().join("main.rs"), "one\ntwo\n").expect("lf fixture");
    edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "edits": [{
                "type": "replace",
                "old_text": "one\r\ntwo",
                "new_text": "first\r\nsecond"
            }]
        }),
    )
    .expect("lf-preserving replacement");
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn edit_file_whitespace_mode_tolerates_indent_and_crlf_differences() {
    let (_workspace, _harness, context) = context_with_file();
    std::fs::write(
        context.workspace.root().join("main.rs"),
        "fn main() {\r\n    let value = 1;\r\n}\r\n",
    )
    .expect("fixture");
    let result = edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "edits": [{
                "type": "replace",
                "old_text": "fn main() {\n  let value = 1;\n}",
                "new_text": "fn main() {\r\n    let value = 2;\r\n}",
                "match_mode": "whitespace"
            }]
        }),
    )
    .expect("whitespace edit");
    assert_eq!(result["applied"], true);
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "fn main() {\r\n    let value = 2;\r\n}\r\n"
    );
}

#[test]
fn edit_file_exact_failure_returns_cost_guidance_and_applies_replacement() {
    let (_workspace, _harness, context) = context_with_file();
    std::fs::write(
        context.workspace.root().join("main.rs"),
        "let  value = 1;\n",
    )
    .expect("fixture");
    let proposal = edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "edits": [{
                "type": "replace",
                "old_text": "let value = 1;",
                "new_text": "let value = 2;"
            }]
        }),
    )
    .expect("proposal");
    assert_eq!(proposal["status"], "proposal_required");
    assert_eq!(proposal["applied"], false);
    assert_eq!(proposal["proposed_content_included"], true);
    assert_eq!(proposal["proposed_content"], "let value = 2;\n");
    assert_eq!(proposal["preferred_format"], "replacement");
    assert_eq!(proposal["replacement_bytes"], 14);
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "let  value = 1;\n"
    );

    let proposal_id = proposal["proposal_id"].as_str().unwrap();
    let result = edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "apply_proposal": {
                "proposal_id": proposal_id,
                "replacement": "let value = 3;"
            }
        }),
    )
    .expect("apply proposal");
    assert_eq!(result["status"], "proposal_applied");
    assert_eq!(result["proposal_apply_format"], "replacement");
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "let value = 3;\n"
    );
}

#[test]
fn edit_file_rejects_stale_proposal() {
    let (_workspace, _harness, context) = context_with_file();
    std::fs::write(
        context.workspace.root().join("main.rs"),
        "let  value = 1;\n",
    )
    .expect("fixture");
    let proposal = edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "edits": [{
                "type": "replace",
                "old_text": "let value = 1;",
                "new_text": "let value = 2;"
            }]
        }),
    )
    .expect("proposal");
    std::fs::write(
        context.workspace.root().join("main.rs"),
        "let  value = 9;\n",
    )
    .expect("change fixture");
    let error = edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "apply_proposal": {
                "proposal_id": proposal["proposal_id"]
            }
        }),
    )
    .expect_err("stale proposal");
    assert_eq!(error.to_error_value()["code"], "EDIT_PROPOSAL_STALE");
}

#[test]
fn edit_file_rejects_stale_hash() {
    let (_workspace, _harness, context) = context_with_file();
    let error = edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "expected_sha256": "0".repeat(64),
            "edits": [{
                "type": "replace",
                "old_text": "old",
                "new_text": "new"
            }]
        }),
    )
    .expect_err("stale hash");
    assert_eq!(error.to_error_value()["code"], "FILE_VERSION_MISMATCH");
}

#[test]
fn edit_file_aggregates_contract_issues_with_guarded_recovery() {
    let (_workspace, _harness, context) = context_with_file();
    let error = edit_file(
        &context,
        &json!({
            "path": "main.rs",
            "edits": [{
                "type": "replace",
                "old_text": "old",
                "anchor": "unexpected"
            }]
        }),
    )
    .expect_err("invalid edit contract");
    let value = error.to_error_value();
    assert_eq!(value["code"], "EDIT_CONTRACT_INVALID");
    assert_eq!(value["details"]["issue_count"], 2);
    assert_eq!(value["details"]["path"], "main.rs");
    assert!(value["details"]["actual_sha256"]
        .as_str()
        .is_some_and(|hash| hash.len() == 64));
    assert_eq!(value["details"]["recovery_actions"][1]["tool"], "edit");
}

#[test]
fn edit_many_contract_failure_identifies_file_and_preserves_atomicity() {
    let (_workspace, _harness, context) = context_with_file();
    std::fs::write(context.workspace.root().join("second.rs"), "second\n").expect("second fixture");
    let error = edit_many(
        &context,
        &json!({
            "files": [
                {
                    "path": "main.rs",
                    "edits": [{
                        "type": "replace",
                        "old_text": "old",
                        "new_text": "new"
                    }]
                },
                {
                    "path": "second.rs",
                    "edits": [{
                        "type": "replace",
                        "old_text": "second",
                        "anchor": "unexpected"
                    }]
                }
            ]
        }),
    )
    .expect_err("second file contract failure");
    let value = error.to_error_value();
    assert_eq!(value["code"], "EDIT_CONTRACT_INVALID");
    assert_eq!(value["details"]["file_index"], 1);
    assert_eq!(value["details"]["path"], "second.rs");
    assert!(value["details"]["recovery_actions"].is_array());
    assert_eq!(
        std::fs::read_to_string(context.workspace.root().join("main.rs")).unwrap(),
        "old\n"
    );
}

#[test]
fn patch_without_line_numbers_rejects_ambiguous_context() {
    let hunk = Hunk {
        old_start: None,
        lines: vec![
            HunkLine::Context("same".into()),
            HunkLine::Add("inserted".into()),
        ],
    };
    let error =
        apply_hunks("same\nother\nsame\n", &[hunk], Some("ambiguous.txt")).expect_err("ambiguous");
    let value = error.to_error_value();
    assert_eq!(value["code"], "PATCH_CONTEXT_AMBIGUOUS");
    let actions = value["details"]["recovery_actions"]
        .as_array()
        .expect("candidate recovery actions");
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0]["candidate_line"], 1);
    assert_eq!(actions[1]["candidate_line"], 3);
    assert_eq!(actions[0]["required_arguments"], json!([]));
    assert_eq!(actions[0]["arguments"]["files"][0]["path"], "ambiguous.txt");
    assert_eq!(
        actions[0]["arguments"]["files"][0]["edits"][0],
        json!({
            "type": "replace_lines",
            "start_line": 1,
            "end_line": 1,
            "expected_text": "same",
            "new_text": "same\ninserted"
        })
    );
}

#[test]
fn patch_preflight_reports_multiple_hunk_issues_together() {
    let hunks = vec![
        Hunk {
            old_start: None,
            lines: vec![HunkLine::Context("missing-one".into())],
        },
        Hunk {
            old_start: None,
            lines: vec![HunkLine::Context("missing-two".into())],
        },
    ];
    let error = apply_hunks("actual\ncontent\n", &hunks, None).expect_err("preflight issues");
    let value = error.to_error_value();
    assert_eq!(value["code"], "PATCH_PREFLIGHT_FAILED");
    assert_eq!(value["details"]["issue_count"], 2);
    assert_eq!(value["details"]["issues"].as_array().unwrap().len(), 2);
}

#[test]
fn apply_patch_checks_expected_hash_and_returns_versions() {
    let (_workspace, _harness, context) = context_with_file();
    let before = sha256_hex(b"old\n");
    let result = apply_patch(
        &context,
        &json!({
            "patch": "--- a/main.rs\n+++ b/main.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n",
            "expected_sha256": { "main.rs": before.clone() }
        }),
    )
    .expect("apply patch");
    assert_eq!(result["preflight"], true);
    assert_eq!(result["applied"], true);
    assert!(result["diff"].as_str().unwrap().contains("+new"));
    assert_eq!(result["file_versions"][0]["before_sha256"], before);
    assert_eq!(
        result["file_versions"][0]["after_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
}

#[test]
fn file_ops_deletes_directories_with_confirmation_for_non_empty_trees() {
    let (_workspace, _harness, context) = context_with_file();
    let root = context.workspace.root().to_path_buf();
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::create_dir_all(root.join("tree/nested")).unwrap();
    std::fs::write(root.join("tree/a.txt"), "a").unwrap();
    std::fs::write(root.join("tree/nested/b.txt"), "b").unwrap();

    // An empty directory needs no confirmation.
    let empty = file_ops(
        &context,
        &json!({"operations": [{"type": "delete", "path": "empty"}]}),
    )
    .expect("delete empty directory");
    assert_eq!(empty["directories_deleted"], json!(["empty"]));
    assert!(!root.join("empty").exists());

    // A non-empty tree is refused without confirm=true and left untouched.
    let refused = file_ops(
        &context,
        &json!({"operations": [{"type": "delete", "path": "tree"}]}),
    )
    .expect_err("non-empty directory needs confirm");
    assert_eq!(
        refused.to_error_value()["code"],
        "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION"
    );
    assert!(root.join("tree/nested/b.txt").is_file());

    // Dry run reports the plan without deleting.
    let planned = file_ops(
        &context,
        &json!({"operations": [{"type": "delete", "path": "tree"}], "confirm": true, "dry_run": true}),
    )
    .expect("dry run");
    assert_eq!(
        planned["affected_files"][0]["operation"],
        "delete_directory"
    );
    assert_eq!(planned["affected_files"][0]["entries"], 3);
    assert!(root.join("tree").is_dir());

    let deleted = file_ops(
        &context,
        &json!({"operations": [{"type": "delete", "path": "tree"}], "confirm": true}),
    )
    .expect("delete tree");
    assert_eq!(deleted["directories_deleted"], json!(["tree"]));
    assert!(!root.join("tree").exists());
    let leftovers = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains("ctm-delete"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn file_ops_directory_delete_refuses_root_git_and_overlapping_operations() {
    let (_workspace, _harness, context) = context_with_file();
    let root = context.workspace.root().to_path_buf();
    std::fs::create_dir_all(root.join("vendor/lib/.git")).unwrap();
    std::fs::create_dir_all(root.join("build")).unwrap();
    std::fs::write(root.join("build/out.txt"), "x").unwrap();

    let root_error = file_ops(
        &context,
        &json!({"operations": [{"type": "delete", "path": "."}], "confirm": true}),
    )
    .expect_err("workspace root");
    assert_eq!(root_error.to_error_value()["code"], "INVALID_ARGUMENT");

    let git_error = file_ops(
        &context,
        &json!({"operations": [{"type": "delete", "path": "vendor"}], "confirm": true}),
    )
    .expect_err("nested repository metadata");
    assert_eq!(git_error.to_error_value()["code"], "PROTECTED_PATH");
    assert!(root.join("vendor/lib/.git").is_dir());

    let overlap = file_ops(
        &context,
        &json!({
            "operations": [
                {"type": "delete", "path": "build"},
                {"type": "create", "path": "build/new.txt", "content": "y"}
            ],
            "confirm": true
        }),
    )
    .expect_err("overlapping paths");
    assert_eq!(overlap.to_error_value()["code"], "INVALID_ARGUMENT");
    assert!(root.join("build/out.txt").is_file());
    assert!(!root.join("build/new.txt").exists());
}
