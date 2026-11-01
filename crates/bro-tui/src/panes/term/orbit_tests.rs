use super::*;

#[test]
fn voice_identity_survives_saved_window_handover_but_not_a_new_lifetime_or_scope() {
    crate::testkit::isolate();
    let root = tempfile::tempdir().unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    let svc = Services::offline(bro_core::config::Settings::default(), tx);
    let meta = Meta {
        sid: "voice-session".into(), harness: None, profile: None, store: None, model: None,
        label: "fixture".into(), name: None, renamed: false, cwd: root.path().to_path_buf(),
        project: ProjectKey { root: root.path().to_path_buf(), key: root.path().to_string_lossy().into_owned(), name: "fixture".into() },
        started: Instant::now(), route_id: None, cleanup: vec![], orchestrator: false,
    };
    let mut term = Term::fixed(meta, "fixture output", svc.clone());
    let before = term.orbit_generation();
    let blob = term.saved(&Spawn::default()).unwrap();
    let info = bro_host::SessionInfo { id: "host-session".into(), pid: None, rows: 24, cols: 80, exited: false, code: None, blob };
    let restored = Term::reattach(&info, svc.clone()).unwrap();
    assert_eq!(before, restored.orbit_generation(), "reconstructed Instant must not change a surviving PTY's voice identity");
    term.meta.started -= Duration::from_secs(60);
    assert_eq!(before, term.orbit_generation(), "UI elapsed time is not process identity");
    let another = Term::fixed(term.meta.clone(), "other output", svc.clone());
    assert_ne!(before, another.orbit_generation(), "a reused session id does not reuse its lifetime");
    term.meta.cwd = root.path().join("different-directory");
    assert_ne!(before, term.orbit_generation(), "relative work must not drift into a changed directory");
    let mut old_blob = info.blob.clone(); old_blob.as_object_mut().unwrap().remove("orbit_lifetime");
    let old_info = bro_host::SessionInfo { blob: old_blob, ..info };
    let legacy = Term::reattach(&old_info, svc).unwrap();
    assert_ne!(before, legacy.orbit_generation(), "legacy metadata cannot invent old-lifetime approval");
}
