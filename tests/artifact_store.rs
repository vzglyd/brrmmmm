use brrmmmm::host::{Artifact, ArtifactStore};

fn make_artifact(kind: &str) -> Artifact {
    Artifact {
        kind: kind.to_string(),
        data: vec![1, 2, 3],
        received_at_ms: 0,
    }
}

#[test]
fn store_published_output_kind() {
    let mut store = ArtifactStore::default();
    store.store(make_artifact("published_output"));
    assert!(store.published_output.is_some());
    assert_eq!(
        store.published_output.as_ref().unwrap().kind,
        "published_output"
    );
    assert!(store.raw_source.is_none());
    assert!(store.normalized.is_none());
}

#[test]
fn store_raw_source_kind() {
    let mut store = ArtifactStore::default();
    store.store(make_artifact("raw_source_payload"));
    assert!(store.raw_source.is_some());
    assert_eq!(
        store.raw_source.as_ref().unwrap().kind,
        "raw_source_payload"
    );
    assert!(store.published_output.is_none());
}

#[test]
fn store_normalized_kind() {
    let mut store = ArtifactStore::default();
    store.store(make_artifact("normalized_payload"));
    assert!(store.normalized.is_some());
    assert_eq!(
        store.normalized.as_ref().unwrap().kind,
        "normalized_payload"
    );
    assert!(store.published_output.is_none());
}

#[test]
fn store_unknown_kind_falls_through_to_published_output() {
    let mut store = ArtifactStore::default();
    store.store(make_artifact("custom_kind"));
    assert!(store.published_output.is_some());
    assert_eq!(store.published_output.as_ref().unwrap().kind, "custom_kind");
}

#[test]
fn take_published_returns_none_on_empty_store() {
    let mut store = ArtifactStore::default();
    assert!(store.take_published().is_none());
}

#[test]
fn take_published_consumes_the_artifact() {
    let mut store = ArtifactStore::default();
    store.store(make_artifact("published_output"));
    let first = store.take_published();
    let second = store.take_published();
    assert!(first.is_some());
    assert!(second.is_none());
}

#[test]
fn store_overwrites_previous_artifact_of_same_kind() {
    let mut store = ArtifactStore::default();
    store.store(Artifact {
        kind: "published_output".to_string(),
        data: vec![1],
        received_at_ms: 0,
    });
    store.store(Artifact {
        kind: "published_output".to_string(),
        data: vec![99],
        received_at_ms: 1,
    });
    let artifact = store.take_published().unwrap();
    assert_eq!(artifact.data, vec![99]);
}
