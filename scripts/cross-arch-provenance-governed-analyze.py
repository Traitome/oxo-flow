#!/usr/bin/env python3
"""Governed decision for the #1397 / #1166 cross-architecture campaign.

Implements the PR-06 (v1.0.2, amendment 2) section C4 ordered rule using exact
Clopper-Pearson two-sided 95% confidence intervals. Standard library only: the
exact beta quantile is computed by bisection over the regularized incomplete
beta function, so the decision can be reproduced without SciPy.

The corpus (workflows, artifact classes, negative controls, tau_ARCH, n_min)
is read from a frozen JSON manifest; this script never decides the corpus.

Usage:
  cross-arch-provenance-governed-analyze.py \
      --corpus scripts/cross-arch-provenance-governed-corpus.json \
      --arm-a <dir> --arm-b <dir> \
      --expected-commit <sha> --out <results.json>

Exit codes: 0 when a category was computed (including INVALID, which is a
governed category and is printed in the JSON), 1 on malformed input (the
analysis itself could not run).
"""

import argparse
import hashlib
import json
import math
import os
import re
import sys

MAX_BETA_ITERATIONS = 200
BETA_EPS = 3e-14
BETA_FPMIN = 1e-300
QUANTILE_EPS = 1e-14


def _betacf(a, b, x):
    """Continued fraction for the incomplete beta function (Numerical Recipes)."""
    qab = a + b
    qap = a + 1.0
    qam = a - 1.0
    c = 1.0
    d = 1.0 - qab * x / qap
    if abs(d) < BETA_FPMIN:
        d = BETA_FPMIN
    d = 1.0 / d
    h = d
    for m in range(1, MAX_BETA_ITERATIONS + 1):
        m2 = 2 * m
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1.0 + aa * d
        if abs(d) < BETA_FPMIN:
            d = BETA_FPMIN
        c = 1.0 + aa / c
        if abs(c) < BETA_FPMIN:
            c = BETA_FPMIN
        d = 1.0 / d
        h *= d * c
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1.0 + aa * d
        if abs(d) < BETA_FPMIN:
            d = BETA_FPMIN
        c = 1.0 + aa / c
        if abs(c) < BETA_FPMIN:
            c = BETA_FPMIN
        d = 1.0 / d
        delta = d * c
        h *= delta
        if abs(delta - 1.0) < BETA_EPS:
            break
    return h


def betainc(a, b, x):
    """Regularized incomplete beta I_x(a, b)."""
    if not (a > 0 and b > 0):
        raise ValueError("betainc requires a > 0 and b > 0")
    if x <= 0.0:
        return 0.0
    if x >= 1.0:
        return 1.0
    log_front = (
        math.lgamma(a + b)
        - math.lgamma(a)
        - math.lgamma(b)
        + a * math.log(x)
        + b * math.log1p(-x)
    )
    front = math.exp(log_front)
    if x < (a + 1.0) / (a + b + 2.0):
        return front * _betacf(a, b, x) / a
    return 1.0 - front * _betacf(b, a, 1.0 - x) / b


def beta_ppf(p, a, b):
    """Quantile of the Beta(a, b) distribution by bisection."""
    if not (0.0 <= p <= 1.0):
        raise ValueError("beta_ppf requires 0 <= p <= 1")
    if p == 0.0:
        return 0.0
    if p == 1.0:
        return 1.0
    lo, hi = 0.0, 1.0
    for _ in range(MAX_BETA_ITERATIONS):
        mid = (lo + hi) / 2.0
        if betainc(a, b, mid) < p:
            lo = mid
        else:
            hi = mid
        if hi - lo < QUANTILE_EPS:
            break
    return (lo + hi) / 2.0


def clopper_pearson(k, n, alpha=0.05):
    """Exact two-sided Clopper-Pearson interval for k successes in n trials."""
    if n <= 0 or not (0 <= k <= n):
        raise ValueError("clopper_pearson requires n > 0 and 0 <= k <= n")
    lower = 0.0 if k == 0 else beta_ppf(alpha / 2.0, k, n - k + 1)
    upper = 1.0 if k == n else beta_ppf(1.0 - alpha / 2.0, k + 1, n - k)
    return lower, upper


def interval_partition(k, n, tau, alpha=0.05):
    """PR-06 section 0.1 step 4 for a higher-is-better endpoint."""
    lower, upper = clopper_pearson(k, n, alpha)
    if lower >= tau:
        category = "POSITIVE"
    elif upper < tau:
        category = "NEGATIVE"
    else:
        category = "INCONCLUSIVE"
    return category, lower, upper


def govern(k, n, class_rows, tau, n_min, invalid_reasons):
    """Apply the PR-06 section C4 ordered rule.

    Returns (category, rule_step, interval_category, lower, upper).
    """
    if invalid_reasons:
        return "INVALID", "step 1: validity condition failed", None, None, None
    eligible_differing = [
        row for row in class_rows if row["eligible"] and row["identical"] < row["total"]
    ]
    interval_category, lower, upper = interval_partition(k, n, tau)
    if eligible_differing:
        return (
            "NEGATIVE",
            "step 2: eligible class contains a differing artifact",
            interval_category,
            lower,
            upper,
        )
    return (
        interval_category,
        "step 3: interval partition",
        interval_category,
        lower,
        upper,
    )


def read_kv(path):
    out = {}
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = re.match(r"^([a-z0-9._-]+):\s*(.*)$", line.rstrip("\n"))
            if match:
                out[match.group(1)] = match.group(2)
    return out


def read_sha256sum(path):
    out = {}
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            parts = line.split()
            if len(parts) != 2:
                continue
            out[parts[1]] = parts[0]
    return out


def sha256_file(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def load_corpus(path):
    with open(path, encoding="utf-8") as handle:
        corpus = json.load(handle)
    if corpus.get("format") != "oxo-flow.crossarch-corpus/v1":
        raise SystemExit("unsupported corpus format: %r" % corpus.get("format"))
    classes = corpus.get("classes", {})
    controls = corpus.get("negative_controls", [])
    analysis = corpus.get("analysis", {})
    tau = float(analysis["tau_ARCH"])
    n_min = int(analysis["n_min"])
    alpha = float(analysis.get("alpha", 0.05))
    all_primary = [key for keys in classes.values() for key in keys]
    if len(all_primary) != len(set(all_primary)):
        raise SystemExit("corpus lists a primary artifact more than once")
    if set(all_primary) & set(controls):
        raise SystemExit("corpus lists an artifact as both primary and negative control")
    declared_count = corpus.get("primary_artifact_count")
    if declared_count is not None and int(declared_count) != len(all_primary):
        raise SystemExit(
            "corpus primary_artifact_count=%s but %d primary artifacts are listed"
            % (declared_count, len(all_primary))
        )
    stems = {wf["stem"] for wf in corpus["workflows"]}
    for key in all_primary + controls:
        stem = key.split("/", 1)[0]
        if stem not in stems:
            raise SystemExit("artifact %r does not start with a known workflow stem" % key)
    return corpus, tau, n_min, alpha, all_primary, controls


def load_workflow(arm_dir, stem):
    """Load one workflow's raw evidence from an arm directory.

    A missing or malformed evidence file is recorded as an arm load error and
    surfaces as governable INVALID, never as a stable artifact.
    """
    checkpoint_path = os.path.join(arm_dir, "%s.checkpoint.json" % stem)
    verify_path = os.path.join(arm_dir, "%s.verify.json" % stem)
    independent_path = os.path.join(arm_dir, "%s.artifacts.sha256" % stem)
    errors = []
    checkpoint = {}
    verify = {}
    independent = {}
    recorded = {}
    try:
        with open(checkpoint_path, encoding="utf-8") as handle:
            checkpoint = json.load(handle)
        for path, digest in checkpoint.get("checksums", {}).items():
            if not digest.startswith("sha256:"):
                raise ValueError("checkpoint checksum is not sha256: %r" % digest)
            recorded["%s/%s" % (stem, path.lstrip("./"))] = digest[len("sha256:") :]
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        errors.append("%s/%s checkpoint: %s" % (os.path.basename(arm_dir), stem, exc))
    try:
        with open(verify_path, encoding="utf-8") as handle:
            verify = json.load(handle)["verify"]
    except (OSError, KeyError, ValueError, json.JSONDecodeError) as exc:
        errors.append("%s/%s verify: %s" % (os.path.basename(arm_dir), stem, exc))
    try:
        independent = read_sha256sum(independent_path)
    except OSError as exc:
        errors.append("%s/%s independent hashes: %s" % (os.path.basename(arm_dir), stem, exc))
    return {
        "checkpoint": checkpoint,
        "verify": verify,
        "independent": independent,
        "recorded": recorded,
        "errors": errors,
        "checkpoint_path": checkpoint_path,
        "verify_path": verify_path,
        "independent_path": independent_path,
    }


def load_arm(arm_dir, corpus):
    arm = {
        "dir": os.path.abspath(arm_dir),
        "identity": {},
        "container": {},
        "workflows": {},
        "load_errors": [],
    }
    for filename, target in (("identity.txt", "identity"), ("container.txt", "container")):
        try:
            arm[target] = read_kv(os.path.join(arm_dir, filename))
        except OSError as exc:
            arm["load_errors"].append(
                "%s/%s: %s" % (os.path.basename(arm_dir), filename, exc)
            )
    for wf in corpus["workflows"]:
        stem = wf["stem"]
        loaded = load_workflow(arm_dir, stem)
        arm["workflows"][stem] = loaded
        arm["load_errors"].extend(loaded["errors"])
    arm["recorded"] = {}
    for stem, wf in arm["workflows"].items():
        arm["recorded"].update(wf["recorded"])
    return arm


def add_invalid(reasons, code, detail):
    reasons.append({"code": code, "detail": detail})


def check_validity(corpus, arm_a, arm_b, expected_commit):
    reasons = []
    for name, arm in (("arm_a", arm_a), ("arm_b", arm_b)):
        for error in arm["load_errors"]:
            add_invalid(reasons, "ARM_EVIDENCE_MISSING", error)
    arms = {"arm_a": arm_a, "arm_b": arm_b}
    digest = corpus["image"]["manifest_list_digest"]

    # Container identity: ONE shared manifest-list digest; identical in-container
    # tool versions; the container architecture is expected to DIFFER.
    for name, arm in arms.items():
        container = arm["container"]
        if container.get("manifest-list-digest") != digest:
            add_invalid(
                reasons,
                "ENV_LOCK_DIGEST_MISMATCH",
                "%s manifest-list-digest=%r, frozen=%r"
                % (name, container.get("manifest-list-digest"), digest),
            )
        if arm["identity"].get("platform") != container.get("platform"):
            add_invalid(
                reasons,
                "ENV_LOCK_PLATFORM_MISMATCH",
                "%s identity platform=%r, container platform=%r"
                % (name, arm["identity"].get("platform"), container.get("platform")),
            )
        if arm["identity"].get("engine-commit") != expected_commit:
            add_invalid(
                reasons,
                "ENGINE_COMMIT_MISMATCH",
                "%s identity engine-commit=%r, expected=%r"
                % (name, arm["identity"].get("engine-commit"), expected_commit),
            )
    a_tools = {k: arm_a["container"].get(k) for k in ("python3", "sort", "gzip", "sha256sum", "libc")}
    b_tools = {k: arm_b["container"].get(k) for k in ("python3", "sort", "gzip", "sha256sum", "libc")}
    if a_tools != b_tools:
        differing = {k: [a_tools.get(k), b_tools.get(k)] for k in a_tools if a_tools.get(k) != b_tools.get(k)}
        add_invalid(reasons, "ENV_LOCK_TOOL_MISMATCH", json.dumps(differing, sort_keys=True))
    if a_tools.get("python3") in (None, "") or a_tools.get("sort") in (None, ""):
        add_invalid(reasons, "ENV_LOCK_TOOL_MISSING", "in-container tool versions not recorded")

    a_platform = arm_a["container"].get("platform")
    b_platform = arm_b["container"].get("platform")
    if {a_platform, b_platform} != {"linux/amd64", "linux/arm64"}:
        add_invalid(
            reasons,
            "ENV_LOCK_PLATFORM_MISMATCH",
            "expected linux/amd64 and linux/arm64, got %r and %r" % (a_platform, b_platform),
        )
    archs = {arm_a["container"].get("container-arch"), arm_b["container"].get("container-arch")}
    if archs != {"x86_64", "aarch64"}:
        add_invalid(
            reasons,
            "NATIVE_ARM_NOT_ACHIEVED",
            "container architectures are %r; expected native x86_64 and aarch64"
            % sorted(str(arch) for arch in archs),
        )

    # Engine commit and clean workflow bytes per workflow and per arm.
    for stem in arm_a["workflows"]:
        wf_a = arm_a["workflows"][stem]
        wf_b = arm_b["workflows"].get(stem)
        if wf_b is None:
            add_invalid(reasons, "ARM_EVIDENCE_MISSING", "arm_b has no evidence for %s" % stem)
            continue
        for name, wf in (("arm_a", wf_a), ("arm_b", wf_b)):
            checkpoint = wf["checkpoint"]
            if checkpoint.get("workflow_git_sha") != expected_commit:
                add_invalid(
                    reasons,
                    "ENGINE_COMMIT_MISMATCH",
                    "%s/%s workflow_git_sha=%r, expected=%r"
                    % (name, stem, checkpoint.get("workflow_git_sha"), expected_commit),
                )
            if checkpoint.get("workflow_git_dirty") is not False:
                add_invalid(
                    reasons,
                    "WORKFLOW_TREE_DIRTY",
                    "%s/%s workflow_git_dirty=%r (must be false)"
                    % (name, stem, checkpoint.get("workflow_git_dirty")),
                )
            if not checkpoint.get("workflow_content_hash"):
                add_invalid(
                    reasons,
                    "WORKFLOW_CONTENT_MISSING",
                    "%s/%s has no workflow_content_hash" % (name, stem),
                )
        if wf_a["checkpoint"].get("workflow_content_hash") != wf_b["checkpoint"].get("workflow_content_hash"):
            add_invalid(
                reasons,
                "WORKFLOW_CONTENT_MISMATCH",
                "%s workflow_content_hash differs across arms" % stem,
            )
        for name, wf in (("arm_a", wf_a), ("arm_b", wf_b)):
            if wf["checkpoint"].get("failed_rules"):
                add_invalid(
                    reasons,
                    "RULE_FAILED",
                    "%s/%s failed_rules=%r"
                    % (name, stem, sorted(wf["checkpoint"]["failed_rules"])),
                )
            verify = wf["verify"]
            if verify.get("mismatched") != 0 or verify.get("missing") != 0:
                add_invalid(
                    reasons,
                    "PROVENANCE_VERIFY_MISMATCH",
                    "%s/%s verify mismatched=%r missing=%r"
                    % (name, stem, verify.get("mismatched"), verify.get("missing")),
                )
            bad_entries = [e for e in verify.get("entries", []) if e.get("status") != "matched"]
            if bad_entries:
                add_invalid(
                    reasons,
                    "PROVENANCE_VERIFY_MISMATCH",
                    "%s/%s has %d non-matched verify entries"
                    % (name, stem, len(bad_entries)),
                )
            if wf["recorded"] != wf["independent"]:
                only_recorded = sorted(set(wf["recorded"]) - set(wf["independent"]))
                only_independent = sorted(set(wf["independent"]) - set(wf["recorded"]))
                changed = sorted(
                    key
                    for key in set(wf["recorded"]) & set(wf["independent"])
                    if wf["recorded"][key] != wf["independent"][key]
                )
                add_invalid(
                    reasons,
                    "CHECKPOINT_INDEPENDENT_HASH_MISMATCH",
                    "%s/%s recorded-vs-independent differs: only_recorded=%r only_independent=%r changed=%r"
                    % (name, stem, only_recorded, only_independent, changed),
                )

    # Corpus completeness: every primary artifact and negative control in both
    # arms; no undeclared artifact. Missingness limit is 0% (PR-06 amendment 2).
    primary = set()
    for keys in corpus["classes"].values():
        primary.update(keys)
    controls = set(corpus["negative_controls"])
    for name, arm in arms.items():
        present = set(arm["recorded"])
        for key in sorted(primary - present):
            add_invalid(reasons, "MISSING_ARTIFACT", "%s missing primary artifact %s" % (name, key))
        for key in sorted(controls - present):
            add_invalid(reasons, "NEGATIVE_CONTROL_MISSING", "%s missing negative control %s" % (name, key))
        for key in sorted(present - primary - controls):
            add_invalid(reasons, "UNEXPECTED_ARTIFACT", "%s produced undeclared artifact %s" % (name, key))

    # Negative control must DIFFER; identical means the harness cannot detect a
    # known architecture-dependent output, so the measurement is invalid.
    for key in sorted(controls):
        ha = arm_a["recorded"].get(key)
        hb = arm_b["recorded"].get(key)
        if ha is not None and hb is not None and ha == hb:
            add_invalid(
                reasons,
                "NEGATIVE_CONTROL_IDENTICAL",
                "negative control %s has the same SHA-256 on both arms (%s); the harness cannot detect a known difference"
                % (key, ha),
            )
    return reasons


def build_result(corpus, arm_a, arm_b, expected_commit, corpus_path):
    tau = float(corpus["analysis"]["tau_ARCH"])
    n_min = int(corpus["analysis"]["n_min"])
    alpha = float(corpus["analysis"].get("alpha", 0.05))
    reasons = check_validity(corpus, arm_a, arm_b, expected_commit)

    artifacts = []
    classes = {}
    for class_name, keys in corpus["classes"].items():
        rows = []
        for key in keys:
            ha = arm_a["recorded"].get(key)
            hb = arm_b["recorded"].get(key)
            identical = ha is not None and ha == hb
            row = {
                "artifact": key,
                "class": class_name,
                "arm_a_sha256": ha,
                "arm_b_sha256": hb,
                "identical": identical,
            }
            artifacts.append(row)
            rows.append(row)
        total = len(rows)
        identical_count = sum(1 for row in rows if row["identical"])
        eligible = total >= n_min
        classes[class_name] = {
            "total": total,
            "identical": identical_count,
            "eligible": eligible,
            "architecture_stable": (identical_count == total) if eligible else None,
            "artifacts": rows,
        }

    primary_total = len(artifacts)
    primary_identical = sum(1 for row in artifacts if row["identical"])
    if primary_total == 0:
        raise SystemExit("corpus has no primary artifacts")
    class_rows = list(classes.values())
    category, rule_step, interval_category, lower, upper = govern(
        primary_identical, primary_total, class_rows, tau, n_min, reasons
    )
    differing = [row["artifact"] for row in artifacts if not row["identical"]]
    unstable_classes = [
        name
        for name, row in classes.items()
        if row["architecture_stable"] is False
    ]
    control_rows = []
    for key in corpus["negative_controls"]:
        ha = arm_a["recorded"].get(key)
        hb = arm_b["recorded"].get(key)
        control_rows.append(
            {
                "artifact": key,
                "arm_a_sha256": ha,
                "arm_b_sha256": hb,
                "differs": ha is not None and hb is not None and ha != hb,
            }
        )

    proportion = primary_identical / primary_total
    interval = {
        "method": "Clopper-Pearson exact two-sided 95%",
        "alpha": alpha,
        "lower": lower,
        "upper": upper,
        "tau_ARCH": tau,
        "point_estimate": proportion,
        "interval_category": interval_category,
    }
    statement = build_statement(
        category,
        corpus,
        primary_total,
        primary_identical,
        proportion,
        lower,
        upper,
        differing,
        unstable_classes,
        classes,
        reasons,
    )

    return {
        "issue": "1397",
        "campaign": "1166",
        "corpus_path": corpus_path,
        "corpus_sha256": sha256_file(corpus_path),
        "expected_engine_commit": expected_commit,
        "arms": {
            "arm_a": {
                "label": "local",
                "dir": arm_a["dir"],
                "identity": arm_a["identity"],
                "container": arm_a["container"],
                "workflows": {
                    stem: {
                        "workflow_git_sha": wf["checkpoint"].get("workflow_git_sha"),
                        "workflow_content_hash": wf["checkpoint"].get("workflow_content_hash"),
                        "workflow_git_dirty": wf["checkpoint"].get("workflow_git_dirty"),
                        "completed_rules": sorted(wf["checkpoint"].get("completed_rules", [])),
                        "failed_rules": sorted(wf["checkpoint"].get("failed_rules", [])),
                        "verify": {
                            k: wf["verify"].get(k)
                            for k in ("matched", "mismatched", "missing")
                        },
                    }
                    for stem, wf in arm_a["workflows"].items()
                },
            },
            "arm_b": {
                "label": "remote",
                "dir": arm_b["dir"],
                "identity": arm_b["identity"],
                "container": arm_b["container"],
                "workflows": {
                    stem: {
                        "workflow_git_sha": wf["checkpoint"].get("workflow_git_sha"),
                        "workflow_content_hash": wf["checkpoint"].get("workflow_content_hash"),
                        "workflow_git_dirty": wf["checkpoint"].get("workflow_git_dirty"),
                        "completed_rules": sorted(wf["checkpoint"].get("completed_rules", [])),
                        "failed_rules": sorted(wf["checkpoint"].get("failed_rules", [])),
                        "verify": {
                            k: wf["verify"].get(k)
                            for k in ("matched", "mismatched", "missing")
                        },
                    }
                    for stem, wf in arm_b["workflows"].items()
                },
            },
        },
        "validity": {"valid": not reasons, "reasons": reasons},
        "primary_endpoint": {
            "name": "architecture-stable artifact proportion",
            "direction": "higher is better",
            "total": primary_total,
            "identical": primary_identical,
            "differing": len(differing),
            "proportion": proportion,
            "differing_set": differing,
        },
        "interval": interval,
        "classes": classes,
        "negative_control": control_rows,
        "flow_table": {
            "included_primary_artifacts": primary_total,
            "excluded_negative_controls": len(control_rows),
            "missing_primary_artifacts": sorted(
                key
                for key in set(k for keys in corpus["classes"].values() for k in keys)
                if arm_a["recorded"].get(key) is None or arm_b["recorded"].get(key) is None
            ),
        },
        "decision": {
            "category": category,
            "rule_step": rule_step,
            "negative_precedence": bool(unstable_classes),
            "unstable_classes": unstable_classes,
            "detail": "PR-06 section C4 ordered rule, frozen amendment 2",
        },
        "statement": statement,
        "artifacts": artifacts,
    }


def build_statement(
    category,
    corpus,
    n,
    k,
    p,
    lower,
    upper,
    differing,
    unstable_classes,
    classes,
    reasons,
):
    digest = corpus["image"]["manifest_list_digest"]
    n_workflows = len(corpus["workflows"])
    n_classes = len(classes)
    if category == "INVALID":
        codes = ", ".join(sorted({reason["code"] for reason in reasons}))
        return (
            "The campaign is INVALID under the pre-registered #1166 validity "
            "conditions (%s). No endpoint estimate is presented as the campaign "
            "result; see the validity reasons and flow table." % codes
        )
    pct = 100.0 * p
    lo = 100.0 * lower
    hi = 100.0 * upper
    if category == "POSITIVE":
        return (
            "On the frozen %d-workflow corpus of %d declared artifacts with "
            "identical container digest %s and matching environment locks, all "
            "%d artifacts were SHA-256 identical across native x86_64 and native "
            "aarch64 (%.1f%%, exact 95%% CI [%.2f%%, %.2f%%]), meeting the "
            "pre-registered tau_ARCH = %.2f. The claim is limited to these %d "
            "workflows, these tool versions, and these %d artifact classes; "
            "it does not establish bitwise reproducibility for arbitrary tools "
            "or workflows."
            % (n_workflows, n, digest, k, pct, lo, hi,
               corpus["analysis"]["tau_ARCH"], n_workflows, n_classes)
        )
    if category == "NEGATIVE":
        unstable = ", ".join(unstable_classes) if unstable_classes else "(none; interval rule)"
        return (
            "On the frozen %d-workflow corpus of %d declared artifacts on "
            "x86_64 and aarch64 with identical container digest %s, %d artifacts "
            "were bit-identical (%.1f%%, exact 95%% CI [%.2f%%, %.2f%%]). "
            "Classes %s are not architecture-stable; the differing set is %s. "
            "Cross-architecture reproducibility is therefore limited to the "
            "stable classes listed, and no general reproducibility claim is "
            "supported." % (n_workflows, n, digest, k, pct, lo, hi, unstable, differing)
        )
    return (
        "On the frozen %d-workflow corpus of %d declared artifacts on x86_64 "
        "and aarch64 with identical container digest %s, %d artifacts were "
        "bit-identical (%.1f%%, exact 95%% CI [%.2f%%, %.2f%%]). The interval "
        "contains the pre-registered tau_ARCH = %.2f, so no capability claim is "
        "supported (INCONCLUSIVE). Differing set: %s."
        % (n_workflows, n, digest, k, pct, lo, hi,
           corpus["analysis"]["tau_ARCH"], differing)
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", required=True)
    parser.add_argument("--arm-a", required=True)
    parser.add_argument("--arm-b", required=True)
    parser.add_argument("--expected-commit", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)

    corpus, _tau, _n_min, _alpha, _primary, _controls = load_corpus(args.corpus)
    try:
        arm_a = load_arm(args.arm_a, corpus)
        arm_b = load_arm(args.arm_b, corpus)
        result = build_result(corpus, arm_a, arm_b, args.expected_commit, args.corpus)
    except (KeyError, OSError, ValueError, json.JSONDecodeError) as exc:
        raise SystemExit("analysis input is malformed: %s" % exc)

    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(result, handle, indent=2, sort_keys=True)
        handle.write("\n")
    print("category: %s" % result["decision"]["category"])
    print("primary: %d/%d identical" % (
        result["primary_endpoint"]["identical"], result["primary_endpoint"]["total"]))
    if result["interval"]["lower"] is not None:
        print("exact 95%% CI: [%.6f, %.6f]" % (
            result["interval"]["lower"], result["interval"]["upper"]))
    if result["validity"]["reasons"]:
        for reason in result["validity"]["reasons"]:
            print("INVALID %s: %s" % (reason["code"], reason["detail"]))
    print(result["statement"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
