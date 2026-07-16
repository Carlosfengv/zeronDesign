use anydesign_runtime::tools::sandbox::sandbox_tools;
use sha2::{Digest, Sha256};

const SANDBOX_TOOL_CONTRACT_V2: &[(&str, &str)] = &[
    (
        "fs.read",
        "5f44faa29592ec02a89e7547856c46c4d100b0a62a9866c3d05a25154d081cd9",
    ),
    (
        "design_source.read_sections",
        "8657fe1b97dd31cddda27a7b44896276c8337754bc57f533a63945075aee1642",
    ),
    (
        "fs.list",
        "5f44faa29592ec02a89e7547856c46c4d100b0a62a9866c3d05a25154d081cd9",
    ),
    (
        "fs.search",
        "ff21f1e14707db1f88294ddf2d25b7e7318f1bce3d260130f1faa7884fdca5d7",
    ),
    (
        "fs.write",
        "daf03cd8ca9a73cc20ea947f0d89aa71eaf131f4828215f6dd988adc470ac137",
    ),
    (
        "fs.write_chunk",
        "33f8f3153b712a54675cb9e3703b5a8bcc462b3fe223acd2255b79cbdbabc4d1",
    ),
    (
        "fs.commit_chunks",
        "9b29741db1eb80ac7fe9fdbffaf83f74cd51698cf5ed93ff4d56545b28fd5aa9",
    ),
    (
        "fs.patch",
        "b931fcc8fcb446b6c71ecad49a34d062bfffb61a0a97c7d2553f15aaf3bdb69b",
    ),
    (
        "fs.multi_patch",
        "86db07e2ffa94a59403975229aac1baba0291e75f482fcea366ff6384c26907b",
    ),
    (
        "style.update_tokens",
        "186683498b03135a33637b49c897bf0a85d770cbb66bc288b02a389c9212a23d",
    ),
    (
        "fs.delete",
        "5f44faa29592ec02a89e7547856c46c4d100b0a62a9866c3d05a25154d081cd9",
    ),
    (
        "shell.run",
        "757a20d4086d7744be003c3aa5ebc0c5ff206a11a82b9f6f50e31329e8d2b609",
    ),
    (
        "project.init",
        "7efdb0f4172e80528a487cb72726a9b1da76d8b14c1da3110d49d5443063d981",
    ),
    (
        "project.write_page",
        "b02f844e9acc5cc050c6d5e9010f3d3105bf9b1bf9e5c77530d2b21e3ef21744",
    ),
    (
        "project.inspect",
        "d746974fa9afd5e951f76f9af38954b0ad7f436f2120dc974da65e5ee39f856f",
    ),
    (
        "project.build",
        "c3c384933f50e4ba94cb7395ca9075b8dd4fe0518b1c860376883db4b5dbc0c3",
    ),
    (
        "project.ensure_dependencies",
        "fc417296e9e1217b55d1e4f80e26196a813cd4278114775a57fe586738311791",
    ),
    (
        "package.install",
        "fc417296e9e1217b55d1e4f80e26196a813cd4278114775a57fe586738311791",
    ),
    (
        "preview.rebuilding",
        "48223277c49a1a383dbe9bf9b899df509ac81dc8aee1aec7824168d3053fb938",
    ),
    (
        "preview.report_candidate",
        "7c3d955e2836df15e135a1153cbf6c8dff9fe21a7731800be68006d7d64474a5",
    ),
    (
        "preview.publish",
        "58821291ece11e38e59f8e75805aaaaf3b45d024790868890fda64ab081a0f58",
    ),
    (
        "preview.start",
        "0e6140c510da8fb00b63148a4509cd97f2803addbfd868d0b953f3436ed60887",
    ),
    (
        "preview.status",
        "d746974fa9afd5e951f76f9af38954b0ad7f436f2120dc974da65e5ee39f856f",
    ),
    (
        "preview.stop",
        "d746974fa9afd5e951f76f9af38954b0ad7f436f2120dc974da65e5ee39f856f",
    ),
    (
        "diagnostics.build_log",
        "d746974fa9afd5e951f76f9af38954b0ad7f436f2120dc974da65e5ee39f856f",
    ),
    (
        "diagnostics.typescript",
        "d746974fa9afd5e951f76f9af38954b0ad7f436f2120dc974da65e5ee39f856f",
    ),
    (
        "diagnostics.accessibility",
        "d746974fa9afd5e951f76f9af38954b0ad7f436f2120dc974da65e5ee39f856f",
    ),
    (
        "preview.audit_responsive",
        "d746974fa9afd5e951f76f9af38954b0ad7f436f2120dc974da65e5ee39f856f",
    ),
    (
        "design_context.status",
        "d746974fa9afd5e951f76f9af38954b0ad7f436f2120dc974da65e5ee39f856f",
    ),
    (
        "browser.open",
        "45e0e2b57d9db3323a7fd5dca298dd49947a0c51bd9b90043aa15919a0d21d60",
    ),
    (
        "browser.screenshot",
        "ff7a99109798352a570199a1c02d7ae1f18f7bb170084fbc1d8de614f37882ae",
    ),
    (
        "browser.inspect",
        "d746974fa9afd5e951f76f9af38954b0ad7f436f2120dc974da65e5ee39f856f",
    ),
];

#[test]
fn sandbox_tool_order_and_input_schemas_match_v2_contract() {
    let tools = sandbox_tools();
    let actual = tools
        .iter()
        .map(|tool| {
            let schema = serde_json::to_vec(&tool.input_schema()).expect("serialize tool schema");
            let digest = Sha256::digest(schema);
            (tool.name(), format!("{digest:x}"))
        })
        .collect::<Vec<_>>();
    let expected = SANDBOX_TOOL_CONTRACT_V2
        .iter()
        .map(|(name, digest)| (*name, (*digest).to_string()))
        .collect::<Vec<_>>();

    assert_eq!(actual, expected);
}

#[test]
fn sandbox_tool_alias_loading_and_interrupt_contract_remains_stable() {
    for tool in sandbox_tools() {
        assert!(tool.aliases().is_empty(), "{} aliases changed", tool.name());
        assert_eq!(
            format!("{:?}", tool.tool_loading()),
            "Eager",
            "{} loading changed",
            tool.name()
        );
        assert_eq!(
            format!("{:?}", tool.interrupt_behavior()),
            "Block",
            "{} interrupt behavior changed",
            tool.name()
        );
    }
}
