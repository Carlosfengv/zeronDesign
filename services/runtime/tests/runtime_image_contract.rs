const RUNTIME_DOCKERFILE: &str = include_str!("../Dockerfile");

#[test]
fn runtime_image_installs_multilingual_browser_fonts() {
    for package in [
        "fontconfig",
        "fonts-noto-core",
        "fonts-noto-cjk",
        "fonts-noto-color-emoji",
    ] {
        assert!(
            RUNTIME_DOCKERFILE.contains(package),
            "Runtime Dockerfile must install {package}"
        );
    }
    assert!(RUNTIME_DOCKERFILE.contains("fc-cache -f"));
    assert!(RUNTIME_DOCKERFILE.contains("LANG=C.UTF-8"));
    assert!(RUNTIME_DOCKERFILE.contains("LC_ALL=C.UTF-8"));
    assert!(RUNTIME_DOCKERFILE.contains("/usr/local/lib/anydesign/check-browser-fonts.mjs"));
    assert!(RUNTIME_DOCKERFILE
        .contains("/usr/local/bin/node /usr/local/lib/anydesign/check-browser-fonts.mjs"));
}
