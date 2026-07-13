#[test]
fn artifact_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("GET", "/artifacts/{project_id}/current"),
            ("GET", "/artifacts/{project_id}/current/{*artifact_path}"),
            ("GET", "/artifacts/{project_id}/versions/{version_id}"),
            (
                "GET",
                "/artifacts/{project_id}/versions/{version_id}/{*artifact_path}",
            ),
            ("GET", "/_next/{*artifact_path}"),
        ],
    );
}
