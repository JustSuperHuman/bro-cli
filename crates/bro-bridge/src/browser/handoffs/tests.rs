use super::*;
use serde_json::json;

fn snapshot(phase: &str, checked: bool) -> Value {
    json!({"protocol":"bro-browser-v1","tabs":[{"tabId":42}],"handoffs":[{
        "handoffId":"handoff-one","tabId":42,"broSessionId":"owner","phase":phase,
        "outcome":"done","checked":checked
    }]})
}
fn sessions() -> HashSet<String> {
    HashSet::from(["owner".into()])
}

#[test]
fn completion_queues_exactly_one_wake_and_survives_restart() {
    let root = tempfile::tempdir().unwrap();
    let first = Store::new(root.path());
    assert!(
        first
            .observe(&snapshot("human", false), &sessions(), 100)
            .unwrap()
            .is_empty()
    );
    assert!(
        first
            .observe(&snapshot("requested", false), &sessions(), 101)
            .unwrap()
            .is_empty()
    );
    drop(first);
    let restarted = Store::new(root.path());
    let ready = restarted
        .observe(&snapshot("agent", false), &sessions(), 102)
        .unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].session, "owner");
    assert!(ready[0].message().contains("inspect its current page"));
    assert!(restarted.claim(&ready[0].id, 102).unwrap());
    assert!(!restarted.claim(&ready[0].id, 102).unwrap());
    restarted.delivered(&ready[0].id, true).unwrap();
    assert!(
        Store::new(root.path())
            .observe(&snapshot("agent", false), &sessions(), 103)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_agent_that_already_rechecked_does_not_get_a_duplicate_turn() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path());
    assert!(
        store
            .observe(&snapshot("agent", true), &sessions(), 100)
            .unwrap()
            .is_empty()
    );
    assert!(!store.claim("handoff-one", 100).unwrap());
}

#[test]
fn another_pending_handoff_does_not_defer_a_completion_notification() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path());
    assert!(
        store
            .observe(&snapshot("agent", false), &HashSet::new(), 100)
            .unwrap()
            .is_empty()
    );
    let mut active = snapshot("agent", false);
    active["handoffs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"handoffId":"another","tabId":43,"broSessionId":"owner","phase":"human"}));
    let ready = store.observe(&active, &sessions(), 101).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, "handoff-one");
    let ready = store
        .observe(&snapshot("agent", false), &sessions(), 102)
        .unwrap();
    assert!(ready.iter().any(|entry| entry.id == "handoff-one"));
}

#[test]
fn a_closed_tab_wakes_its_owner_but_a_disconnect_is_not_a_completion() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path());
    assert!(
        store
            .observe(&snapshot("requested", false), &sessions(), 100)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .observe(&snapshot("requested", false), &sessions(), 200)
            .unwrap()
            .is_empty()
    );
    let ready = store
        .observe(&json!({"tabs":[],"handoffs":[]}), &sessions(), 201)
        .unwrap();
    assert_eq!(ready.len(), 1);
    assert!(ready[0].closed);
    assert!(ready[0].message().contains("tab was closed"));
}

#[test]
fn a_failed_delivery_and_an_abandoned_claim_can_be_retried() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path());
    store
        .observe(&snapshot("agent", false), &sessions(), 100)
        .unwrap();
    assert!(store.claim("handoff-one", 100).unwrap());
    assert!(!Store::new(root.path()).claim("handoff-one", 101).unwrap());
    assert!(Store::new(root.path()).claim("handoff-one", 146).unwrap());
    store.delivered("handoff-one", false).unwrap();
    assert!(store.claim("handoff-one", 147).unwrap());
}

#[test]
fn separate_host_instances_cannot_claim_the_same_completion() {
    let root = tempfile::tempdir().unwrap();
    Store::new(root.path())
        .observe(&snapshot("agent", false), &sessions(), 100)
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let path = root.path().to_owned();
            std::thread::spawn(move || {
                barrier.wait();
                Store::new(&path).claim("handoff-one", 100).unwrap()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|won| *won)
            .count(),
        1
    );
}
