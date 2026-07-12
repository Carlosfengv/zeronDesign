#[test]
fn capture_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "capture",
        &[
            ("GET", "/preview-captures/{lease_id}"),
            ("GET", "/preview-captures/{lease_id}/{*preview_path}"),
        ],
    );
}
