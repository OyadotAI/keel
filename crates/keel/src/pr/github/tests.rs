//! Scratch Git and a fake hosting seam prove retries cannot publish the wrong target or duplicate PRs.
use super::*;
const LINK: &str = "https://github.com/example/project/pull/7";

fn hosting() -> Repository {
    Repository {
        base: "example/project".into(),
        source: "example/project".into(),
        origin: String::new(),
    }
}
fn existing(source: &str) -> String {
    serde_json::json!([{"url":LINK,"headRepository":{"nameWithOwner":source},"headRepositoryOwner":{"login":source.split('/').next().unwrap()},"headRefName":"feature","baseRefName":"main","headRefOid":"f".repeat(40)}]).to_string()
}

fn repository() -> (tempfile::TempDir, camino::Utf8PathBuf, Target, PrQuery) {
    let dir = tempfile::tempdir().unwrap();
    let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "core.hooksPath", "/nonexistent"],
        vec!["commit", "--allow-empty", "-m", "base"],
        vec!["switch", "-c", "feature"],
        vec!["commit", "--allow-empty", "-m", "change"],
    ] {
        crate::git::run(&root, &args).unwrap();
    }
    let query = PrQuery {
        base: "main".into(),
        ..Default::default()
    };
    let target = target::capture(&root, &query).unwrap();
    (dir, root, target, query)
}

#[test]
fn an_existing_pr_is_returned_without_another_push_or_create() {
    let (_dir, root, target, query) = repository();
    let mut calls = Vec::new();
    let opened = publish(
        &root,
        &target,
        &hosting(),
        &query,
        &mut |program, args| {
            calls.push((program.to_string(), args.to_vec()));
            Ok(existing("example/project"))
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(opened.existing);
    assert_eq!(opened.url, LINK);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1[1], "list");
}

#[test]
fn an_uncertain_lookup_never_falls_through_to_push_or_creation() {
    let (_dir, root, target, query) = repository();
    let mut calls = 0;
    let error = publish(
        &root,
        &target,
        &hosting(),
        &query,
        &mut |_, _| {
            calls += 1;
            Err("offline".into())
        },
        &mut |_| {},
    )
    .err()
    .unwrap();
    assert!(error.contains("Could not check"));
    assert_eq!(calls, 1);
}

#[test]
fn a_lost_creation_response_is_reconciled_to_the_existing_link() {
    let (_dir, root, target, query) = repository();
    let mut lists = 0;
    let mut creates = 0;
    let opened = publish(
        &root,
        &target,
        &hosting(),
        &query,
        &mut |program, args| {
            if program == "git" {
                return Ok(String::new());
            }
            if args[1] == "list" {
                lists += 1;
                return Ok(if lists < 3 {
                    "[]".into()
                } else {
                    existing("example/project")
                });
            }
            creates += 1;
            Err("response lost".into())
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(opened.existing);
    assert_eq!(creates, 1);
    assert_eq!(opened.url, LINK);
}

#[test]
fn a_changed_head_cannot_create_a_pr_for_a_different_commit() {
    let (_dir, root, target, query) = repository();
    let mut pushed = Vec::new();
    let error = publish(
        &root,
        &target,
        &hosting(),
        &query,
        &mut |program, args| {
            if program == "git" && args[0] == "push" {
                pushed = args.to_vec();
                crate::git::run(&root, &["commit", "--allow-empty", "-m", "newer"]).unwrap();
                return Ok(String::new());
            }
            if program == "git" {
                return Ok(String::new());
            }
            assert_eq!(args[1], "list");
            Ok("[]".into())
        },
        &mut |_| {},
    )
    .err()
    .unwrap();
    assert!(error.contains("branch changed"));
    assert_eq!(pushed[2], format!("{}:refs/heads/feature", target.sha));
}

#[test]
fn a_real_local_push_uses_the_captured_commit_and_retains_tracking() {
    let (_dir, root, target, query) = repository();
    let remote = tempfile::tempdir().unwrap();
    let remote = camino::Utf8Path::from_path(remote.path()).unwrap();
    crate::git::run(remote, &["init", "--bare"]).unwrap();
    crate::git::run(&root, &["remote", "add", "origin", remote.as_str()]).unwrap();
    let mut created = false;
    let opened = publish(
        &root,
        &target,
        &Repository {
            origin: remote.to_string(),
            ..hosting()
        },
        &query,
        &mut |program, args| {
            if program == "git" {
                return crate::git::run(
                    &root,
                    &args.iter().map(String::as_str).collect::<Vec<_>>(),
                )
                .map(|text| text.trim().to_string());
            }
            if args[1] == "list" {
                return Ok(if created {
                    existing("example/project")
                } else {
                    "[]".into()
                });
            }
            created = true;
            Ok(LINK.into())
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(!opened.existing);
    assert_eq!(
        crate::git::trimmed(remote, &["rev-parse", "refs/heads/feature"]).unwrap(),
        target.sha
    );
    assert_eq!(
        crate::git::trimmed(&root, &["config", "branch.feature.remote"]).unwrap(),
        "origin"
    );
}

#[test]
fn an_invalid_commit_is_refused_and_a_changed_branch_cannot_be_published() {
    let (_dir, root, target, mut query) = repository();
    query.head = "old-commit".into();
    assert!(target::capture(&root, &query).is_err());
    query.head = target.sha;
    query.branch = "different".into();
    let captured = target::capture(&root, &query).unwrap();
    assert!(
        publish(
            &root,
            &captured,
            &hosting(),
            &query,
            &mut |program, args| {
                assert_eq!(program, "gh");
                assert_eq!(args[1], "list");
                Ok("[]".into())
            },
            &mut |_| {}
        )
        .is_err()
    );
}

#[test]
fn a_retry_after_checkout_moves_recovers_the_existing_link_before_refusing_a_push() {
    let (_dir, root, old, mut query) = repository();
    query.head = old.sha.clone();
    query.branch = old.branch.clone();
    crate::git::run(&root, &["commit", "--allow-empty", "-m", "newer"]).unwrap();
    let captured = target::capture(&root, &query).unwrap();
    let mut calls = 0;
    let opened = publish(
        &root,
        &captured,
        &hosting(),
        &query,
        &mut |program, args| {
            calls += 1;
            assert_eq!(program, "gh");
            assert_eq!(args[1], "list");
            Ok(existing("example/project"))
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(opened.existing);
    assert_eq!(calls, 1);
    let error = publish(
        &root,
        &captured,
        &hosting(),
        &query,
        &mut |program, args| {
            assert_eq!(program, "gh");
            assert_eq!(args[1], "list");
            Ok("[]".into())
        },
        &mut |_| {},
    )
    .err()
    .unwrap();
    assert!(error.contains("branch changed"));
}

#[test]
fn another_forks_same_branch_cannot_impersonate_this_publication() {
    let (_dir, root, target, query) = repository();
    let mut creates = 0;
    let opened = publish(
        &root,
        &target,
        &hosting(),
        &query,
        &mut |program, args| {
            if program == "git" {
                return Ok(String::new());
            }
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--repo", "example/project"])
            );
            if args[1] == "list" {
                return Ok(existing(if creates == 0 {
                    "attacker/project"
                } else {
                    "example/project"
                }));
            }
            creates += 1;
            Ok(LINK.into())
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(!opened.existing);
    assert_eq!(creates, 1);
}

#[test]
fn a_matching_source_can_recover_a_newer_head_without_claiming_the_requested_commit() {
    let (_dir, root, target, query) = repository();
    let opened = publish(
        &root,
        &target,
        &hosting(),
        &query,
        &mut |program, _| {
            assert_eq!(program, "gh");
            Ok(existing("example/project"))
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(opened.existing);
    assert_eq!(opened.head, "f".repeat(40));
    assert_ne!(opened.head, target.sha);
}

#[test]
fn a_remote_advance_during_creation_reports_the_hosting_head_not_the_requested_commit() {
    let (_dir, root, target, query) = repository();
    let mut created = false;
    let opened = publish(
        &root,
        &target,
        &hosting(),
        &query,
        &mut |program, args| {
            if program == "git" {
                return Ok(String::new());
            }
            if args[1] == "list" {
                return Ok(if created {
                    existing("example/project")
                } else {
                    "[]".into()
                });
            }
            // Simulate another writer advancing the branch while GitHub creates the PR.
            created = true;
            Ok(LINK.into())
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(!opened.existing);
    assert_ne!(opened.head, target.sha);
    assert_eq!(opened.head, "f".repeat(40));
}
