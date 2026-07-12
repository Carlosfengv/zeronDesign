#[test]
fn publication_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("POST", "/projects/{project_id}/publish"),
            ("POST", "/projects/{project_id}/unpublish"),
            ("POST", "/projects/{project_id}/rollback"),
            ("GET", "/projects/{project_id}/deployment-state"),
            ("GET", "/projects/{project_id}/releases"),
            ("GET", "/operations/{operation_id}"),
        ],
    );
}
