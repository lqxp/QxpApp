#!/usr/bin/env python3
import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import NamedTuple, NoReturn

ROOT = Path(__file__).resolve().parent.parent
PACKAGE_JSON = ROOT / "package.json"
CLIENT_DIR = ROOT / "client"
CLIENT_PACKAGE_JSON = CLIENT_DIR / "package.json"
CARGO_TOML = ROOT / "src-tauri" / "Cargo.toml"
CARGO_LOCK = ROOT / "src-tauri" / "Cargo.lock"
TAURI_CONF = ROOT / "src-tauri" / "tauri.conf.json"
QXCHAT_NIX = ROOT / "nix" / "qxchat.nix"

# Sibling umbrella project: ../lqxp, whose "web" folder is a submodule
# pointing at this repo. After a release, we pin it to the new tag.
LQXP_SIBLING_DIR = ROOT.parent / "lqxp"
LQXP_WEB_SUBMODULE = LQXP_SIBLING_DIR / "web"

SEMVER_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")


class Color:
    RED = "\033[31m"
    GREEN = "\033[32m"
    BLUE = "\033[34m"
    YELLOW = "\033[33m"
    BOLD = "\033[1m"
    RESET = "\033[0m"


def paint(text: str, color: str) -> str:
    return f"{color}{text}{Color.RESET}"


def die(message: str) -> NoReturn:
    print(paint(f"Error: {message}", Color.RED), file=sys.stderr)
    sys.exit(1)


def info(message: str) -> None:
    print(paint(message, Color.BLUE))


def note(message: str) -> None:
    print(paint(message, Color.YELLOW))


def warn(message: str) -> None:
    print(paint(f"Warning: {message}", Color.YELLOW))


def success(message: str) -> None:
    print(paint(message, Color.GREEN))


def bullet(message: str) -> None:
    print(paint(f"  - {message}", Color.YELLOW))


class Version(NamedTuple):
    major: int
    minor: int
    patch: int

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"

    def bump(self, kind: str) -> "Version":
        if kind == "major":
            return Version(self.major + 1, 0, 0)
        if kind == "minor":
            return Version(self.major, self.minor + 1, 0)
        if kind == "patch":
            return Version(self.major, self.minor, self.patch + 1)
        die(f"Unknown bump type: {kind}")


def parse_semver(version: str) -> Version:
    match = SEMVER_RE.match(version)
    if not match:
        die(f"Invalid version '{version}'. Expected format: MAJOR.MINOR.PATCH (e.g. 1.2.3)")
    return Version(*map(int, match.groups()))


def bump_version(current: str, kind: str) -> str:
    return str(parse_semver(current).bump(kind))


# --- git helpers -------------------------------------------------------

def git_probe(cmd: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    """Run git without raising on a non-zero exit; caller inspects the result."""
    try:
        return subprocess.run(cmd, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    except FileNotFoundError:
        die("git is required")


def run(cmd: list[str], cwd: Path = ROOT, *, capture: bool = False) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            cmd,
            cwd=cwd,
            check=True,
            text=True,
            stdout=subprocess.PIPE if capture else None,
            stderr=subprocess.PIPE if capture else None,
        )
    except FileNotFoundError:
        die("git is required")
    except subprocess.CalledProcessError as exc:
        die(exc.stderr.strip() if capture and exc.stderr else f"{' '.join(cmd)} failed")


def run_git(args: list[str], cwd: Path = ROOT, capture: bool = False) -> str:
    result = run(["git", *args], cwd=cwd, capture=capture)
    return result.stdout.strip() if capture and result.stdout else ""


def is_git_repo(path: Path) -> bool:
    return path.is_dir() and git_probe(["git", "rev-parse", "--is-inside-work-tree"], path).returncode == 0


def tag_exists(cwd: Path, tag_name: str) -> bool:
    return git_probe(["git", "rev-parse", "--verify", f"refs/tags/{tag_name}"], cwd).returncode == 0


def has_changes(cwd: Path, paths: list[str]) -> bool:
    return bool(run_git(["status", "--short", "--", *paths], cwd=cwd, capture=True))


def commit_changes(cwd: Path, paths: list[str], message: str) -> bool:
    if not has_changes(cwd, paths):
        return False
    run_git(["add", "--", *paths], cwd=cwd)
    run_git(["commit", "-m", message], cwd=cwd)
    return True


def ensure_up_to_date(cwd: Path, push: bool) -> None:
    """Abort early if the remote has commits we don't have locally yet.

    Without this check, a release can be committed and tagged locally, then
    fail to push because the remote moved on — leaving the repo half-released
    (local tag exists, nothing pushed).
    """
    if not push:
        return

    if git_probe(["git", "fetch"], cwd).returncode != 0:
        die(f"Unable to fetch from remote in {cwd}")

    result = git_probe(["git", "rev-list", "--left-right", "--count", "HEAD...@{u}"], cwd)
    if result.returncode != 0:
        return  # No upstream configured for the current branch.

    try:
        _ahead, behind = result.stdout.split()
    except ValueError:
        return

    if int(behind) > 0:
        die(f"{cwd} is behind its remote by {behind} commit(s). Run 'git pull' in {cwd} before releasing.")


def create_version_tag(cwd: Path, version: str, push: bool) -> None:
    tag_name = f"v{version}"
    if tag_exists(cwd, tag_name):
        die(f"Tag {tag_name} already exists in {cwd}")

    run_git(["tag", tag_name], cwd=cwd)

    if push:
        run_git(["push", "origin", "HEAD"], cwd=cwd)
        run_git(["push", "origin", tag_name], cwd=cwd)


# --- version file writers -----------------------------------------------

def load_package_version() -> str:
    try:
        data = json.loads(PACKAGE_JSON.read_text(encoding="utf-8"))
    except FileNotFoundError:
        die(f"File not found: {PACKAGE_JSON}")
    except json.JSONDecodeError as exc:
        die(f"Invalid JSON in {PACKAGE_JSON}: {exc}")

    version = data.get("version")
    if not isinstance(version, str):
        die("Missing or invalid 'version' field in package.json")

    parse_semver(version)
    return version


def write_json_version(path: Path, version: str) -> None:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        die(f"File not found: {path}")
    except json.JSONDecodeError as exc:
        die(f"Invalid JSON in {path}: {exc}")

    data["version"] = version
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def write_package_version(version: str) -> None:
    write_json_version(PACKAGE_JSON, version)


def write_client_package_version(version: str) -> None:
    if not CLIENT_PACKAGE_JSON.exists():
        warn(f"{CLIENT_PACKAGE_JSON} not found, skipping")
        return
    write_json_version(CLIENT_PACKAGE_JSON, version)


def write_tauri_conf_version(version: str) -> None:
    write_json_version(TAURI_CONF, version)


def get_cargo_package_name() -> str:
    try:
        content = CARGO_TOML.read_text(encoding="utf-8")
    except FileNotFoundError:
        die(f"File not found: {CARGO_TOML}")

    package_section_match = re.search(r"(?ms)^\[package\]\n(.*?)(?=^\[|\Z)", content)
    if not package_section_match:
        die("[package] section not found in src-tauri/Cargo.toml")

    name_match = re.search(r'(?m)^name\s*=\s*"([^"]+)"\s*$', package_section_match.group(1))
    if not name_match:
        die("name key not found in Cargo.toml [package] section")

    return name_match.group(1)


def write_cargo_version(version: str) -> None:
    try:
        content = CARGO_TOML.read_text(encoding="utf-8")
    except FileNotFoundError:
        die(f"File not found: {CARGO_TOML}")

    package_section_match = re.search(r"(?ms)^\[package\]\n(.*?)(?=^\[|\Z)", content)
    if not package_section_match:
        die("[package] section not found in src-tauri/Cargo.toml")

    package_section = package_section_match.group(1)
    if not re.search(r'(?m)^version\s*=\s*"[^"]+"\s*$', package_section):
        die("version key not found in Cargo.toml [package] section")

    updated_section = re.sub(
        r'(?m)^version\s*=\s*"[^"]+"\s*$',
        f'version = "{version}"',
        package_section,
        count=1,
    )

    start, end = package_section_match.span(1)
    CARGO_TOML.write_text(content[:start] + updated_section + content[end:], encoding="utf-8")


def write_cargo_lock_version(version: str) -> None:
    if not CARGO_LOCK.exists():
        warn(f"{CARGO_LOCK} not found, skipping")
        return

    package_name = get_cargo_package_name()

    try:
        content = CARGO_LOCK.read_text(encoding="utf-8")
    except FileNotFoundError:
        die(f"File not found: {CARGO_LOCK}")

    # Local/workspace packages in Cargo.lock carry no "source"/"checksum"
    # field, so patching the version in place is safe.
    block_re = re.compile(
        r'(?ms)(^\[\[package\]\]\nname\s*=\s*"' + re.escape(package_name) + r'"\nversion\s*=\s*")[^"]+(")'
    )
    updated_content, count = block_re.subn(rf"\g<1>{version}\g<2>", content, count=1)
    if count == 0:
        die(f"Package '{package_name}' entry not found in src-tauri/Cargo.lock")

    CARGO_LOCK.write_text(updated_content, encoding="utf-8")


def write_qxchat_nix_version(version: str) -> None:
    try:
        content = QXCHAT_NIX.read_text(encoding="utf-8")
    except FileNotFoundError:
        die(f"File not found: {QXCHAT_NIX}")

    if not re.search(r'(?m)^\s*version\s*=\s*"[^"]+"\s*;', content):
        die("version key not found in nix/qxchat.nix")

    updated_content = re.sub(
        r'(?m)^(\s*)version\s*=\s*"[^"]+"\s*;',
        rf'\1version = "{version}";',
        content,
        count=1,
    )
    QXCHAT_NIX.write_text(updated_content, encoding="utf-8")


# --- release orchestration -----------------------------------------------

def validate_release(version: str, push: bool) -> bool:
    tag_name = f"v{version}"

    ensure_up_to_date(ROOT, push)
    if tag_exists(ROOT, tag_name):
        die(f"Tag {tag_name} already exists in {ROOT}")

    client_is_git_repo = CLIENT_PACKAGE_JSON.exists() and is_git_repo(CLIENT_DIR)
    if client_is_git_repo:
        ensure_up_to_date(CLIENT_DIR, push)
    if client_is_git_repo and tag_exists(CLIENT_DIR, tag_name):
        warn(f"tag {tag_name} already exists in {CLIENT_DIR}, skipping")
        client_is_git_repo = False
    if CLIENT_PACKAGE_JSON.exists() and not client_is_git_repo:
        warn("client is not a git repository, web tag skipped")

    return client_is_git_repo


def release_version(version: str, push: bool, client_is_git_repo: bool) -> None:
    if client_is_git_repo:
        commit_changes(CLIENT_DIR, ["package.json"], f"Release v{version}")
        create_version_tag(CLIENT_DIR, version, push)
        success(f"Web client tagged as v{version}")

    commit_changes(
        ROOT,
        [
            "package.json",
            "src-tauri/Cargo.toml",
            "src-tauri/Cargo.lock",
            "src-tauri/tauri.conf.json",
            "nix/qxchat.nix",
            "client",
        ],
        f"Release v{version}",
    )
    create_version_tag(ROOT, version, push)


def sync_lqxp_submodule(version: str) -> None:
    """If a sibling 'lqxp' project sits next to this repo, pull the freshly
    tagged release into its 'web' submodule and push the updated pointer.
    """
    if not is_git_repo(LQXP_SIBLING_DIR):
        return

    if not is_git_repo(LQXP_WEB_SUBMODULE):
        warn(f"{LQXP_WEB_SUBMODULE} not found or not a git repo, skipping submodule sync")
        return

    tag_name = f"v{version}"
    info(f"Syncing {LQXP_WEB_SUBMODULE} to {tag_name}")
    run_git(["fetch", "origin", "--tags"], cwd=LQXP_WEB_SUBMODULE)
    run_git(["checkout", tag_name], cwd=LQXP_WEB_SUBMODULE)

    if not has_changes(LQXP_SIBLING_DIR, ["web"]):
        info(f"{LQXP_SIBLING_DIR} already points at {tag_name}, nothing to push")
        return

    ensure_up_to_date(LQXP_SIBLING_DIR, push=True)
    commit_changes(LQXP_SIBLING_DIR, ["web"], f"Update web submodule to {tag_name}")
    run_git(["push", "origin", "HEAD"], cwd=LQXP_SIBLING_DIR)
    success(f"lqxp/web submodule pointer pushed for {tag_name}")


# --- CLI -------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--no-push", action="store_true", help="Don't push commits/tags to origin")

    parser = argparse.ArgumentParser(description="Bump and release the project version.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    for kind in ("patch", "minor", "major"):
        subparsers.add_parser(kind, parents=[common], help=f"Bump the {kind} version and release")

    set_parser = subparsers.add_parser("set", parents=[common], help="Set an explicit version without releasing")
    set_parser.add_argument("version", help="Version in MAJOR.MINOR.PATCH format")

    return parser


def main() -> None:
    args = build_parser().parse_args()
    push = not args.no_push

    if args.command == "set":
        new_version = args.version
        parse_semver(new_version)
        should_release = False
        client_is_git_repo = False
    else:
        current_version = load_package_version()
        new_version = bump_version(current_version, args.command)
        should_release = True
        client_is_git_repo = validate_release(new_version, push)

    write_package_version(new_version)
    write_client_package_version(new_version)
    write_cargo_version(new_version)
    write_cargo_lock_version(new_version)
    write_tauri_conf_version(new_version)
    write_qxchat_nix_version(new_version)

    if should_release:
        release_version(new_version, push, client_is_git_repo)
        if push:
            sync_lqxp_submodule(new_version)

    success(f"Version updated to {new_version}")
    print(paint("Updated files:", Color.BOLD))
    bullet("package.json")
    if CLIENT_PACKAGE_JSON.exists():
        bullet("client/package.json")
    bullet("src-tauri/Cargo.toml")
    if CARGO_LOCK.exists():
        bullet("src-tauri/Cargo.lock")
    bullet("src-tauri/tauri.conf.json")
    bullet("nix/qxchat.nix")
    if should_release:
        note(f"Created tag v{new_version} in app and web client")
        if push:
            note("Pushed commits and tags to GitHub")


if __name__ == "__main__":
    main()
