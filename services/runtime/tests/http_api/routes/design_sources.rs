#[test]
fn design_source_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("POST", "/design-source-artifacts"),
            ("GET", "/design-source-artifacts/{artifact_id}"),
            ("GET", "/design-source-artifacts/{artifact_id}/content"),
            ("POST", "/design-profiles/import"),
        ],
    );
}
