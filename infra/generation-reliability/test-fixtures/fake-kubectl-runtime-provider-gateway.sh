#!/usr/bin/env bash
set -Eeuo pipefail

args=" $* "
if [[ "${args}" == *" patch deployment "* ]]; then
  touch "${FAKE_KUBECTL_STATE:?}"
  exit 0
fi
if [[ "${args}" == *" get deployment "* && "${args}" == *" -o json "* ]]; then
  gateway_url="http://fixture-model-gateway.anydesign-runtime.svc.cluster.local:9000"
  auth_env=""
  generation=4
  if [[ -e "${FAKE_KUBECTL_STATE:?}" ]]; then
    gateway_url="http://provider-gateway.provider-system.svc.cluster.local:9000"
    auth_env=',{"name":"MODEL_GATEWAY_AUTH_TOKEN","valueFrom":{"secretKeyRef":{"name":"provider-gateway-runtime-auth","key":"MODEL_GATEWAY_AUTH_TOKEN"}}}'
    generation=5
  fi
  if [[ "${FAKE_KUBECTL_BAD_AFTER:-0}" == "1" && -e "${FAKE_KUBECTL_STATE}" ]]; then
    gateway_url="http://fixture-model-gateway.anydesign-runtime.svc.cluster.local:9000"
  fi
  printf '{"metadata":{"generation":%s},"spec":{"template":{"spec":{"containers":[{"name":"runtime","env":[{"name":"MODEL_PROVIDER","value":"internal_gateway"},{"name":"MODEL_GATEWAY_URL","value":"%s"}%s]}]}}},"status":{"observedGeneration":%s}}\n' \
    "${generation}" "${gateway_url}" "${auth_env}" "${generation}"
  exit 0
fi
if [[ "${args}" == *" get pods "* && "${args}" == *" -o json "* ]]; then
  printf '%s\n' '{"items":[{"metadata":{"name":"runtime-ready","uid":"pod-uid"},"status":{"phase":"Running","conditions":[{"type":"Ready","status":"True"}]}}]}'
  exit 0
fi
exit 0
