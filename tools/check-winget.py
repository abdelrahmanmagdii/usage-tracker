"""Validate generated WinGet manifests against Microsoft's published schemas.

    python tools/check-winget.py <manifest-dir> [<manifest-dir> ...]

Needs `pyyaml` and `jsonschema` (see .github/workflows/windows.yml). The
schemas come from a pinned winget-cli commit, so the check does not drift.
"""
import json
import pathlib
import sys
import urllib.request

import yaml
from jsonschema import Draft7Validator

WINGET_CLI_SHA = "d9acda871de78385492753450218694d54cd1078"
SCHEMA_URL = (
    "https://raw.githubusercontent.com/microsoft/winget-cli/{sha}"
    "/schemas/JSON/manifests/v{version}/manifest.{kind}.{version}.json"
)
KINDS = {"version": "version", "installer": "installer", "defaultLocale": "defaultLocale"}


def schema(kind: str, version: str) -> dict:
    url = SCHEMA_URL.format(sha=WINGET_CLI_SHA, kind=kind, version=version)
    with urllib.request.urlopen(url, timeout=30) as response:
        return json.load(response)


def check(directory: pathlib.Path) -> list[str]:
    problems = []
    seen = set()
    for file in sorted(directory.glob("*.yaml")):
        document = yaml.safe_load(file.read_text(encoding="utf-8"))
        kind = document.get("ManifestType") if isinstance(document, dict) else None
        if kind not in KINDS:
            problems.append(f"{file.name}: unknown ManifestType {kind!r}")
            continue
        seen.add(kind)
        validator = Draft7Validator(schema(kind, str(document.get("ManifestVersion"))))
        for error in validator.iter_errors(document):
            where = "/".join(str(part) for part in error.path) or "(root)"
            problems.append(f"{file.name}: {where}: {error.message}")
    for kind in KINDS.keys() - seen:
        problems.append(f"{directory}: missing the {kind} manifest")
    return problems


def main(directories: list[str]) -> int:
    if not directories:
        print(__doc__)
        return 2
    problems = [line for directory in directories for line in check(pathlib.Path(directory))]
    print("\n".join(problems) if problems else "WinGet manifests match the schema.")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
