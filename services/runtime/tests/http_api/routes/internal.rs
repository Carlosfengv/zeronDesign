#[test]
fn internal_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "internal",
        &[
            ("POST", "/internal/template-build"),
            ("POST", "/internal/previews/promote"),
            ("PUT", "/internal/projects/{project_id}/access"),
            (
                "GET",
                "/internal/projects/{project_id}/design-context-canary-metrics",
            ),
            (
                "PUT",
                "/internal/projects/{project_id}/design-context-enforcement",
            ),
            ("GET", "/internal/projects/{project_id}/release-evidence"),
            ("POST", "/internal/projects/{project_id}/release-sandbox"),
            (
                "GET",
                "/internal/runs/{run_id}/visual-artifacts/{artifact_id}/content",
            ),
            ("POST", "/internal/visual-artifacts/gc"),
        ],
    );
}
