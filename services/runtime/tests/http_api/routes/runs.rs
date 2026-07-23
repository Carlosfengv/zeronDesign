#[test]
fn run_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("POST", "/runs"),
            ("POST", "/runs/{run_id}/continue"),
            ("POST", "/runs/{run_id}/cancel"),
            ("GET", "/runs/{run_id}/design-context-manifest"),
            ("GET", "/runs/{run_id}/design-context-diagnostics"),
            ("GET", "/runs/{run_id}/efficiency-metrics"),
            ("GET", "/runs/{run_id}/model-usage"),
            ("GET", "/runs/{run_id}/budget-profile"),
            ("GET", "/runs/{run_id}/prompt-efficiency"),
            ("POST", "/runs/{run_id}/design-profile-sync-plan"),
            (
                "POST",
                "/runs/{run_id}/design-profile-sync-operations/{operation_id}/confirm",
            ),
            ("POST", "/permissions/{permission_id}/decision"),
        ],
    );
}
