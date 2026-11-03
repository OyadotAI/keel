//! A move, on scratch folders: the project, its lanes, its history and its trust go together, and
//! every refusal leaves it where it was.
use super::*;

struct Scratch {
    _dirs: Vec<tempfile::TempDir>,
    project: Utf8PathBuf,
    roots: Roots,
}

fn scratch(name: &str) -> Scratch {
    let base = tempfile::tempdir().unwrap();
    let home = Utf8PathBuf::from_path_buf(base.path().to_path_buf())
        .unwrap()
        .real()
        .unwrap();
    let project = home.join("elsewhere").join(name);
    std::fs::create_dir_all(&project).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "t"],
        &["config", "user.email", "t@t"],
    ] {
        crate::git::run(&project, args).unwrap();
    }
    std::fs::write(project.join("README.md"), "hi\n").unwrap();
    crate::git::run(&project, &["add", "-A"]).unwrap();
    crate::git::run(&project, &["commit", "-qm", "init"]).unwrap();
    let roots = Roots {
        workspace: home.join("Dev/KeelWorkspace"),
        claude: Some(home.join(".claude")),
        moved: Some(home.join(".keel/moved.json")),
    };
    Scratch {
        _dirs: vec![base],
        project,
        roots,
    }
}

fn state(project: &Utf8Path) -> Arc<AppState> {
    Arc::new(AppState::new(project.to_owned()))
}

#[test]
fn a_moved_project_keeps_its_lane_checkouts() {
    let s = scratch("app");
    crate::worktree::create(&s.project, "one").unwrap();
    crate::worktree::create(&s.project, "two").unwrap();
    let moved = run(&state(&s.project), &s.project, &s.roots).expect("moved");
    let to = Utf8PathBuf::from(&moved.to);
    assert!(!s.project.exists(), "the old folder is gone");
    assert_eq!(to, s.roots.workspace.join("app"));
    let mut lanes = crate::worktree::names(&to);
    lanes.sort();
    assert_eq!(lanes, vec!["one".to_string(), "two".to_string()]);
    for lane in ["one", "two"] {
        crate::git::run(
            &to.join(crate::worktree::DIR).join(lane),
            &["status", "--porcelain"],
        )
        .expect("each lane still opens");
    }
    assert!(
        moved.warnings.iter().all(|w| w.step != "history"),
        "{:?}",
        moved.warnings
    );
}

#[test]
fn moving_foo_leaves_foo_bars_history_alone() {
    let s = scratch("foo");
    let projects = s.roots.claude.as_ref().unwrap().join("projects");
    let sibling = s.project.with_file_name("foo-bar");
    for p in [&s.project, &sibling] {
        let dir = projects.join(keel_workspace::project_key(p));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("s.jsonl"), "{}\n").unwrap();
    }
    let moved = run(&state(&s.project), &s.project, &s.roots).expect("moved");
    let to = Utf8PathBuf::from(&moved.to);
    assert!(
        projects
            .join(keel_workspace::project_key(&to))
            .join("s.jsonl")
            .is_file()
    );
    assert!(
        !projects
            .join(keel_workspace::project_key(&s.project))
            .exists()
    );
    assert!(
        projects
            .join(keel_workspace::project_key(&sibling))
            .join("s.jsonl")
            .is_file(),
        "foo-bar's history stays where it was"
    );
}

#[test]
fn a_moved_lanes_history_moves_with_it() {
    let s = scratch("app");
    crate::worktree::create(&s.project, "one").unwrap();
    let projects = s.roots.claude.as_ref().unwrap().join("projects");
    let lane = s.project.join(crate::worktree::DIR).join("one");
    let dir = projects.join(keel_workspace::project_key(&lane));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("l.jsonl"), "{}\n").unwrap();
    let moved = run(&state(&s.project), &s.project, &s.roots).expect("moved");
    let new_lane = Utf8PathBuf::from(&moved.to)
        .join(crate::worktree::DIR)
        .join("one");
    assert!(
        projects
            .join(keel_workspace::project_key(&new_lane))
            .join("l.jsonl")
            .is_file()
    );
}

#[test]
fn a_moved_project_stays_trusted_and_its_former_path_is_recorded() {
    let s = scratch("app");
    crate::permissions::set_trusted(&s.project, true).unwrap();
    let moved = run(&state(&s.project), &s.project, &s.roots).expect("moved");
    let to = Utf8PathBuf::from(&moved.to);
    assert!(crate::permissions::trusted(&to), "{:?}", moved.warnings);
    let records = keel_workspace::moved::records_at(s.roots.moved.as_ref().unwrap());
    assert_eq!(records.len(), 1);
    assert!(records[0].from.contains(&s.project.to_string()));
    assert_eq!(records[0].to, to.to_string());
    let places = keel_workspace::moved::places_in(&records, &to.join("src"));
    assert!(places.contains(&s.project.join("src")));
}

#[test]
fn a_lane_that_cannot_be_relinked_puts_the_project_back() {
    let s = scratch("app");
    crate::worktree::create(&s.project, "one").unwrap();
    // Its admin entry gone: no repair can relink the checkout, so the move must not stand.
    let admin = s.project.join(".git/worktrees/one");
    std::fs::remove_dir_all(&admin).unwrap();
    let refused = run(&state(&s.project), &s.project, &s.roots).unwrap_err();
    assert!(matches!(refused, Refusal::Repair { .. }), "{refused:?}");
    assert!(s.project.join("README.md").is_file(), "back where it was");
    assert!(!s.roots.workspace.join("app").exists());
}

#[test]
fn moving_refuses_a_taken_name() {
    let s = scratch("app");
    std::fs::create_dir_all(s.roots.workspace.join("app")).unwrap();
    let r = run(&state(&s.project), &s.project, &s.roots).unwrap_err();
    assert!(matches!(r, Refusal::Taken { .. }), "{r:?}");
    assert!(s.project.join("README.md").is_file());
}

#[test]
fn moving_refuses_a_folder_already_inside_or_holding_the_workspace() {
    let s = scratch("app");
    let inside = s.roots.workspace.join("there");
    std::fs::create_dir_all(&inside).unwrap();
    assert_eq!(
        plan(&state(&inside), &inside, &s.roots).refusal,
        Some(Refusal::Inside)
    );
    let home = s
        .roots
        .workspace
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    assert_eq!(
        plan(&state(&home), &home, &s.roots).refusal,
        Some(Refusal::Contains)
    );
}

#[test]
fn moving_refuses_a_folder_inside_a_repository() {
    let s = scratch("app");
    let inner = s.project.join("frontend");
    std::fs::create_dir_all(&inner).unwrap();
    let r = plan(&state(&inner), &inner, &s.roots).refusal;
    assert!(matches!(r, Some(Refusal::Nested { .. })), "{r:?}");
}

#[cfg(unix)]
#[test]
fn moving_refuses_a_link() {
    let s = scratch("app");
    let link = s.project.with_file_name("alias");
    std::os::unix::fs::symlink(&s.project, &link).unwrap();
    let r = plan(&state(&link), &link, &s.roots).refusal;
    assert!(matches!(r, Some(Refusal::Link { .. })), "{r:?}");
}

#[test]
fn moving_refuses_a_running_turn_and_writes_nothing() {
    let s = scratch("app");
    let state = state(&s.project);
    state.claim("lane-1", &s.project, true).unwrap();
    let r = run(&state, &s.project, &s.roots).unwrap_err();
    assert_eq!(
        r,
        Refusal::Turn {
            lane: "lane-1".into()
        }
    );
    assert!(s.project.join("README.md").is_file());
    assert!(
        !s.roots.moved.as_ref().unwrap().exists(),
        "no record of a move that did not happen"
    );
}

#[test]
fn a_rename_error_names_what_a_person_can_do() {
    let cross = std::io::Error::from(std::io::ErrorKind::CrossesDevices);
    assert_eq!(refusal_of(&cross), Refusal::Volume);
    let other = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert!(matches!(refusal_of(&other), Refusal::Rename { .. }));
}
