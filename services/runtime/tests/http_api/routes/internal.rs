#[test]
fn internal_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "internal",
        &[
            ("POST", "/internal/template-build"),
            ("POST", "/internal/previews/promote"),
            ("PUT", "/internal/projects/{project_id}/access"),
            ("GET", "/internal/projects/{project_id}/release-evidence"),
            ("POST", "/internal/projects/{project_id}/release-sandbox"),
        ],
    );
}
