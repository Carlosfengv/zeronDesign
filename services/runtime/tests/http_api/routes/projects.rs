#[test]
fn project_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("GET", "/projects/{project_id}/conversation"),
            ("GET", "/projects/{project_id}/history"),
            ("GET", "/projects/{project_id}/draft-preview"),
            ("GET", "/draft-preview-sessions/{session_id}"),
            ("GET", "/draft-preview-sessions/{session_id}/events"),
            ("POST", "/draft-preview-sessions/{session_id}/heartbeat"),
            ("POST", "/draft-preview-sessions/{session_id}/takeover"),
            ("POST", "/projects/{project_id}/element-observations"),
            (
                "GET",
                "/projects/{project_id}/element-observations/{observation_id}",
            ),
            ("POST", "/projects/{project_id}/edit-impact-plans"),
            (
                "GET",
                "/projects/{project_id}/edit-impact-plans/{plan_hash}",
            ),
            (
                "POST",
                "/projects/{project_id}/edit-impact-plans/{plan_hash}/confirm",
            ),
            ("GET", "/projects/{project_id}/assets"),
            ("GET", "/projects/{project_id}/assets/{asset_id}"),
            ("GET", "/projects/{project_id}/runtime-state"),
            ("POST", "/projects/{project_id}/visual-artifacts"),
            (
                "GET",
                "/projects/{project_id}/visual-artifacts/{artifact_id}",
            ),
            (
                "DELETE",
                "/projects/{project_id}/visual-artifacts/{artifact_id}",
            ),
            (
                "GET",
                "/projects/{project_id}/visual-artifacts/{artifact_id}/content",
            ),
            (
                "GET",
                "/projects/{project_id}/runs/{run_id}/visual-bindings",
            ),
            (
                "POST",
                "/projects/{project_id}/runs/{run_id}/visual-bindings",
            ),
            ("POST", "/projects/{project_id}/visual-reviews"),
        ],
    );
}
