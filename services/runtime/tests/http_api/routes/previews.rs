#[test]
fn preview_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("GET", "/preview/{project_id}/current"),
            ("GET", "/preview/{project_id}/{version_id}"),
            ("GET", "/previews/{lease_id}"),
        ],
    );
}
