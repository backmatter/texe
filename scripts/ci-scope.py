#!/usr/bin/env python3
"""Reuse a successful source check only for a strictly version-only release PR."""
import json
import os
import re
import subprocess
import sys
import tomllib


def command(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.PIPE).strip()


def read(ref, path):
    return command("git", "show", f"{ref}:{path}")


def version(manifest):
    value = manifest["package"]["version"]
    return manifest["workspace"]["package"]["version"] if isinstance(value, dict) else value


def normalized(path, text, name, release, local_names):
    if path.endswith("CHANGELOG.md"):
        return None
    if path.endswith(".json"):
        data = json.loads(text)
        assert data.pop("version") == release
        if path.endswith("/package.json"):
            assert data.pop("texLsServerVersion") == release
        if path.endswith("package-lock.json"):
            assert data["packages"][""].pop("version") == release
        return data
    data = tomllib.loads(text)
    if path == "Cargo.lock":
        for package in data["package"]:
            if package["name"] in local_names and "source" not in package:
                assert package.pop("version") == release
    elif path == "Cargo.toml":
        if isinstance(data["package"]["version"], dict):
            data["workspace"]["package"].pop("version")
        else:
            data["package"].pop("version")
    else:
        dependency = data["dependencies"][name]
        assert "path" in dependency and dependency.pop("version") == release
    return data


def metadata_only(base, head):
    old = tomllib.loads(read(base, "Cargo.toml"))
    new = tomllib.loads(read(head, "Cargo.toml"))
    before, after = version(old), version(new)
    stable = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    if not all(re.fullmatch(stable, v) for v in [before, after]):
        return False
    if tuple(map(int, after.split("."))) <= tuple(map(int, before.split("."))):
        return False
    allowed = {"Cargo.toml", "Cargo.lock", "CHANGELOG.md", "xtask/Cargo.toml"}
    if old["package"]["name"] == "tex-ls":
        allowed.update({"editors/vscode/package.json", "editors/vscode/package-lock.json", "editors/vscode/CHANGELOG.md"})
    local_names = {old["package"]["name"]}
    if isinstance(old["package"]["version"], dict):
        for member in old["workspace"]["members"]:
            if member == ".":
                continue
            package = tomllib.loads(read(base, f"{member}/Cargo.toml"))["package"]
            if package.get("version") == {"workspace": True}:
                local_names.add(package["name"])
    paths = command("git", "diff", "--name-only", base, head).splitlines()
    return bool(paths) and set(paths) <= allowed and all(
        normalized(p, read(base, p), old["package"]["name"], before, local_names)
        == normalized(p, read(head, p), old["package"]["name"], after, local_names)
        for p in paths
    )


def api(path):
    return json.loads(command("gh", "api", path))


def successful_run(source, workflow, events):
    repo = os.environ["GITHUB_REPOSITORY"]
    runs = api(f"repos/{repo}/actions/workflows/{workflow}/runs?head_sha={source}&status=success&per_page=10")
    for run in runs["workflow_runs"]:
        if run["event"] not in events or run["head_repository"]["full_name"] != repo:
            continue
        jobs = api(f"repos/{repo}/actions/runs/{run['id']}/jobs?per_page=100")["jobs"]
        heavy = [j for j in jobs if j["name"] not in {"Validation scope", "pr-title"}]
        if heavy and all(j["conclusion"] == "success" for j in heavy):
            return run["html_url"]
    return None


def checked_base(base, workflow):
    base = command("git", "rev-parse", base)
    proof = successful_run(base, workflow, {"push", "workflow_dispatch"})
    if proof:
        return proof
    repo = os.environ["GITHUB_REPOSITORY"]
    for pr in api(f"repos/{repo}/commits/{base}/pulls"):
        if not pr.get("merged_at") or pr.get("merge_commit_sha") != base:
            continue
        source = pr["head"]["sha"]
        command("git", "fetch", "--no-tags", "origin", source)
        # A stale PR head or a merge containing other changes cannot provide proof.
        if command("git", "rev-parse", f"{source}^{{tree}}") != command("git", "rev-parse", f"{base}^{{tree}}"):
            continue
        proof = successful_run(source, workflow, {"pull_request", "workflow_dispatch"})
        if proof:
            return proof
    return None


def main():
    proof = None
    try:
        base = None
        head = os.environ.get("GITHUB_SHA")
        if os.environ.get("GITHUB_EVENT_NAME") == "pull_request":
            event = json.loads(open(os.environ["GITHUB_EVENT_PATH"]).read())
            base = event["pull_request"]["base"]["sha"]
        elif os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch" and os.environ.get("CI_RELEASE_PR"):
            number = os.environ["CI_RELEASE_PR"]
            if not number.isdigit():
                raise ValueError("Invalid release PR number")
            repo = os.environ["GITHUB_REPOSITORY"]
            pr = api(f"repos/{repo}/pulls/{number}")
            if pr["head"]["sha"] != head or pr["base"]["ref"] != "main":
                raise ValueError("The release check must run on the current PR head")
            base = pr["base"]["sha"]
        if base and metadata_only(base, head):
            proof = checked_base(base, sys.argv[1])
    except (AssertionError, KeyError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Full validation required: release equivalence could not be verified ({type(error).__name__}).")
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"full={'false' if proof else 'true'}\n")
    if proof:
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
            summary.write(f"Only release versions and changelogs changed. Reusing the full successful source validation: {proof}\n")
    else:
        print("Running the full platform validation.")


if __name__ == "__main__":
    main()
