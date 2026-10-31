#!/usr/bin/env python3
"""Build the two locked, source-only Horizon proof bundles."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import stat
import sys
import tempfile
import tomllib
import zipfile
from pathlib import Path, PurePosixPath


TOOL_ROOT = Path(__file__).resolve().parent
LOCK_PATH = TOOL_ROOT / "bundle-lock.json"
TEMPLATE_ROOT = TOOL_ROOT / "templates"
LOCK_SHA256 = "3ae29079c5971dd84a608885b28cb6e93d828344c76fcba1add34d45ae5f9dd4"
HORIZON_REVISION = "681124a485b82a27ac4e769756a2f3a2356e6827"
MIXINS_REVISION = "4342a0bfbe71d4d14185528d64e496d6022f16a1"
FIXED_ZIP_TIME = (1980, 1, 1, 0, 0, 0)
MAX_ENTRIES = 512
MAX_EXPANDED_BYTES = 64 << 20
RECEIPT_NAME = "bundle-pack-receipt.json"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def normalize_lf(data: bytes) -> bytes:
    return data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")


def no_duplicate_json_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def read_json(data: bytes, label: str) -> dict[str, object]:
    try:
        parsed = json.loads(data.decode("utf-8"), object_pairs_hook=no_duplicate_json_keys)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"{label} is not valid UTF-8 JSON") from exc
    if not isinstance(parsed, dict):
        raise ValueError(f"{label} must contain a JSON object")
    return parsed


def safe_relative(value: object, label: str) -> PurePosixPath:
    if not isinstance(value, str) or not value or "\\" in value or ":" in value or "\x00" in value:
        raise ValueError(f"{label} must be a safe relative POSIX path")
    if value.startswith("/"):
        raise ValueError(f"{label} must be relative")
    raw_parts = value.split("/")
    if any(part in ("", ".", "..") for part in raw_parts):
        raise ValueError(f"{label} contains an unsafe path segment")
    return PurePosixPath(*raw_parts)


def resolve_under(root: Path, value: object, label: str) -> Path:
    relative = safe_relative(value, label)
    root = root.resolve()
    path = root
    for part in relative.parts:
        path = path / part
        if path.is_symlink():
            raise ValueError(f"{label} resolves through a symlink")
    resolved = path.resolve(strict=False)
    try:
        resolved.relative_to(root)
    except ValueError as exc:
        raise ValueError(f"{label} escapes its root") from exc
    return resolved


def read_required_file(root: Path, relative: object, label: str) -> bytes:
    path = resolve_under(root, relative, label)
    if not path.is_file():
        raise ValueError(f"missing regular file: {label}")
    try:
        return path.read_bytes()
    except OSError as exc:
        raise ValueError(f"could not read {label}") from exc


def walk_regular_files(root: Path, label: str) -> dict[str, bytes]:
    if root.is_symlink() or not root.is_dir():
        raise ValueError(f"{label} must be a regular directory")
    root = root.resolve()
    result: dict[str, bytes] = {}

    def on_walk_error(exc: OSError) -> None:
        raise ValueError(f"could not inspect {label}") from exc

    for current, directories, filenames in os.walk(root, topdown=True, followlinks=False, onerror=on_walk_error):
        current_path = Path(current)
        for directory in directories:
            if (current_path / directory).is_symlink():
                raise ValueError(f"symlink in {label}")
        for filename in filenames:
            path = current_path / filename
            if path.is_symlink() or not path.is_file():
                raise ValueError(f"non-regular file in {label}")
            relative = path.relative_to(root).as_posix()
            safe_relative(relative, f"{label} file path")
            try:
                result[relative] = path.read_bytes()
            except OSError as exc:
                raise ValueError(f"could not read a file in {label}") from exc
    return result


def assert_file_set(actual: set[str], expected: set[str], label: str) -> None:
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    if missing or extra:
        raise ValueError(f"{label} allowlist mismatch; missing={missing}, extra={extra}")


def parse_toml(data: bytes, label: str) -> dict[str, object]:
    try:
        parsed = tomllib.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"{label} is not valid UTF-8 TOML") from exc
    if not isinstance(parsed, dict):
        raise ValueError(f"{label} must contain a TOML table")
    return parsed


def toml_string(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label} must be a string")
    return json.dumps(value, ensure_ascii=False)


def load_lock() -> tuple[dict[str, object], bytes]:
    try:
        raw = LOCK_PATH.read_bytes()
    except OSError as exc:
        raise ValueError("bundle-lock.json is missing or unreadable") from exc
    if sha256(normalize_lf(raw)) != LOCK_SHA256:
        raise ValueError("bundle-lock.json changed; review and update the pinned lock hash first")
    lock = read_json(raw, "bundle-lock.json")
    if lock.get("schema") != 1:
        raise ValueError("unsupported bundle-lock.json schema")
    if lock.get("horizon_revision") != HORIZON_REVISION or lock.get("mixins_revision") != MIXINS_REVISION:
        raise ValueError("bundle lock does not match the reviewed Horizon and Mixins revisions")
    bundles = lock.get("bundles")
    expected_ids = {"gtnh_foundation_proof", "gtnh_tick_probe"}
    if not isinstance(bundles, list) or {item.get("id") for item in bundles if isinstance(item, dict)} != expected_ids:
        raise ValueError("bundle lock must contain exactly the two reviewed proof bundles")
    if len(bundles) != 2:
        raise ValueError("bundle lock must contain exactly two proof bundles")
    return lock, raw


def load_workspace(pack_root: Path, lock: dict[str, object]) -> tuple[bytes, dict[str, object]]:
    workspace = lock.get("workspace")
    if not isinstance(workspace, dict):
        raise ValueError("bundle lock has no workspace metadata")
    relative = safe_relative(workspace.get("source_path"), "workspace source path")
    raw = read_required_file(pack_root, relative.as_posix(), "workspace source manifest")
    if sha256(normalize_lf(raw)) != workspace.get("source_lf_sha256"):
        raise ValueError("workspace Cargo.toml hash differs from the bundle lock")

    parsed = parse_toml(raw, "workspace Cargo.toml")
    workspace_table = parsed.get("workspace")
    if not isinstance(workspace_table, dict):
        raise ValueError("workspace Cargo.toml has no [workspace] table")
    package = workspace_table.get("package")
    expected_package = workspace.get("package")
    lints = workspace_table.get("lints")
    expected_lints = workspace.get("lints")
    if not isinstance(package, dict) or package != expected_package:
        raise ValueError("literal workspace package metadata differs from the bundle lock")
    if not isinstance(lints, dict) or lints != expected_lints:
        raise ValueError("literal workspace lint metadata differs from the bundle lock")
    if not isinstance(expected_package, dict) or not isinstance(expected_lints, dict):
        raise ValueError("bundle lock workspace metadata is malformed")
    return raw, {"source_path": relative.as_posix(), "package": package, "lints": lints}


def render_standalone_manifest(
    pack_root: Path,
    crate: dict[str, object],
    workspace: dict[str, object],
) -> tuple[bytes, list[str]]:
    source_dir = safe_relative(crate.get("source_dir"), "crate source directory").as_posix()
    manifest_path = f"{source_dir}/Cargo.toml"
    raw = read_required_file(pack_root, manifest_path, f"{crate.get('id')} source manifest")
    if sha256(normalize_lf(raw)) != crate.get("source_manifest_lf_sha256"):
        raise ValueError(f"{crate.get('id')} Cargo.toml hash differs from the bundle lock")

    parsed = parse_toml(raw, f"{crate.get('id')} Cargo.toml")
    if set(parsed) != {"package", "dependencies", "lints"}:
        raise ValueError(f"{crate.get('id')} Cargo.toml contains an unreviewed section")
    package = parsed.get("package")
    dependencies = parsed.get("dependencies")
    lints = parsed.get("lints")
    if not isinstance(package, dict) or set(package) != {"name", "version", "edition", "rust-version", "license"}:
        raise ValueError(f"{crate.get('id')} package metadata is not in the reviewed form")
    expected_package = {
        "name": crate.get("source_name"),
        "version": {"workspace": True},
        "edition": {"workspace": True},
        "rust-version": {"workspace": True},
        "license": {"workspace": True},
    }
    if package != expected_package or lints != {"workspace": True} or not isinstance(dependencies, dict):
        raise ValueError(f"{crate.get('id')} source manifest is not the reviewed workspace member")

    direct = crate.get("direct_dependencies")
    if not isinstance(direct, dict) or set(dependencies) != set(direct):
        raise ValueError(f"{crate.get('id')} direct dependencies differ from the lock")
    expected_dependencies: dict[str, object] = {}
    for name, pin in direct.items():
        if not isinstance(pin, dict) or not isinstance(pin.get("requirement"), str) or not isinstance(pin.get("locked_version"), str):
            raise ValueError(f"{crate.get('id')} has malformed dependency pins")
        requirement = pin["requirement"]
        expected_dependencies[name] = {"version": requirement, "features": ["derive"]} if name == "serde" else requirement
    if dependencies != expected_dependencies:
        raise ValueError(f"{crate.get('id')} dependency requirements/features differ from the lock")

    workspace_package = workspace.get("package")
    workspace_lints = workspace.get("lints")
    if not isinstance(workspace_package, dict) or not isinstance(workspace_lints, dict):
        raise ValueError("workspace metadata is malformed")
    required_fields = ("version", "edition", "rust-version", "license")
    if any(not isinstance(workspace_package.get(key), str) for key in required_fields):
        raise ValueError("workspace package values must be literal strings")
    rust_lints = workspace_lints.get("rust")
    if not isinstance(rust_lints, dict) or rust_lints != {"unsafe_code": "forbid"}:
        raise ValueError("workspace Rust lints differ from the reviewed bundle policy")

    lines = [
        "[package]",
        f"name = {toml_string(crate.get('package_name'), 'standalone package name')}",
        f"version = {toml_string(workspace_package['version'], 'workspace version')}",
        f"edition = {toml_string(workspace_package['edition'], 'workspace edition')}",
        f"rust-version = {toml_string(workspace_package['rust-version'], 'workspace rust-version')}",
        f"license = {toml_string(workspace_package['license'], 'workspace license')}",
        "publish = false",
        "",
        "[dependencies]",
    ]
    for name in sorted(direct):
        requirement = direct[name]["requirement"]
        if name == "serde":
            lines.append(f"{name} = {{ version = {toml_string(requirement, 'serde version')}, features = [\"derive\"] }}")
        else:
            lines.append(f"{name} = {toml_string(requirement, f'{name} version')}")
    lines.extend(["", "[lints.rust]"])
    for lint, setting in sorted(rust_lints.items()):
        lines.append(f"{lint} = {toml_string(setting, f'{lint} lint')}")
    rendered = ("\n".join(lines) + "\n").encode("utf-8")
    if sha256(rendered) != crate.get("standalone_manifest_sha256"):
        raise ValueError(f"{crate.get('id')} rendered Cargo.toml differs from the reviewed standalone manifest")
    return rendered, [workspace["source_path"], manifest_path]


def add_entry(
    entries: dict[str, tuple[bytes, dict[str, object]]],
    archive_path: object,
    data: bytes,
    origin: dict[str, object],
    expected_sha256: object,
    label: str,
) -> None:
    path = safe_relative(archive_path, f"{label} archive path").as_posix()
    digest = sha256(data)
    if digest != expected_sha256:
        raise ValueError(f"{label} hash differs from the bundle lock")
    if path in entries:
        raise ValueError(f"duplicate archive path: {path}")
    entries[path] = (data, origin)


def build_bundle(
    pack_root: Path,
    lock: dict[str, object],
    workspace: dict[str, object],
    bundle: dict[str, object],
) -> tuple[bytes, dict[str, object]]:
    bundle_id = bundle.get("id")
    if not isinstance(bundle_id, str):
        raise ValueError("bundle ID is malformed")
    entries: dict[str, tuple[bytes, dict[str, object]]] = {}

    templates = bundle.get("templates")
    if not isinstance(templates, list):
        raise ValueError(f"{bundle_id} template list is malformed")
    for template in templates:
        if not isinstance(template, dict):
            raise ValueError(f"{bundle_id} has a malformed template entry")
        template_path = safe_relative(template.get("template"), "template path").as_posix()
        data = normalize_lf(read_required_file(TEMPLATE_ROOT, template_path, f"template {template_path}"))
        add_entry(
            entries,
            template.get("archive_path"),
            data,
            {"kind": "template", "path": template_path},
            template.get("sha256"),
            f"{bundle_id} template {template_path}",
        )

    license_info = bundle.get("license")
    if not isinstance(license_info, dict):
        raise ValueError(f"{bundle_id} license record is malformed")
    license_path = safe_relative(license_info.get("source_path"), "license source path").as_posix()
    license_bytes = read_required_file(pack_root, license_path, f"{bundle_id} license")
    policy = license_info.get("newline_policy")
    if policy == "lf":
        license_bytes = normalize_lf(license_bytes)
    elif policy == "crlf":
        license_bytes = normalize_lf(license_bytes).replace(b"\n", b"\r\n")
    else:
        raise ValueError(f"{bundle_id} has an unsupported license newline policy")
    if sha256(normalize_lf(license_bytes)) != license_info.get("normalized_text_sha256"):
        raise ValueError(f"{bundle_id} normalized license text differs from the bundle lock")
    add_entry(
        entries,
        license_info.get("archive_path"),
        license_bytes,
        {"kind": "pack", "path": license_path, "newline_policy": policy},
        license_info.get("archive_sha256"),
        f"{bundle_id} license",
    )

    betterquesting = bundle.get("betterquesting_license")
    if betterquesting is not None:
        if not isinstance(betterquesting, dict):
            raise ValueError(f"{bundle_id} BetterQuesting license record is malformed")
        source_path = safe_relative(betterquesting.get("source_path"), "BetterQuesting license path").as_posix()
        data = normalize_lf(read_required_file(pack_root, source_path, "BetterQuesting MIT license"))
        add_entry(
            entries,
            betterquesting.get("archive_path"),
            data,
            {"kind": "pack", "path": source_path},
            betterquesting.get("sha256"),
            "BetterQuesting MIT license",
        )

    crates = bundle.get("library_crates", [])
    if not isinstance(crates, list):
        raise ValueError(f"{bundle_id} crate list is malformed")
    for crate in crates:
        if not isinstance(crate, dict):
            raise ValueError(f"{bundle_id} has a malformed crate record")
        source_dir = resolve_under(pack_root, crate.get("source_dir"), "crate source directory")
        files = walk_regular_files(source_dir, f"{bundle_id}/{crate.get('crate_id')} source")
        source_files = crate.get("source_files")
        if not isinstance(source_files, list):
            raise ValueError(f"{bundle_id}/{crate.get('crate_id')} source allowlist is malformed")
        expected_files = {"Cargo.toml"}
        for record in source_files:
            if not isinstance(record, dict):
                raise ValueError(f"{bundle_id}/{crate.get('crate_id')} has a malformed source hash")
            relative = safe_relative(record.get("path"), "crate source path").as_posix()
            if not relative.startswith("src/") or not relative.endswith(".rs"):
                raise ValueError(f"crate allowlist contains a non-Rust source: {relative}")
            if relative in expected_files:
                raise ValueError(f"duplicate crate source path: {relative}")
            expected_files.add(relative)
            raw_data = files.get(relative)
            data = normalize_lf(raw_data) if raw_data is not None else None
            if data is None or sha256(data) != record.get("sha256"):
                raise ValueError(f"source file hash differs from the lock: {crate.get('crate_id')}/{relative}")
            archive_dir = safe_relative(crate.get("archive_dir"), "crate archive directory").as_posix()
            add_entry(
                entries,
                f"{archive_dir}/{relative}",
                data,
                {"kind": "pack", "path": f"{safe_relative(crate.get('source_dir'), 'crate source directory').as_posix()}/{relative}"},
                record.get("sha256"),
                f"{crate.get('crate_id')}/{relative}",
            )
        assert_file_set(set(files), expected_files, f"{bundle_id}/{crate.get('crate_id')} source")

        rendered_manifest, manifest_sources = render_standalone_manifest(pack_root, crate, workspace)
        generated_path = f"{safe_relative(crate.get('archive_dir'), 'crate archive directory').as_posix()}/Cargo.toml"
        add_entry(
            entries,
            generated_path,
            rendered_manifest,
            {"kind": "generated_manifest", "source_paths": manifest_sources},
            crate.get("standalone_manifest_sha256"),
            f"{bundle_id}/{crate.get('crate_id')} standalone manifest",
        )

    if len(entries) > MAX_ENTRIES:
        raise ValueError(f"{bundle_id} exceeds the {MAX_ENTRIES}-entry Horizon limit")
    expanded_bytes = sum(len(data) for data, _ in entries.values())
    if expanded_bytes > MAX_EXPANDED_BYTES:
        raise ValueError(f"{bundle_id} exceeds the 64 MiB expanded-size limit")

    archive_bytes = deterministic_zip(entries)
    archive_digest = sha256(archive_bytes)
    if archive_digest != bundle.get("reference_archive_sha256"):
        raise ValueError(f"{bundle_id} archive differs from the reviewed reference archive")
    entry_receipt = validate_archive(archive_bytes, bundle, entries, lock)
    archive_name = bundle.get("archive_name")
    safe_relative(archive_name, "archive filename")
    if PurePosixPath(archive_name).name != archive_name or not archive_name.endswith(".hnmod"):
        raise ValueError(f"{bundle_id} archive filename is invalid")
    return archive_bytes, {
        "id": bundle_id,
        "archive": archive_name,
        "bytes": len(archive_bytes),
        "sha256": archive_digest,
        "entry_count": len(entry_receipt),
        "entries": entry_receipt,
    }


def deterministic_zip(entries: dict[str, tuple[bytes, dict[str, object]]]) -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED, allowZip64=False) as archive:
        archive.comment = b""
        for name in sorted(entries):
            data, _ = entries[name]
            info = zipfile.ZipInfo(name, date_time=FIXED_ZIP_TIME)
            info.compress_type = zipfile.ZIP_STORED
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            info.extra = b""
            info.comment = b""
            archive.writestr(info, data)
    return output.getvalue()


def validate_archive(
    archive_bytes: bytes,
    bundle: dict[str, object],
    entries: dict[str, tuple[bytes, dict[str, object]]],
    lock: dict[str, object],
) -> list[dict[str, object]]:
    with zipfile.ZipFile(io.BytesIO(archive_bytes), "r") as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        if names != sorted(entries) or len(names) != len(set(names)):
            raise ValueError(f"{bundle.get('id')} archive entry order or uniqueness check failed")
        if len(infos) > MAX_ENTRIES or any(info.is_dir() for info in infos):
            raise ValueError(f"{bundle.get('id')} archive has invalid entry structure")
        expanded = 0
        receipt = []
        for info in infos:
            name = safe_relative(info.filename, "archive entry path").as_posix()
            data = archive.read(info)
            expected_data, origin = entries[name]
            if data != expected_data or info.compress_type != zipfile.ZIP_STORED:
                raise ValueError(f"{bundle.get('id')} archive entry failed byte or compression verification: {name}")
            expanded += len(data)
            mode = (info.external_attr >> 16) & 0xFFFF
            if info.create_system != 3 or mode != (stat.S_IFREG | 0o644) or info.date_time != FIXED_ZIP_TIME:
                raise ValueError(f"{bundle.get('id')} archive metadata is not deterministic: {name}")
            receipt.append({
                "path": name,
                "bytes": len(data),
                "sha256": sha256(data),
                "source": origin,
            })
        if expanded > MAX_EXPANDED_BYTES:
            raise ValueError(f"{bundle.get('id')} archive exceeds the expanded-size limit")

        contents = {info.filename: archive.read(info) for info in infos}
        raw_manifest = contents.get("horizon-native.json")
        if raw_manifest is None:
            raise ValueError(f"{bundle.get('id')} has no horizon-native.json")
        manifest = read_json(raw_manifest, f"{bundle.get('id')} horizon-native.json")
        if (
            manifest.get("id") != bundle.get("id")
            or manifest.get("horizon") != lock.get("horizon_revision")
            or manifest.get("mixins") != lock.get("mixins_revision")
        ):
            raise ValueError(f"{bundle.get('id')} manifest identity or revisions do not match the lock")
        mixin_path = f"mixins/{bundle.get('id')}/horizon-mixin.toml"
        raw_mixin = contents.get(mixin_path)
        if raw_mixin is None:
            raise ValueError(f"{bundle.get('id')} has no horizon-mixin.toml")
        mixin = parse_toml(raw_mixin, f"{bundle.get('id')} horizon-mixin.toml")
        mod = mixin.get("mod")
        compatibility = mixin.get("compatibility")
        if (
            not isinstance(mod, dict)
            or not isinstance(compatibility, dict)
            or mod.get("id") != bundle.get("id")
            or mod.get("version") != manifest.get("version")
            or compatibility.get("horizon_revision") != lock.get("horizon_revision")
        ):
            raise ValueError(f"{bundle.get('id')} mixin identity or revision differs from the native manifest")
        return receipt


def make_receipt(lock: dict[str, object], archive_receipts: list[dict[str, object]]) -> bytes:
    receipt = {
        "schema": 1,
        "status": "SOURCE_AND_ARCHIVE_CHECKS_PASS; RUNTIME_NOT_CHECKED",
        "horizon_revision": lock["horizon_revision"],
        "mixins_revision": lock["mixins_revision"],
        "determinism": {
            "entry_order": "lexicographically sorted POSIX paths",
            "timestamp": "1980-01-01T00:00:00",
            "compression": "ZIP_STORED",
            "unix_entry_mode": "regular file 0644",
            "text_line_endings": "LF; the probe bundle LICENSE is canonicalized to CRLF",
        },
        "archives": archive_receipts,
        "scope": [
            "Two bounded Horizon proof bundles only; this does not package the complete GTNH modpack.",
            "The native Rust source files and notices are included only under the paths listed in this receipt.",
        ],
        "limits": [
            "Packaging checks locked source hashes, archive structure and the reviewed reference archive hashes.",
            "This tool does not run Cargo, compile native code, launch Horizon, or start the game.",
            "Native build staging, launcher behavior, and in-game hook execution are outside this receipt.",
        ],
    }
    return (json.dumps(receipt, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def output_root(value: str, pack_root: Path) -> Path:
    requested = Path(value).expanduser()
    if not requested.is_absolute():
        requested = Path.cwd() / requested
    resolved = requested.resolve(strict=False)
    if resolved == pack_root or pack_root in resolved.parents:
        raise ValueError("output directory must be outside --pack-root")
    if requested.exists() or requested.is_symlink():
        raise ValueError("output directory already exists; choose a new path to avoid overwriting files")
    if not requested.parent.resolve(strict=False).is_dir():
        raise ValueError("output directory parent must already exist")
    return resolved


def write_output(output_dir: Path, files: dict[str, bytes]) -> None:
    if output_dir.exists() or output_dir.is_symlink():
        raise ValueError("output directory appeared before writing; nothing was written")
    parent = output_dir.parent
    with tempfile.TemporaryDirectory(prefix=f".{output_dir.name}.staging-", dir=parent) as temp_name:
        staging = Path(temp_name)
        for name, data in files.items():
            safe_relative(name, "output filename")
            path = staging / name
            with path.open("xb") as handle:
                handle.write(data)
                handle.flush()
                os.fsync(handle.fileno())
        if output_dir.exists() or output_dir.is_symlink():
            raise ValueError("output directory appeared before finalization; nothing was published")
        os.rename(staging, output_dir)


def run(pack_root_arg: str, output_dir_arg: str) -> dict[str, object]:
    pack_root = Path(pack_root_arg).expanduser().resolve()
    if not pack_root.is_dir():
        raise ValueError("--pack-root must name an existing directory")
    output_dir = output_root(output_dir_arg, pack_root)

    lock, _ = load_lock()
    _, workspace = load_workspace(pack_root, lock)

    template_files = walk_regular_files(TEMPLATE_ROOT, "templates")
    locked_template_paths = {
        item["template"]
        for bundle in lock["bundles"]
        for item in bundle["templates"]
    }
    assert_file_set(set(template_files), locked_template_paths, "template")

    bundles_by_id = {bundle["id"]: bundle for bundle in lock["bundles"]}
    ordered_bundles = [bundles_by_id[name] for name in ("gtnh_foundation_proof", "gtnh_tick_probe")]
    staged: dict[str, bytes] = {}
    archive_receipts = []
    for bundle in ordered_bundles:
        archive_bytes, archive_receipt = build_bundle(pack_root, lock, workspace, bundle)
        staged[archive_receipt["archive"]] = archive_bytes
        archive_receipts.append(archive_receipt)

    receipt_bytes = make_receipt(lock, archive_receipts)
    read_json(receipt_bytes, RECEIPT_NAME)
    staged[RECEIPT_NAME] = receipt_bytes

    archive_names = [bundle["archive_name"] for bundle in ordered_bundles]
    return {
        "output_dir": output_dir,
        "files": staged,
        "status": "SOURCE_AND_ARCHIVE_CHECKS_PASS; RUNTIME_NOT_CHECKED",
        "archives": [
            {"name": item["archive"], "bytes": item["bytes"], "sha256": item["sha256"], "entries": item["entry_count"]}
            for item in archive_receipts
        ],
        "receipt": RECEIPT_NAME,
        "archive_names": archive_names,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Build locked, source-only Horizon proof bundles.")
    parser.add_argument("--output-dir", required=True, help="new output directory outside the source pack checkout")
    parser.add_argument("--pack-root", default=".", help="source pack checkout root (default: current directory)")
    args = parser.parse_args()
    try:
        result = run(args.pack_root, args.output_dir)
        write_output(result["output_dir"], result["files"])
    except (OSError, ValueError, zipfile.BadZipFile) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    print(json.dumps({
        "status": result["status"],
        "archives": result["archives"],
        "receipt": result["receipt"],
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
