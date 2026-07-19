#[test]
fn design_profile_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("GET", "/design-profiles"),
            ("POST", "/design-profiles"),
            ("GET", "/design-profiles/{design_profile_id}"),
            ("PUT", "/design-profiles/{design_profile_id}"),
            ("POST", "/projects/{project_id}/design-profile"),
            ("DELETE", "/projects/{project_id}/design-profile"),
        ],
    );
}
