#[test]
fn run_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("POST", "/runs"),
            ("POST", "/runs/{run_id}/continue"),
            ("POST", "/runs/{run_id}/cancel"),
            ("POST", "/permissions/{permission_id}/decision"),
        ],
    );
}
