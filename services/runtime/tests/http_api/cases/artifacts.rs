use super::*;

#[tokio::test]
async fn artifact_serving_supports_next_export_routes_and_assets() {
    let workspace = unique_temp_dir("artifact-next-export");
    let project_root = workspace.join("docs-project/project/out");
    fs::create_dir_all(project_root.join("_next/static/css")).unwrap();
    fs::create_dir_all(project_root.join("docs")).unwrap();
    fs::write(project_root.join("index.html"), "<main>Home</main>").unwrap();
    fs::write(
        project_root.join("docs.html"),
        r#"<link rel="stylesheet" href="/_next/static/css/app.css"><a href="/docs/runtime-flow">Runtime Flow</a><script>self.__next_f.push([1,"{\"href\":\"/docs\"}"])</script>"#,
    )
    .unwrap();
    fs::write(
        project_root.join("docs/runtime-flow.html"),
        "<main>Runtime Flow</main>",
    )
    .unwrap();
    fs::write(
        project_root.join("_next/static/css/app.css"),
        ".flex{display:flex}",
    )
    .unwrap();

    let mut config = RuntimeConfig::from_env();
    config.workspace_root = workspace.clone();
    config.runtime_storage_dir = workspace.join("runtime-storage");
    let store = install_immutable_artifact(&config, "docs-project", &project_root).await;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let docs = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/artifacts/docs-project/current/docs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(docs.status(), StatusCode::OK);
    let body = String::from_utf8(to_bytes(docs.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    assert!(body.contains(r#"href="/artifacts/docs-project/current/_next/static/css/app.css""#));
    assert!(body.contains(r#"href="/artifacts/docs-project/current/docs/runtime-flow""#));
    assert!(body.contains(r#"\"href\":\"/artifacts/docs-project/current/docs\""#));

    let root = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/artifacts/docs-project/current/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(root.status(), StatusCode::OK);

    let nested_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/artifacts/docs-project/current/docs/runtime-flow")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(nested_page.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(nested_page.into_body(), 4096)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("Runtime Flow"));

    let css = app
        .oneshot(
            Request::builder()
                .uri("/artifacts/docs-project/current/_next/static/css/app.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(
        css.headers().get("content-type").unwrap(),
        "text/css; charset=utf-8"
    );
}

#[tokio::test]
async fn artifact_serving_supports_phase_a_global_workspace_root() {
    let workspace = unique_temp_dir("artifact-phase-a-global");
    let project_root = workspace.join("project/dist");
    fs::create_dir_all(project_root.join("_astro")).unwrap();
    fs::write(
        project_root.join("index.html"),
        r#"<link rel="stylesheet" href="/_astro/app.css"><main>Phase A Artifact</main>"#,
    )
    .unwrap();
    fs::write(project_root.join("_astro/app.css"), "body{color:white}").unwrap();

    let mut config = RuntimeConfig::from_env();
    config.workspace_root = workspace.clone();
    config.runtime_storage_dir = workspace.join("runtime-storage");
    let store = install_immutable_artifact(&config, "phase-a-project", &project_root).await;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/artifacts/phase-a-project/current/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("Phase A Artifact"));
    assert!(body.contains(r#"href="/artifacts/phase-a-project/current/_astro/app.css""#));

    let css = app
        .oneshot(
            Request::builder()
                .uri("/artifacts/phase-a-project/current/_astro/app.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(
        css.headers().get("content-type").unwrap(),
        "text/css; charset=utf-8"
    );
}
