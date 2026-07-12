#[test]
fn project_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("GET", "/projects/{project_id}/conversation"),
            ("GET", "/projects/{project_id}/runtime-state"),
        ],
    );
}
