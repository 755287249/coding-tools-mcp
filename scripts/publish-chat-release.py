"""Publish a tested standalone EXE using verified personal GitHub authorization."""
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


class GitHub:
    def __init__(self, token):
        self.token = token
        self.opener = urllib.request.build_opener(NoRedirect)

    def request(self, method, path, data=None, content_type="application/json", missing=False):
        url = path if path.startswith("https://") else "https://api.github.com" + path
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme != "https" or parsed.netloc not in {"api.github.com", "uploads.github.com"}:
            raise RuntimeError("Unexpected GitHub API host")
        body = json.dumps(data).encode() if data is not None and content_type == "application/json" else data
        request = urllib.request.Request(url, data=body, method=method, headers={
            "Authorization": "Bearer " + self.token,
            "User-Agent": "Coding-Tools-MCP-personal-release",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "Content-Type": content_type,
        })
        try:
            with self.opener.open(request, timeout=120) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            if missing and error.code == 404:
                return None
            raise RuntimeError(f"GitHub HTTP {error.code}: {method} {parsed.path}") from None


def publish(api, directory, repository, owner, version, commit):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) or repository.split('/')[0] != owner:
        raise RuntimeError("Repository owner mismatch")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version) or not re.fullmatch(r"[a-f0-9]{40}", commit):
        raise RuntimeError("Invalid release version or commit")
    identity = api.request("GET", "/user")
    if identity.get("login") != owner or identity.get("type") != "User":
        raise RuntimeError("Personal authorization must belong to the repository owner; bot fallback is forbidden")
    directory = Path(directory)
    name = f"ctmcp-{version}-win64.exe"
    binary = (directory / name).read_bytes()
    if len(binary) < 2_000_000 or not binary.startswith(b"MZ"):
        raise RuntimeError("Standalone EXE is missing or invalid")
    digest = hashlib.sha256(binary).hexdigest()
    checksums = (directory / "SHA256SUMS.txt").read_text(encoding="utf-8-sig").splitlines()
    if not any(line.split() == [digest, name] for line in checksums):
        raise RuntimeError("Standalone EXE checksum does not match the build artifact")
    tag = f"v{version}"
    prefix = f"/repos/{repository}"
    # Existing tags/releases must resolve to the exact tested source revision.
    reference = api.request("GET", f"{prefix}/git/ref/tags/{tag}", missing=True)
    if reference:
        obj = reference["object"]
        for _ in range(5):
            if obj["type"] != "tag":
                break
            obj = api.request("GET", f"{prefix}/git/tags/{obj['sha']}")["object"]
        if obj["type"] != "commit" or obj["sha"] != commit:
            raise RuntimeError("Existing release tag points to a different commit")
    release = api.request("GET", f"{prefix}/releases/tags/{tag}", missing=True)
    if release and not reference and release.get("target_commitish") != commit:
        raise RuntimeError("Existing draft release targets a different commit")
    if release and release.get("author", {}).get("login") != owner:
        raise RuntimeError("Existing release is not authored by the authorized owner")
    if not release:
        release = api.request("POST", f"{prefix}/releases", {
            "tag_name": tag, "target_commitish": commit, "name": f"Coding Tools MCP {version}",
            "draft": True, "prerelease": True,
            "body": f"Windows standalone EXE. Download and run directly.\n\nCommit: {commit}\n\nSHA-256: `{digest}`",
        })
    asset_path = f"{prefix}/releases/{int(release['id'])}/assets?per_page=100"
    assets = api.request("GET", asset_path)
    if len(assets) > 1 or any(asset.get("name") != name for asset in assets):
        raise RuntimeError("Unexpected existing release assets; inspect manually before publishing the single EXE")
    expected = "sha256:" + digest
    if assets:
        if assets[0].get("digest") != expected or assets[0].get("size") != len(binary):
            raise RuntimeError(f"Existing asset differs or lacks a verifiable digest: {name}")
    else:
        upload = f"https://uploads.github.com/repos/{repository}/releases/{int(release['id'])}/assets?name={urllib.parse.quote(name)}"
        asset = api.request("POST", upload, binary, content_type="application/octet-stream")
        if asset.get("size") != len(binary) or asset.get("digest") != expected:
            raise RuntimeError(f"Uploaded asset failed verification: {name}")
    # Re-read the server's asset list before making the draft public (also on retries).
    assets = api.request("GET", asset_path)
    if len(assets) != 1 or assets[0].get("name") != name or assets[0].get("digest") != expected or assets[0].get("size") != len(binary):
        raise RuntimeError("Release must contain exactly one verified versioned EXE")
    if release.get("draft"):
        release = api.request("PATCH", f"{prefix}/releases/{int(release['id'])}", {"draft": False})
    return release["html_url"]


def main():
    token = os.environ.get("PERSONAL_RELEASE_TOKEN", "")
    if not token:
        raise RuntimeError("PERSONAL_RELEASE_TOKEN is required; no bot-token fallback")
    return publish(GitHub(token), sys.argv[1], os.environ["RELEASE_REPOSITORY"], os.environ["RELEASE_OWNER"], os.environ["RELEASE_VERSION"], os.environ["RELEASE_COMMIT"])


if __name__ == "__main__":
    try:
        print(main())
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
