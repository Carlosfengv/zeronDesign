#[test]
fn run_event_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[("GET", "/runs/{run_id}/events")],
    );
}
