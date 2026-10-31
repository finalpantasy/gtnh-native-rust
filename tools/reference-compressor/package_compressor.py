#!/usr/bin/env python3
"""Build the locked, source-only six-case compressor prototype packages."""

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
SUPPORT_ROOT = TOOL_ROOT / "support"
LOCK_SHA256 = "83cf9b0161c8758a28bec617a8258b70c6016c1806c5184a3e10632c4984002d"
FIXED_ZIP_TIME = (1980, 1, 1, 0, 0, 0)
MAX_ENTRIES = 512
MAX_EXPANDED_BYTES = 64 << 20
RECEIPT_NAME = "compressor-package-receipt.json"


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
    parts = value.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise ValueError(f"{label} contains an unsafe path segment")
    return PurePosixPath(*parts)


def resolve_under(root: Path, value: object, label: str) -> Path:
    relative = safe_relative(value, label)
    resolved_root = root.resolve()
    path = resolved_root
    for part in relative.parts:
        path /= part
        if path.is_symlink():
            raise ValueError(f"{label} resolves through a symlink")
    resolved = path.resolve(strict=False)
    try:
        resolved.relative_to(resolved_root)
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
    resolved_root = root.resolve()
    files: dict[str, bytes] = {}

    def on_error(exc: OSError) -> None:
        raise ValueError(f"could not inspect {label}") from exc

    for current, directories, filenames in os.walk(
        resolved_root, topdown=True, followlinks=False, onerror=on_error
    ):
        current_path = Path(current)
        for directory in directories:
            if (current_path / directory).is_symlink():
                raise ValueError(f"symlink in {label}")
        for filename in filenames:
            path = current_path / filename
            if path.is_symlink() or not path.is_file():
                raise ValueError(f"non-regular file in {label}")
            relative = path.relative_to(resolved_root).as_posix()
            safe_relative(relative, f"{label} file path")
            files[relative] = path.read_bytes()
    return files


def assert_file_set(actual: set[str], expected: set[str], label: str) -> None:
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    if missing or extra:
        raise ValueError(f"{label} allowlist mismatch; missing={missing}, extra={extra}")


def parse_toml(data: bytes, label: str) -> dict[str, object]:
    try:
        value = tomllib.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"{label} is not valid UTF-8 TOML") from exc
    if not isinstance(value, dict):
        raise ValueError(f"{label} must contain a TOML table")
    return value


def toml_string(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label} must be a string")
    return json.dumps(value, ensure_ascii=False)


def load_lock() -> dict[str, object]:
    raw = LOCK_PATH.read_bytes()
    if sha256(normalize_lf(raw)) != LOCK_SHA256:
        raise ValueError("bundle-lock.json changed; review and update the pinned lock hash first")
    lock = read_json(raw, "bundle-lock.json")
    if lock.get("schema") != 1:
        raise ValueError("unsupported bundle-lock.json schema")
    if set(lock) != {"schema", "package", "pack_inputs", "support_files", "native_templates", "content_templates"}:
        raise ValueError("bundle-lock.json contains unreviewed top-level fields")
    return lock


def add_entry(
    entries: dict[str, tuple[bytes, dict[str, object]]],
    archive_path: object,
    data: bytes,
    origin: dict[str, object],
    expected_hash: object,
    label: str,
) -> None:
    path = safe_relative(archive_path, f"{label} archive path").as_posix()
    digest = sha256(data)
    if digest != expected_hash:
        raise ValueError(f"{label} hash differs from the bundle lock")
    if path in entries:
        raise ValueError(f"duplicate archive path: {path}")
    entries[path] = (data, origin)


def render_standalone_manifest(crate: dict[str, object], workspace: dict[str, object]) -> bytes:
    workspace_package = workspace.get("package")
    workspace_lints = workspace.get("lints")
    dependencies = crate.get("dependencies")
    if not isinstance(workspace_package, dict) or not isinstance(workspace_lints, dict):
        raise ValueError("workspace metadata is malformed")
    if not isinstance(dependencies, dict):
        raise ValueError("locked dependencies are malformed")
    fields = ("version", "edition", "rust-version", "license")
    if any(not isinstance(workspace_package.get(field), str) for field in fields):
        raise ValueError("workspace package values must be literal strings")
    rust_lints = workspace_lints.get("rust")
    if not isinstance(rust_lints, dict) or rust_lints != {"unsafe_code": "forbid"}:
        raise ValueError("workspace Rust lint metadata differs from the reviewed package")

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
    for name in sorted(dependencies):
        dependency = dependencies[name]
        if name == "serde":
            if not isinstance(dependency, dict) or dependency.get("features") != ["derive"]:
                raise ValueError("serde dependency must enable only the reviewed derive feature")
            requirement = dependency.get("requirement")
            lines.append(
                f"{name} = {{ version = {toml_string(requirement, 'serde requirement')}, features = [\"derive\"] }}"
            )
        else:
            if not isinstance(dependency, dict):
                raise ValueError(f"{name} dependency pin is malformed")
            lines.append(f"{name} = {toml_string(dependency.get('requirement'), f'{name} requirement')}")
    lines.extend(["", "[lints.rust]"])
    for lint, setting in sorted(rust_lints.items()):
        lines.append(f"{lint} = {toml_string(setting, f'{lint} lint')}")
    return ("\n".join(lines) + "\n").encode("utf-8")


def assemble_runtime(support: dict[str, bytes], fixture: bytes, expected_fixture_hash: object) -> bytes:
    fixture = normalize_lf(fixture)
    if sha256(fixture) != expected_fixture_hash:
        raise ValueError("six-case fixture hash differs from the bundle lock")
    try:
        fixture_text = fixture.decode("ascii")
        bridge = normalize_lf(support["horizon_machine.rs"]).decode("utf-8")
        adapter = normalize_lf(support["compressor_adapter.rs"]).decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError("runtime inputs must be ASCII fixture JSON and UTF-8 Rust") from exc
    marker = 'const CONTENT: &[u8] = include_bytes!("../fixtures/compressor-reference-v1.json");'
    if bridge.count(marker) != 1:
        raise ValueError("bridge source must contain exactly one fixture include marker")
    if "#[cfg(test)]" in adapter:
        raise ValueError("test-only adapter code must not be injected into the package")
    bundle_id = "gtnh_compressor_eval"
    content_block = f"{bundle_id}:reference_compressor"
    if f'pub const BLOCK: &str = "{content_block}";' not in adapter:
        raise ValueError("adapter block ID differs from the content package")
    try:
        fixture_object = read_json(fixture, "compressor reference fixture")
    except ValueError as exc:
        raise ValueError("compressor reference fixture is invalid") from exc
    recipes = fixture_object.get("recipes")
    if not isinstance(recipes, list) or len(recipes) != 6:
        raise ValueError("prototype fixture must contain exactly six compressor cases")

    raw = fixture_text
    hashes = "#"
    while f'"{hashes}' in raw:
        hashes += "#"
    bridge = bridge.replace(marker, f"const CONTENT: &[u8] = br{hashes}\"{raw}\"{hashes};")
    source = (
        "pub(crate) mod bridge {\n"
        f"{bridge.rstrip()}\n"
        "}\n\n"
        "pub(crate) mod adapter {\n"
        f"{adapter.rstrip()}\n"
        "}\n"
    )
    return source.encode("utf-8")


def deterministic_zip(entries: dict[str, tuple[bytes, dict[str, object]]]) -> bytes:
    if len(entries) > MAX_ENTRIES:
        raise ValueError(f"archive exceeds the {MAX_ENTRIES}-entry limit")
    if sum(len(data) for data, _ in entries.values()) > MAX_EXPANDED_BYTES:
        raise ValueError("archive exceeds the 64 MiB expanded-size limit")
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
    entries: dict[str, tuple[bytes, dict[str, object]]],
    label: str,
) -> list[dict[str, object]]:
    with zipfile.ZipFile(io.BytesIO(archive_bytes), "r") as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        if names != sorted(entries) or len(names) != len(set(names)):
            raise ValueError(f"{label} archive order or uniqueness check failed")
        receipt: list[dict[str, object]] = []
        expanded = 0
        for info in infos:
            name = safe_relative(info.filename, "archive entry path").as_posix()
            data = archive.read(info)
            expected_data, origin = entries[name]
            mode = (info.external_attr >> 16) & 0xFFFF
            if data != expected_data or info.compress_type != zipfile.ZIP_STORED:
                raise ValueError(f"{label} archive entry failed byte/compression verification: {name}")
            if info.create_system != 3 or mode != (stat.S_IFREG | 0o644) or info.date_time != FIXED_ZIP_TIME:
                raise ValueError(f"{label} archive metadata is not deterministic: {name}")
            expanded += len(data)
            receipt.append({
                "path": name,
                "bytes": len(data),
                "sha256": sha256(data),
                "source": origin,
            })
        if expanded > MAX_EXPANDED_BYTES:
            raise ValueError(f"{label} archive exceeds the expanded-size limit")
        return receipt


def load_inputs(pack_root: Path, lock: dict[str, object]) -> tuple[dict[str, bytes], dict[str, bytes], dict[str, object]]:
    pack_inputs = lock.get("pack_inputs")
    package = lock.get("package")
    if not isinstance(pack_inputs, dict) or not isinstance(package, dict):
        raise ValueError("bundle lock input metadata is malformed")

    workspace_meta = pack_inputs.get("workspace_manifest")
    if not isinstance(workspace_meta, dict):
        raise ValueError("workspace manifest lock is malformed")
    workspace_path = safe_relative(workspace_meta.get("path"), "workspace manifest path").as_posix()
    workspace_raw = normalize_lf(read_required_file(pack_root, workspace_path, "workspace Cargo.toml"))
    if sha256(workspace_raw) != workspace_meta.get("sha256"):
        raise ValueError("native-foundation Cargo.toml differs from the bundle lock")
    workspace_toml = parse_toml(workspace_raw, "native-foundation Cargo.toml")
    workspace = {
        "package": workspace_toml.get("workspace", {}).get("package"),
        "lints": workspace_toml.get("workspace", {}).get("lints"),
    }
    if workspace != pack_inputs.get("workspace_metadata"):
        raise ValueError("literal workspace package/lint metadata differs from the bundle lock")

    crate = pack_inputs.get("crate")
    if not isinstance(crate, dict):
        raise ValueError("industrial-sim lock is malformed")
    source_dir = safe_relative(crate.get("source_dir"), "industrial-sim source directory").as_posix()
    source_files = walk_regular_files(resolve_under(pack_root, source_dir, "industrial-sim source directory"), "industrial-sim source")
    records = crate.get("source_files")
    if not isinstance(records, list):
        raise ValueError("industrial-sim source allowlist is malformed")
    expected_files = {"Cargo.toml"}
    normalized_sources: dict[str, bytes] = {}
    for record in records:
        if not isinstance(record, dict):
            raise ValueError("industrial-sim source hash record is malformed")
        relative = safe_relative(record.get("path"), "industrial-sim source path").as_posix()
        if not relative.startswith("src/") or not relative.endswith(".rs") or relative in expected_files:
            raise ValueError(f"unreviewed industrial-sim source path: {relative}")
        expected_files.add(relative)
        raw = source_files.get(relative)
        if raw is None:
            raise ValueError(f"industrial-sim source file is missing: {relative}")
        data = normalize_lf(raw)
        if sha256(data) != record.get("sha256"):
            raise ValueError(f"industrial-sim source hash differs from the lock: {relative}")
        normalized_sources[relative] = data
    assert_file_set(set(source_files), expected_files, "industrial-sim source")

    raw_manifest = normalize_lf(source_files["Cargo.toml"])
    if sha256(raw_manifest) != crate.get("source_manifest_sha256"):
        raise ValueError("industrial-sim Cargo.toml differs from the bundle lock")
    manifest = parse_toml(raw_manifest, "industrial-sim Cargo.toml")
    if set(manifest) != {"package", "dependencies", "lints"}:
        raise ValueError("industrial-sim Cargo.toml has an unreviewed section")
    if manifest.get("package") != {
        "name": crate.get("source_name"),
        "version": {"workspace": True},
        "edition": {"workspace": True},
        "rust-version": {"workspace": True},
        "license": {"workspace": True},
    } or manifest.get("lints") != {"workspace": True}:
        raise ValueError("industrial-sim Cargo.toml is not the reviewed workspace member")
    dependencies = crate.get("dependencies")
    if not isinstance(dependencies, dict):
        raise ValueError("direct dependency lock is malformed")
    expected_dependencies: dict[str, object] = {}
    for name, pin in dependencies.items():
        if not isinstance(pin, dict) or not isinstance(pin.get("requirement"), str):
            raise ValueError(f"{name} dependency lock is malformed")
        expected_dependencies[name] = (
            {"version": pin["requirement"], "features": pin["features"]}
            if name == "serde"
            else pin["requirement"]
        )
    if manifest.get("dependencies") != expected_dependencies:
        raise ValueError("industrial-sim direct dependencies differ from the bundle lock")

    support_files = walk_regular_files(SUPPORT_ROOT, "support")
    support_records = lock.get("support_files")
    if not isinstance(support_records, list):
        raise ValueError("support allowlist is malformed")
    expected_support: set[str] = set()
    normalized_support: dict[str, bytes] = {}
    for record in support_records:
        if not isinstance(record, dict):
            raise ValueError("support hash record is malformed")
        relative = safe_relative(record.get("path"), "support source path").as_posix()
        expected_support.add(relative)
        raw = support_files.get(relative)
        if raw is None:
            raise ValueError(f"support source is missing: {relative}")
        data = normalize_lf(raw)
        if sha256(data) != record.get("sha256"):
            raise ValueError(f"support source hash differs from the lock: {relative}")
        normalized_support[relative] = data
    assert_file_set(set(support_files), expected_support, "support")
    if set(normalized_support) != {"horizon_machine.rs", "compressor_adapter.rs"}:
        raise ValueError("support allowlist must contain exactly the bridge and production adapter")

    fixture_meta = pack_inputs.get("fixture")
    if not isinstance(fixture_meta, dict):
        raise ValueError("fixture lock is malformed")
    fixture = normalize_lf(read_required_file(pack_root, fixture_meta.get("path"), "six-case fixture"))
    if sha256(fixture) != fixture_meta.get("sha256"):
        raise ValueError("six-case fixture differs from the bundle lock")

    return normalized_sources, normalized_support, {
        "workspace": workspace,
        "crate": crate,
        "fixture": fixture,
        "fixture_hash": fixture_meta.get("sha256"),
        "workspace_path": workspace_path,
        "crate_source_dir": source_dir,
        "fixture_path": safe_relative(fixture_meta.get("path"), "fixture path").as_posix(),
    }


def load_templates(lock: dict[str, object]) -> tuple[dict[str, bytes], dict[str, bytes]]:
    native_records = lock.get("native_templates")
    content_records = lock.get("content_templates")
    if not isinstance(native_records, list) or not isinstance(content_records, list):
        raise ValueError("template allowlists are malformed")
    native = walk_regular_files(TEMPLATE_ROOT / "native", "native templates")
    content = walk_regular_files(TEMPLATE_ROOT / "content", "content templates")

    def check(records: list[object], files: dict[str, bytes], label: str) -> dict[str, bytes]:
        expected: set[str] = set()
        normalized: dict[str, bytes] = {}
        for record in records:
            if not isinstance(record, dict):
                raise ValueError(f"{label} template record is malformed")
            relative = safe_relative(record.get("path"), f"{label} template path").as_posix()
            expected.add(relative)
            raw = files.get(relative)
            if raw is None:
                raise ValueError(f"{label} template is missing: {relative}")
            data = normalize_lf(raw)
            if sha256(data) != record.get("sha256"):
                raise ValueError(f"{label} template hash differs from the lock: {relative}")
            normalized[relative] = data
        assert_file_set(set(files), expected, f"{label} template")
        return normalized

    return check(native_records, native, "native"), check(content_records, content, "content")


def build_archives(pack_root: Path, lock: dict[str, object]) -> tuple[dict[str, bytes], list[dict[str, object]]]:
    package = lock["package"]
    if not isinstance(package, dict):
        raise ValueError("package metadata is malformed")
    native_id = package.get("id")
    if native_id != "gtnh_compressor_eval":
        raise ValueError("unexpected compressor package identity")
    native_sources, support, inputs = load_inputs(pack_root, lock)
    native_templates, content_templates = load_templates(lock)

    entries: dict[str, tuple[bytes, dict[str, object]]] = {}
    entries["horizon-native.json"] = (
        native_templates.pop("horizon-native.json"),
        {"kind": "template", "path": "templates/native/horizon-native.json"},
    )
    native_manifest = read_json(entries["horizon-native.json"][0], "native manifest")
    if native_manifest != {
        "schema": 1,
        "id": native_id,
        "version": package.get("version"),
        "game": package.get("game"),
        "horizon": package.get("horizon_revision"),
        "mixins": package.get("mixins_revision"),
    }:
        raise ValueError("native manifest identity or source revisions differ from the lock")

    native_template_records = lock["native_templates"]
    record_by_path = {record["path"]: record for record in native_template_records}
    for relative, data in sorted(native_templates.items()):
        record = record_by_path[relative]
        archive_path = safe_relative(record.get("archive_path"), "native template archive path").as_posix()
        add_entry(
            entries,
            archive_path,
            data,
            {"kind": "template", "path": f"templates/native/{relative}"},
            record.get("sha256"),
            f"native template {relative}",
        )

    fixture = inputs["fixture"]
    runtime = assemble_runtime(support, fixture, inputs["fixture_hash"])
    runtime_hash = package.get("runtime_sha256")
    add_entry(
        entries,
        f"mixins/{native_id}/runtime.rs",
        runtime,
        {
            "kind": "assembled_support",
            "source_paths": [
                "support/horizon_machine.rs",
                "support/compressor_adapter.rs",
                f"pack/{inputs['fixture_path']}",
            ],
        },
        runtime_hash,
        "assembled runtime",
    )

    license_template = entries.get(f"mixins/{native_id}/LICENSE")
    license_root = read_required_file(TOOL_ROOT, "LICENSE", "package MIT license")
    package_license_hash = lock["pack_inputs"].get("package_license_sha256")
    if sha256(normalize_lf(license_root)) != package_license_hash:
        raise ValueError("root LICENSE differs from the bundle lock")
    if license_template is None or normalize_lf(license_root) != license_template[0]:
        raise ValueError("root LICENSE and native archive MIT license template must match")

    workspace = inputs["workspace"]
    crate = inputs["crate"]
    standalone = render_standalone_manifest(crate, workspace)
    if sha256(standalone) != crate.get("standalone_manifest_sha256"):
        raise ValueError("rendered standalone Cargo.toml differs from the reviewed manifest")
    crate_dir = safe_relative(crate.get("archive_dir"), "crate archive directory").as_posix()
    add_entry(
        entries,
        f"{crate_dir}/Cargo.toml",
        standalone,
        {
            "kind": "generated_manifest",
            "source_paths": [inputs["workspace_path"], f"{inputs['crate_source_dir']}/Cargo.toml"],
        },
        crate.get("standalone_manifest_sha256"),
        "standalone crate manifest",
    )
    source_records = crate["source_files"]
    for record in source_records:
        relative = safe_relative(record["path"], "crate source path").as_posix()
        data = native_sources[relative]
        add_entry(
            entries,
            f"{crate_dir}/{relative}",
            data,
            {"kind": "pack", "path": f"pack/{inputs['crate_source_dir']}/{relative}"},
            record.get("sha256"),
            f"industrial-sim/{relative}",
        )

    native_bytes = deterministic_zip(entries)
    native_receipt = validate_archive(native_bytes, entries, "native")
    if sha256(native_bytes) != package.get("native_archive_sha256"):
        raise ValueError("native archive hash differs from the locked v3 reference")

    content_entries: dict[str, tuple[bytes, dict[str, object]]] = {}
    for relative, data in sorted(content_templates.items()):
        record = next(item for item in lock["content_templates"] if item["path"] == relative)
        archive_path = safe_relative(record.get("archive_path"), "content archive path").as_posix()
        add_entry(
            content_entries,
            archive_path,
            data,
            {"kind": "template", "path": f"templates/content/{relative}"},
            record.get("sha256"),
            f"content template {relative}",
        )
    validate_content(content_entries, native_id)
    content_bytes = deterministic_zip(content_entries)
    content_receipt = validate_archive(content_bytes, content_entries, "content")
    if sha256(content_bytes) != package.get("content_archive_sha256"):
        raise ValueError("content archive hash differs from the locked v3 reference")

    artifacts = {
        package["native_archive"]: native_bytes,
        package["content_archive"]: content_bytes,
    }
    receipt = make_receipt(package, native_receipt, content_receipt, artifacts, inputs)
    return {**artifacts, RECEIPT_NAME: receipt}, [
        {"name": package["native_archive"], "bytes": len(native_bytes), "sha256": sha256(native_bytes), "entries": len(native_receipt)},
        {"name": package["content_archive"], "bytes": len(content_bytes), "sha256": sha256(content_bytes), "entries": len(content_receipt)},
    ]


def validate_content(entries: dict[str, tuple[bytes, dict[str, object]]], bundle_id: str) -> None:
    contents = {name: data for name, (data, _) in entries.items()}
    if any("textures" in name.split("/") for name in contents):
        raise ValueError("content package must not include texture assets")
    manifest_data = contents.get("horizon-mod.toml")
    if manifest_data is None:
        raise ValueError("content package has no horizon-mod.toml")
    manifest = parse_toml(manifest_data, "content manifest")
    if manifest != {
        "id": bundle_id,
        "name": "GTNH Reference Compressor",
        "version": "0.1.0",
        "side": "both",
        "api": "0.2",
        "capabilities": ["registries"],
    }:
        raise ValueError("content manifest differs from the reviewed prototype declaration")
    block_path = f"data/{bundle_id}/horizon/block/reference_compressor.json"
    block = read_json(contents.get(block_path, b""), "compressor block declaration")
    if block != {"template": "minecraft:furnace", "item": True}:
        raise ValueError("compressor block must use the vanilla furnace template")
    state_path = f"assets/{bundle_id}/blockstates/reference_compressor.json"
    states = read_json(contents.get(state_path, b""), "compressor blockstates").get("variants")
    if not isinstance(states, dict) or len(states) != 8 or any(
        not isinstance(item, dict) or not str(item.get("model", "")).startswith("minecraft:block/furnace")
        for item in states.values()
    ):
        raise ValueError("compressor blockstates must reference vanilla furnace models")
    item_path = f"assets/{bundle_id}/items/reference_compressor.json"
    item = read_json(contents.get(item_path, b""), "compressor item definition")
    if item.get("model") != {"type": "minecraft:model", "model": f"{bundle_id}:item/reference_compressor"}:
        raise ValueError("compressor item must use the bundle's vanilla-referencing model")
    recipe_path = f"data/{bundle_id}/recipe/evaluation_compressor.json"
    recipe = read_json(contents.get(recipe_path, b""), "evaluation compressor recipe")
    if recipe.get("result", {}).get("id") != f"{bundle_id}:reference_compressor":
        raise ValueError("evaluation recipe must create the declared compressor block")
    expected_count = 8
    if len(contents) != expected_count:
        raise ValueError("content package entry count differs from the reviewed allowlist")


def make_receipt(
    package: dict[str, object],
    native_entries: list[dict[str, object]],
    content_entries: list[dict[str, object]],
    artifacts: dict[str, bytes],
    inputs: dict[str, object],
) -> bytes:
    receipt = {
        "schema": 1,
        "status": "SOURCE_AND_ARCHIVE_CHECKS_PASS; RUNTIME_NOT_VERIFIED",
        "horizon_revision": package["horizon_revision"],
        "mixins_revision": package["mixins_revision"],
        "determinism": {
            "entry_order": "lexicographically sorted POSIX paths",
            "timestamp": "1980-01-01T00:00:00",
            "compression": "ZIP_STORED",
            "unix_entry_mode": "regular file 0644",
            "text_line_endings": "LF",
        },
        "archives": [
            {"name": package["native_archive"], "sha256": sha256(artifacts[package["native_archive"]]), "entries": native_entries},
            {"name": package["content_archive"], "sha256": sha256(artifacts[package["content_archive"]]), "entries": content_entries},
        ],
        "scope": [
            "Six-case compressor prototype and operator test-charge slice only.",
            "Source data are numerical declarations and do not establish full GTNH recipe parity.",
            "The content archive references vanilla furnace models and contains no textures.",
        ],
        "limits": [
            "The packager checks strict source/template allowlists, pinned hashes, archive structure, and reference archive hashes.",
            "This receipt does not verify Rust compilation, launcher staging, game startup, or in-game behavior.",
            "Runtime status: NOT_VERIFIED.",
        ],
        "source_roots": {
            "foundation_crate": inputs["crate_source_dir"],
            "fixture": inputs["fixture_path"],
            "support": ["support/horizon_machine.rs", "support/compressor_adapter.rs"],
        },
    }
    return (json.dumps(receipt, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def output_root(value: str, pack_root: Path) -> Path:
    requested = Path(value).expanduser()
    if not requested.is_absolute():
        requested = Path.cwd() / requested
    resolved = requested.resolve(strict=False)
    source_roots = (pack_root.resolve(), TOOL_ROOT.resolve())
    if any(resolved == root or root in resolved.parents for root in source_roots):
        raise ValueError("output directory must be outside both --pack-root and the packaging source tree")
    if requested.exists() or requested.is_symlink():
        raise ValueError("output directory already exists; choose a fresh path to avoid overwriting files")
    if not requested.parent.resolve(strict=False).is_dir():
        raise ValueError("output directory parent must already exist")
    return resolved


def write_output(output_dir: Path, files: dict[str, bytes]) -> None:
    if output_dir.exists() or output_dir.is_symlink():
        raise ValueError("output directory appeared before writing; nothing was written")
    with tempfile.TemporaryDirectory(prefix=f".{output_dir.name}.staging-", dir=output_dir.parent) as temp_name:
        staging = Path(temp_name)
        for name, data in files.items():
            safe_relative(name, "output filename")
            target = staging / name
            with target.open("xb") as handle:
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
    lock = load_lock()
    files, archives = build_archives(pack_root, lock)
    return {"output_dir": output_dir, "files": files, "archives": archives}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pack-root", required=True, help="GTNH pack source root containing native-foundation")
    parser.add_argument("--output-dir", required=True, help="new output directory outside both source roots")
    args = parser.parse_args()
    try:
        result = run(args.pack_root, args.output_dir)
        write_output(result["output_dir"], result["files"])
    except (OSError, ValueError, zipfile.BadZipFile) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    print(json.dumps({"status": "SOURCE_AND_ARCHIVE_CHECKS_PASS; RUNTIME_NOT_VERIFIED", "archives": result["archives"], "receipt": RECEIPT_NAME}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
