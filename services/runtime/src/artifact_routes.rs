use crate::types::canonical_json_hash;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;

pub const ARTIFACT_ROUTE_MANIFEST_SCHEMA: &str = "artifact-route-manifest@1";
pub const ARTIFACT_ROUTE_MANIFEST_FILE: &str = ".anydesign-artifact-routes.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutePolicy {
    Root,
    TrailingSlash,
    CleanHtml,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRouteContract {
    pub entry_route: String,
    pub canonical_policy: RoutePolicy,
}

impl ArtifactRouteContract {
    pub fn website() -> Self {
        Self {
            entry_route: "/".to_string(),
            canonical_policy: RoutePolicy::Root,
        }
    }

    pub fn docs() -> Self {
        Self {
            entry_route: "/docs/".to_string(),
            canonical_policy: RoutePolicy::TrailingSlash,
        }
    }

    pub fn validate(&self) -> Result<(), ArtifactRouteError> {
        validate_route(&self.entry_route)?;
        let canonical = canonicalize_route(&self.entry_route, self.canonical_policy);
        if canonical != self.entry_route {
            return Err(ArtifactRouteError::invalid_contract(format!(
                "entry route {} is not canonical for {:?}; expected {canonical}",
                self.entry_route, self.canonical_policy
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRouteFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRouteTarget {
    pub file: String,
    pub sha256: String,
    pub content_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRouteManifest {
    pub schema_version: String,
    pub build_id: String,
    pub entry_route: String,
    pub canonical_policy: RoutePolicy,
    pub routes: BTreeMap<String, ArtifactRouteTarget>,
    pub aliases: BTreeMap<String, String>,
}

impl ArtifactRouteManifest {
    pub fn build(
        build_id: impl Into<String>,
        contract: &ArtifactRouteContract,
        files: impl IntoIterator<Item = ArtifactRouteFile>,
    ) -> Result<Self, ArtifactRouteError> {
        contract.validate()?;
        let build_id = build_id.into();
        if build_id.trim().is_empty() {
            return Err(ArtifactRouteError::invalid_manifest(
                "buildId must not be empty",
            ));
        }

        let mut routes = BTreeMap::<String, ArtifactRouteTarget>::new();
        let mut sources_by_route = HashMap::<String, String>::new();
        for file in files {
            validate_artifact_path(&file.path)?;
            validate_sha256(&file.sha256)?;
            let Some(raw_route) = route_for_html_file(&file.path) else {
                continue;
            };
            let route = canonicalize_route(&raw_route, contract.canonical_policy);
            if let Some(existing) = sources_by_route.get(&route).cloned() {
                let existing_target = routes.get(&route).ok_or_else(|| {
                    ArtifactRouteError::invalid_manifest(format!(
                        "route source index is missing target for {route}"
                    ))
                })?;
                if let Some(preferred) = equivalent_next_not_found_alias(
                    &existing,
                    &existing_target.sha256,
                    &file.path,
                    &file.sha256,
                ) {
                    if preferred == file.path {
                        sources_by_route.insert(route.clone(), file.path.clone());
                        routes.insert(
                            route,
                            ArtifactRouteTarget {
                                file: file.path,
                                sha256: file.sha256,
                                content_type: "text/html; charset=utf-8".to_string(),
                            },
                        );
                    }
                    continue;
                }
                return Err(ArtifactRouteError::ambiguous(
                    route,
                    vec![existing, file.path],
                ));
            }
            sources_by_route.insert(route.clone(), file.path.clone());
            routes.insert(
                route,
                ArtifactRouteTarget {
                    file: file.path,
                    sha256: file.sha256,
                    content_type: "text/html; charset=utf-8".to_string(),
                },
            );
        }

        if !routes.contains_key(&contract.entry_route) {
            return Err(ArtifactRouteError::entry_route_missing(
                contract.entry_route.clone(),
            ));
        }

        let aliases = aliases_for_routes(&routes, contract.canonical_policy);
        let manifest = Self {
            schema_version: ARTIFACT_ROUTE_MANIFEST_SCHEMA.to_string(),
            build_id,
            entry_route: contract.entry_route.clone(),
            canonical_policy: contract.canonical_policy,
            routes,
            aliases,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ArtifactRouteError> {
        if self.schema_version != ARTIFACT_ROUTE_MANIFEST_SCHEMA {
            return Err(ArtifactRouteError::invalid_manifest(format!(
                "unsupported artifact route manifest schema: {}",
                self.schema_version
            )));
        }
        if self.build_id.trim().is_empty() {
            return Err(ArtifactRouteError::invalid_manifest(
                "buildId must not be empty",
            ));
        }
        validate_route(&self.entry_route)?;
        let canonical_entry = canonicalize_route(&self.entry_route, self.canonical_policy);
        if canonical_entry != self.entry_route {
            return Err(ArtifactRouteError::invalid_manifest(format!(
                "entry route {} is not canonical for {:?}; expected {canonical_entry}",
                self.entry_route, self.canonical_policy
            )));
        }
        if self.routes.is_empty() {
            return Err(ArtifactRouteError::invalid_manifest(
                "routes must not be empty",
            ));
        }
        if !self.routes.contains_key(&self.entry_route) {
            return Err(ArtifactRouteError::entry_route_missing(
                self.entry_route.clone(),
            ));
        }
        for (route, target) in &self.routes {
            validate_route(route)?;
            let canonical = canonicalize_route(route, self.canonical_policy);
            if canonical != *route {
                return Err(ArtifactRouteError::invalid_manifest(format!(
                    "route {route} is not canonical for {:?}; expected {canonical}",
                    self.canonical_policy
                )));
            }
            validate_artifact_path(&target.file)?;
            validate_sha256(&target.sha256)?;
            if target.content_type != "text/html; charset=utf-8" {
                return Err(ArtifactRouteError::invalid_manifest(format!(
                    "route {route} has unsupported content type {}",
                    target.content_type
                )));
            }
        }
        let expected_aliases = aliases_for_routes(&self.routes, self.canonical_policy);
        if self.aliases != expected_aliases {
            return Err(ArtifactRouteError::invalid_manifest(
                "aliases do not match the canonical route policy",
            ));
        }
        for (alias, canonical) in &self.aliases {
            validate_route(alias)?;
            validate_route(canonical)?;
            if alias == canonical || !self.routes.contains_key(canonical) {
                return Err(ArtifactRouteError::invalid_manifest(format!(
                    "alias {alias} does not resolve to a distinct canonical route"
                )));
            }
            if self.routes.contains_key(alias) {
                return Err(ArtifactRouteError::invalid_manifest(format!(
                    "alias {alias} collides with a canonical route"
                )));
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, ArtifactRouteError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|error| {
            ArtifactRouteError::invalid_manifest(format!(
                "artifact route manifest serialization failed: {error}"
            ))
        })?;
        Ok(canonical_json_hash(&value))
    }

    pub fn resolve(&self, request_path: &str) -> Option<&ArtifactRouteTarget> {
        let canonical = self
            .aliases
            .get(request_path)
            .map(String::as_str)
            .unwrap_or(request_path);
        self.routes.get(canonical)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRouteError {
    pub error_kind: &'static str,
    pub message: String,
    pub route: Option<String>,
    pub files: Vec<String>,
}

impl ArtifactRouteError {
    fn invalid_contract(message: impl Into<String>) -> Self {
        Self {
            error_kind: "artifact.route_contract_invalid",
            message: message.into(),
            route: None,
            files: Vec::new(),
        }
    }

    fn invalid_manifest(message: impl Into<String>) -> Self {
        Self {
            error_kind: "artifact.route_manifest_invalid",
            message: message.into(),
            route: None,
            files: Vec::new(),
        }
    }

    fn ambiguous(route: String, mut files: Vec<String>) -> Self {
        files.sort();
        Self {
            error_kind: "artifact.route_ambiguous",
            message: format!(
                "multiple artifact files resolve to the same route {route}: {}",
                files.join(", ")
            ),
            route: Some(route),
            files,
        }
    }

    fn entry_route_missing(route: String) -> Self {
        Self {
            error_kind: "artifact.entry_route_missing",
            message: format!("artifact does not contain the contracted entry route {route}"),
            route: Some(route),
            files: Vec::new(),
        }
    }
}

impl fmt::Display for ArtifactRouteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ArtifactRouteError {}

fn route_for_html_file(path: &str) -> Option<String> {
    let normalized = path.trim_start_matches('/');
    if normalized == "index.html" {
        return Some("/".to_string());
    }
    if let Some(prefix) = normalized.strip_suffix("/index.html") {
        return Some(format!("/{}/", prefix.trim_matches('/')));
    }
    normalized
        .strip_suffix(".html")
        .map(|prefix| format!("/{}", prefix.trim_matches('/')))
}

pub(crate) fn equivalent_next_not_found_alias<'a>(
    left_path: &'a str,
    left_sha256: &str,
    right_path: &'a str,
    right_sha256: &str,
) -> Option<&'a str> {
    if left_sha256 != right_sha256
        || left_sha256.len() != 64
        || !left_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return None;
    }
    match (left_path, right_path) {
        ("404.html", "404/index.html") | ("404/index.html", "404.html") => Some("404.html"),
        _ => None,
    }
}

fn canonicalize_route(route: &str, policy: RoutePolicy) -> String {
    if route == "/" {
        return route.to_string();
    }
    match policy {
        RoutePolicy::Root => route.to_string(),
        RoutePolicy::TrailingSlash => format!("{}/", route.trim_end_matches('/')),
        RoutePolicy::CleanHtml => route.trim_end_matches('/').to_string(),
    }
}

fn aliases_for_routes(
    routes: &BTreeMap<String, ArtifactRouteTarget>,
    policy: RoutePolicy,
) -> BTreeMap<String, String> {
    routes
        .keys()
        .filter(|route| route.as_str() != "/")
        .filter_map(|canonical| {
            let alias = match policy {
                RoutePolicy::TrailingSlash => canonical.trim_end_matches('/').to_string(),
                RoutePolicy::CleanHtml => format!("{canonical}/"),
                RoutePolicy::Root => return None,
            };
            Some((alias, canonical.clone()))
        })
        .collect()
}

fn validate_route(route: &str) -> Result<(), ArtifactRouteError> {
    if !route.starts_with('/')
        || route.contains("//")
        || route.contains('\\')
        || route.contains('%')
        || route.contains('?')
        || route.contains('#')
        || route.contains('\0')
        || route.chars().any(char::is_whitespace)
        || route
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
    {
        return Err(ArtifactRouteError::invalid_contract(format!(
            "invalid artifact route: {route}"
        )));
    }
    Ok(())
}

fn validate_artifact_path(path: &str) -> Result<(), ArtifactRouteError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|segment| {
            segment.is_empty() || matches!(segment, "." | "..") || segment.contains('\0')
        })
    {
        return Err(ArtifactRouteError::invalid_manifest(format!(
            "invalid artifact file path: {path}"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), ArtifactRouteError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ArtifactRouteError::invalid_manifest(
            "artifact route file sha256 must be lowercase hexadecimal",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ConformanceCorpus {
        schema_version: String,
        cases: Vec<ConformanceCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ConformanceCase {
        id: String,
        contract: ArtifactRouteContract,
        files: Vec<ArtifactRouteFile>,
        #[serde(default)]
        expected_routes: BTreeMap<String, String>,
        #[serde(default)]
        expected_aliases: BTreeMap<String, String>,
        #[serde(default)]
        resolutions: Vec<ConformanceResolution>,
        #[serde(default)]
        expected_error_kind: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ConformanceResolution {
        request_path: String,
        expected_file: Option<String>,
    }

    fn file(path: &str, hash: char) -> ArtifactRouteFile {
        ArtifactRouteFile {
            path: path.to_string(),
            sha256: hash.to_string().repeat(64),
        }
    }

    #[test]
    fn docs_entry_and_alias_resolve_to_the_same_target() {
        let manifest = ArtifactRouteManifest::build(
            "build-1",
            &ArtifactRouteContract::docs(),
            [
                file("docs/index.html", 'a'),
                file("docs/guide/index.html", 'b'),
            ],
        )
        .unwrap();

        assert_eq!(manifest.entry_route, "/docs/");
        assert_eq!(manifest.aliases["/docs"], "/docs/");
        assert_eq!(
            manifest.resolve("/docs").unwrap(),
            manifest.resolve("/docs/").unwrap()
        );
        assert_eq!(manifest.resolve("/docs/").unwrap().file, "docs/index.html");
    }

    #[test]
    fn legacy_docs_html_maps_to_the_contracted_trailing_slash_route() {
        let manifest = ArtifactRouteManifest::build(
            "build-1",
            &ArtifactRouteContract::docs(),
            [file("docs.html", 'a')],
        )
        .unwrap();

        assert_eq!(manifest.resolve("/docs/").unwrap().file, "docs.html");
        assert_eq!(manifest.resolve("/docs").unwrap().file, "docs.html");
    }

    #[test]
    fn duplicate_clean_and_trailing_slash_files_are_ambiguous() {
        let error = ArtifactRouteManifest::build(
            "build-1",
            &ArtifactRouteContract::docs(),
            [file("docs.html", 'a'), file("docs/index.html", 'a')],
        )
        .unwrap_err();

        assert_eq!(error.error_kind, "artifact.route_ambiguous");
        assert_eq!(error.route.as_deref(), Some("/docs/"));
        assert_eq!(error.files, vec!["docs.html", "docs/index.html"]);
    }

    #[test]
    fn identical_next_export_not_found_aliases_use_the_stable_flat_file() {
        let manifest = ArtifactRouteManifest::build(
            "build-1",
            &ArtifactRouteContract::docs(),
            [
                file("docs/index.html", 'a'),
                file("404/index.html", 'b'),
                file("404.html", 'b'),
            ],
        )
        .unwrap();

        assert_eq!(manifest.routes["/404/"].file, "404.html");
        assert_eq!(manifest.aliases["/404"], "/404/");

        let error = ArtifactRouteManifest::build(
            "build-2",
            &ArtifactRouteContract::docs(),
            [
                file("docs/index.html", 'a'),
                file("404.html", 'b'),
                file("404/index.html", 'c'),
            ],
        )
        .unwrap_err();
        assert_eq!(error.error_kind, "artifact.route_ambiguous");
    }

    #[test]
    fn missing_entry_route_is_rejected() {
        let error = ArtifactRouteManifest::build(
            "build-1",
            &ArtifactRouteContract::docs(),
            [file("guide/index.html", 'a')],
        )
        .unwrap_err();

        assert_eq!(error.error_kind, "artifact.entry_route_missing");
    }

    #[test]
    fn digest_is_stable_across_input_order() {
        let left = ArtifactRouteManifest::build(
            "build-1",
            &ArtifactRouteContract::docs(),
            [
                file("docs/index.html", 'a'),
                file("docs/guide/index.html", 'b'),
            ],
        )
        .unwrap();
        let right = ArtifactRouteManifest::build(
            "build-1",
            &ArtifactRouteContract::docs(),
            [
                file("docs/guide/index.html", 'b'),
                file("docs/index.html", 'a'),
            ],
        )
        .unwrap();

        assert_eq!(left.digest().unwrap(), right.digest().unwrap());
    }

    #[test]
    fn manifest_validation_rejects_aliases_outside_the_frozen_policy() {
        let mut manifest = ArtifactRouteManifest::build(
            "build-1",
            &ArtifactRouteContract::docs(),
            [file("docs/index.html", 'a')],
        )
        .unwrap();
        manifest
            .aliases
            .insert("/unrelated".to_string(), "/docs/".to_string());

        let error = manifest.validate().unwrap_err();
        assert_eq!(error.error_kind, "artifact.route_manifest_invalid");
        assert!(error.message.contains("canonical route policy"));
    }

    #[test]
    fn manifest_validation_rejects_ambiguous_encoded_routes() {
        let mut manifest = ArtifactRouteManifest::build(
            "build-1",
            &ArtifactRouteContract::docs(),
            [file("docs/index.html", 'a')],
        )
        .unwrap();
        manifest
            .aliases
            .insert("/docs%2f".to_string(), "/docs/".to_string());

        assert!(manifest.validate().is_err());
    }

    #[test]
    fn shared_route_conformance_corpus_matches_the_rust_oracle() {
        let corpus: ConformanceCorpus = serde_json::from_str(include_str!(
            "../evidence/replay/contracts/artifact-route-conformance@1.json"
        ))
        .unwrap();
        assert_eq!(corpus.schema_version, "artifact-route-conformance@1");

        for case in corpus.cases {
            let result = ArtifactRouteManifest::build(&case.id, &case.contract, case.files);
            if let Some(expected_error_kind) = case.expected_error_kind {
                assert_eq!(
                    result.unwrap_err().error_kind,
                    expected_error_kind,
                    "conformance case {} returned a different error",
                    case.id
                );
                continue;
            }

            let manifest = result
                .unwrap_or_else(|error| panic!("conformance case {} failed: {error}", case.id));
            let actual_routes = manifest
                .routes
                .iter()
                .map(|(route, target)| (route.clone(), target.file.clone()))
                .collect::<BTreeMap<_, _>>();
            assert_eq!(actual_routes, case.expected_routes, "case {}", case.id);
            assert_eq!(manifest.aliases, case.expected_aliases, "case {}", case.id);
            for resolution in case.resolutions {
                assert_eq!(
                    manifest
                        .resolve(&resolution.request_path)
                        .map(|target| target.file.as_str()),
                    resolution.expected_file.as_deref(),
                    "case {} request {}",
                    case.id,
                    resolution.request_path
                );
            }
        }
    }
}
