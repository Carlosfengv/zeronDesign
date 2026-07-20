#[test]
fn draft_preview_event_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[("GET", "/draft-preview-sessions/{session_id}/events")],
    );
}
