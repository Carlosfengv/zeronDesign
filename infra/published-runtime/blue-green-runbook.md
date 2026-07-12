# Published Work Blue/Green Runbook

## Normal switch

1. Confirm the target WorkRelease and packaging record are `validated` and the image is digest pinned.
2. Confirm the green Deployment is Available and its isolated release probe returns the target release ID.
3. Record the stable Service UID, resourceVersion, and current release selector.
4. CAS-patch only the stable Service release selector.
5. Wait until every EndpointSlice endpoint targets a Pod labeled with the target release ID.
6. Verify the unchanged HTTPS host returns the target `X-AnyDesign-Release-Id` and release body.
7. Only then commit `currentReleaseId`, `previousReleaseId`, deployment identity, and operation completion.

The old Deployment remains running as the immediate rollback target. HTML responses are `no-store`; the switch does not claim a zero-duration mixed-endpoint window.

## Automatic selector restore

If EndpointSlice convergence or the external identity probe exceeds the bounded timeout:

1. CAS-patch the stable Service selector from green back to the persisted current blue release.
2. Wait until EndpointSlices contain only blue Pods.
3. Verify the public host again returns the blue release identity.
4. Keep Store `currentReleaseId` unchanged and mark the operation/runtime `reconcile_required` with the original switch error.

If selector restore or blue verification also fails, treat the work as a traffic-integrity incident. Do not update Store pointers and do not delete either Deployment.

## Restart recovery

- Service already selects desired green while Store still names blue: resume EndpointSlice and external verification, then commit Store.
- Service still selects persisted blue: retry the green CAS switch.
- Service selects neither persisted blue nor desired green, UID changed, or controlled fields drifted: fail closed as `reconcile_required`; do not adopt the resource.
- Ingress remains stable throughout Update/Rollback. Host or backend drift follows the G7 fail-closed recovery path.

## Registry GC

Before any registry deletion, obtain one protection snapshot containing:

- desired/current/previous/last-successful Runtime release IDs;
- nonterminal PublishOperation and ReleasePackagingRecord release IDs;
- every live Deployment release ID and image digest in the Published namespace.

If RuntimeStore, ReleaseStore, or the Kubernetes scan is unavailable, abort GC. Any protected release ID or digest blocks deletion. The current implementation remains conservative: validated releases and live retained Deployments are not made garbage-collectable automatically.

## Operator evidence

Capture the operation ID, desired generation, Service UID/resourceVersion and selector before/after, EndpointSlice target Pod labels, external release header/body, rollback result, and protected release/digest snapshot. Never include Registry credentials or TLS private key material.
