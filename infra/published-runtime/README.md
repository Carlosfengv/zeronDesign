# Published runtime packaging infrastructure

This directory contains the trusted, template-independent runtime image inputs.
Template adapters produce a verified artifact manifest; they do not select the
runtime image or inject web-server configuration.

`static-web/` packages any artifact conforming to `artifact-manifest@1` and
`runtime-manifest@1` profile `static-web-v1`. Published files are copied to
`/srv/www`; immutable release metadata is copied to `/.anydesign` and is denied
by the web server. The container runs as UID/GID 101 and listens on port 8080.

All external images are executed by digest from `images.lock.json`. Updating a
source tag without resolving and reviewing its new multi-platform digest is not
an accepted upgrade.

The G4 local acceptance path uses the exact Syft, Trivy, and Cosign versions in
the same lock file. On macOS they can be installed with
`brew install syft trivy cosign`; the helper rejects a version mismatch. Release
CI must install artifacts from the listed upstream release and validate the
listed platform checksum before exposing the tool directory to Runtime. The
helper itself is independently SHA-256 pinned by
`ProcessReleasePackagingBackend` for every operation.

The local acceptance Registry is intentionally HTTP-only on loopback. Shared
or production registries must use TLS and workload identity; Registry or signing
credentials are never passed into an Agent Sandbox.
