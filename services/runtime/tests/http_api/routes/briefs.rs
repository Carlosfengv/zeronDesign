#[test]
fn brief_route_family_is_frozen() {
    crate::contract_manifest::assert_manifest_entries(
        "public",
        &[
            ("GET", "/briefs/{brief_id}"),
            ("POST", "/briefs/{brief_id}/confirm"),
        ],
    );
}
