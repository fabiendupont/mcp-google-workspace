#!/usr/bin/env python3
"""Compare the chart's rendering with the plain manifests in deploy/kubernetes/.

The chart and the kustomize manifests are two hand-maintained descriptions of
the same workload. Labels necessarily differ (Helm adds release-scoped and
chart labels, kustomize adds part-of/managed-by), so this compares the parts
that must not drift:

  * Deployment: replicas and the whole pod spec (containers, args, env, probes,
    resources, security context, volumes, mounts)
  * Service: type and ports
  * ConfigMap: the parsed policy JSON, with `_comment*` documentation keys
    removed (they are prose, not policy)
  * NetworkPolicy: policyTypes, ingress and egress rules
  * ServiceMonitor: endpoints (port, path, interval, authorization)

Rendering uses charts/mcp-google-workspace/ci/plain-values.yaml, which supplies
the all-zero digest placeholder and the Secret names the plain manifests use.

Usage: scripts/chart-drift.py [--helm PATH]
Exit status is 0 when the two renderings agree, 1 otherwise.
"""

from __future__ import annotations

import argparse
import difflib
import json
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import yaml

REPO_ROOT = Path(__file__).resolve().parent.parent
CHART_DIR = REPO_ROOT / "charts" / "mcp-google-workspace"
VALUES = CHART_DIR / "ci" / "plain-values.yaml"
MANIFEST_DIR = REPO_ROOT / "deploy" / "kubernetes"

# Release name and namespace the plain manifests hard-code. The chart derives
# resource names from the release name and every GWS_ALLOWED_HOSTS entry from
# the namespace, so the drift check has to use the same pair.
RELEASE_NAME = "mcp-google-workspace"
NAMESPACE = "mcp-google-workspace"

COMMENT_PREFIX = "_comment"


def load_documents(text: str) -> list[dict[str, Any]]:
    return [doc for doc in yaml.safe_load_all(text) if isinstance(doc, dict)]


def render_chart(helm: str) -> list[dict[str, Any]]:
    cmd = [
        helm,
        "template",
        RELEASE_NAME,
        str(CHART_DIR),
        "--namespace",
        NAMESPACE,
        "--values",
        str(VALUES),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        sys.stderr.write("helm template failed:\n" + result.stderr)
        sys.exit(1)
    return load_documents(result.stdout)


def load_plain_manifests() -> list[dict[str, Any]]:
    documents: list[dict[str, Any]] = []
    for path in sorted(MANIFEST_DIR.glob("*.yaml")):
        documents.extend(load_documents(path.read_text()))
    return documents


def index(documents: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    """Key each document by kind; the chart renders one object of each kind."""
    return {doc["kind"]: doc for doc in documents if "kind" in doc}


def strip_comments(value: Any) -> Any:
    if isinstance(value, dict):
        return {
            key: strip_comments(item)
            for key, item in value.items()
            if not str(key).startswith(COMMENT_PREFIX)
        }
    if isinstance(value, list):
        return [strip_comments(item) for item in value]
    return value


def projection(kind: str, doc: dict[str, Any]) -> Any:
    spec = doc.get("spec") or {}
    if kind == "Deployment":
        return {
            "replicas": spec.get("replicas"),
            "podSpec": (spec.get("template") or {}).get("spec"),
        }
    if kind == "Service":
        # An omitted type is ClusterIP, which is what the chart states.
        return {
            "type": spec.get("type", "ClusterIP"),
            "ports": spec.get("ports"),
        }
    if kind == "ConfigMap":
        raw = (doc.get("data") or {}).get("policy.json")
        if raw is None:
            return None
        return strip_comments(json.loads(raw))
    if kind == "NetworkPolicy":
        return {
            "policyTypes": spec.get("policyTypes"),
            "ingress": spec.get("ingress"),
            "egress": spec.get("egress"),
        }
    if kind == "ServiceMonitor":
        return {"endpoints": spec.get("endpoints")}
    raise AssertionError(f"no projection for kind {kind}")


def compare(kind: str, rendered: dict[str, Any], plain: dict[str, Any]) -> bool:
    left = projection(kind, rendered)
    right = projection(kind, plain)
    if left == right:
        print(f"ok   {kind}")
        return True

    print(f"FAIL {kind} differs between the chart and deploy/kubernetes/")
    left_text = json.dumps(left, indent=2, sort_keys=True, default=str)
    right_text = json.dumps(right, indent=2, sort_keys=True, default=str)
    diff = difflib.unified_diff(
        right_text.splitlines(),
        left_text.splitlines(),
        fromfile=f"deploy/kubernetes ({kind})",
        tofile=f"chart rendering ({kind})",
        lineterm="",
    )
    print("\n".join(diff))
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--helm",
        default=shutil.which("helm"),
        help="helm binary to use (default: first on PATH)",
    )
    args = parser.parse_args()
    if not args.helm:
        sys.stderr.write(
            "helm not found on PATH; install helm 3 or pass --helm /path/to/helm\n"
        )
        return 1

    chart_docs = index(render_chart(args.helm))
    plain_docs = index(load_plain_manifests())

    ok = True
    for kind in ("Deployment", "Service", "ConfigMap", "NetworkPolicy", "ServiceMonitor"):
        if kind not in chart_docs:
            print(f"FAIL chart did not render a {kind} (check ci/plain-values.yaml)")
            ok = False
            continue
        if kind not in plain_docs:
            print(f"FAIL deploy/kubernetes/ has no {kind}")
            ok = False
            continue
        ok = compare(kind, chart_docs[kind], plain_docs[kind]) and ok

    if not ok:
        print(
            "\nThe chart and the plain manifests disagree. Update both, or make the "
            "chart reproduce the manifest through ci/plain-values.yaml.",
            file=sys.stderr,
        )
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
