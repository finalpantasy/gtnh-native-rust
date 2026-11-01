#!/usr/bin/env python3
"""Assemble the pinned, source-only GTNH LV compressor evaluation bundle."""

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
REPO_ROOT = TOOL_ROOT.parents[1]
LOCK_PATH = TOOL_ROOT / "bundle-lock.json"
MOD = "gtnh_lv_compressor_eval"
BLOCK = f"{MOD}:lv_compressor"
HORIZON = "681124a485b82a27ac4e769756a2f3a2356e6827"
SDK = "3ac63df26d332bc344ba645d5d23497b7d4ad985"
RECEIPT_NAME = "lv-compressor-package-receipt.json"
HOOKS = (
    "block_entity",
    "block_entity_ticks",
    "block_entity_container",
    "block_entity_tick",
    "screen",
    "screen_draw",
    "register_commands",
    "perform_command",
)
FIXED_ZIP_TIME = (1980, 1, 1, 0, 0, 0)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def is_repo_input(path: str) -> bool:
    return path.startswith(("gtnh-runtime/", "native-foundation/"))


def relative(value: str, label: str = "package/input path") -> PurePosixPath:
    if not isinstance(value, str) or not value:
        raise ValueError(f"unsafe {label}: {value!r}")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or any(part in ("", ".", "..") for part in value.split("/"))
        or "\\" in value
        or ":" in value
    ):
        raise ValueError(f"unsafe {label}: {value}")
    return path


def is_link(path: Path) -> bool:
    if path.is_symlink():
        return True
    junction_check = getattr(path, "is_junction", lambda: False)
    if bool(junction_check()):
        return True
    try:
        attributes = getattr(path.lstat(), "st_file_attributes", 0)
    except FileNotFoundError:
        return False
    reparse_point = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(attributes & reparse_point)


def reject_link_components(path: Path, label: str) -> None:
    absolute = Path(os.path.abspath(path))
    current = Path(absolute.anchor)
    for part in absolute.parts[1:]:
        current = current / part
        if is_link(current):
            raise ValueError(f"linked {label}: {current}")


def source(root: Path, path: str) -> bytes:
    parts = relative(path, "source path").parts
    current = root
    reject_link_components(current, "source root")
    for part in parts:
        current = current / part
        if is_link(current):
            raise ValueError(f"linked source input: {path}")
    if not current.is_file():
        raise ValueError(f"source input is missing or not a file: {path}")
    return current.read_bytes()


def parse_json(data: bytes, label: str) -> object:
    try:
        return json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"{label} is not valid UTF-8 JSON") from exc


def parse_toml(data: bytes, label: str) -> dict[str, object]:
    try:
        return tomllib.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"{label} is not valid UTF-8 TOML") from exc


def load_lock() -> dict[str, object]:
    reject_link_components(LOCK_PATH, "bundle lock")
    try:
        lock = parse_json(LOCK_PATH.read_bytes(), "bundle lock")
    except OSError as exc:
        raise ValueError("bundle lock cannot be read") from exc
    if not isinstance(lock, dict) or set(lock) != {
        "schema",
        "package",
        "pack_inputs",
        "local_inputs",
        "native_templates",
        "content_templates",
    }:
        raise ValueError("bundle lock has an unsupported shape")
    if lock.get("schema") != 1:
        raise ValueError("unsupported bundle lock schema")
    return lock


def template_records(lock: dict[str, object], label: str) -> dict[str, str]:
    records = lock.get(label)
    if not isinstance(records, list):
        raise ValueError(f"{label} allowlist is malformed")
    result: dict[str, str] = {}
    for record in records:
        if not isinstance(record, dict) or set(record) != {"path", "sha256"}:
            raise ValueError(f"{label} record is malformed")
        path = relative(record["path"], f"{label} path").as_posix()
        digest = record["sha256"]
        if not isinstance(digest, str) or len(digest) != 64:
            raise ValueError(f"{label} hash is malformed: {path}")
        if path in result:
            raise ValueError(f"duplicate {label} path: {path}")
        result[path] = digest
    return result


def input_hashes(lock: dict[str, object]) -> dict[str, str]:
    pack_inputs = lock.get("pack_inputs")
    local_inputs = lock.get("local_inputs")
    if not isinstance(pack_inputs, dict) or set(pack_inputs) != {
        "files",
        "workspace_manifest",
        "workspace_metadata",
        "policy_manifest",
        "policy_source",
        "industrial_manifest",
        "industrial_source_dir",
        "industrial_fixture",
        "industrial_source_files",
        "dependencies",
        "crate_names",
    }:
        raise ValueError("pack input lock is malformed")
    pack_files = pack_inputs.get("files")
    if not isinstance(pack_files, dict) or not pack_files:
        raise ValueError("pack input file allowlist is malformed")
    if not isinstance(local_inputs, dict) or not local_inputs:
        raise ValueError("local input allowlist is malformed")

    path_fields = (
        "workspace_manifest",
        "policy_manifest",
        "policy_source",
        "industrial_manifest",
        "industrial_source_dir",
        "industrial_fixture",
    )
    referenced_pack_paths: set[str] = set()
    for field in path_fields:
        value = pack_inputs.get(field)
        safe_path = relative(value, f"{field} path").as_posix()
        if not is_repo_input(safe_path):
            raise ValueError(f"{field} must be repository-relative under gtnh-runtime/ or native-foundation/")
        if field != "industrial_source_dir":
            referenced_pack_paths.add(safe_path)
    source_files = pack_inputs.get("industrial_source_files")
    if not isinstance(source_files, list) or not source_files:
        raise ValueError("industrial source file allowlist is malformed")
    safe_source_files = [relative(path, "industrial source path").as_posix() for path in source_files]
    if len(set(safe_source_files)) != len(safe_source_files):
        raise ValueError("industrial source allowlist contains duplicate paths")
    source_dir = relative(pack_inputs["industrial_source_dir"], "industrial source directory").as_posix()
    referenced_pack_paths.update(f"{source_dir}/{path}" for path in safe_source_files)
    if set(pack_files) != referenced_pack_paths:
        raise ValueError("pack file allowlist differs from the explicitly referenced source inputs")
    if set(local_inputs) != {
        "LICENSE",
        "support/adapter.rs",
        "support/fixtures/compressor-reference-v1.json",
    }:
        raise ValueError("local support and license allowlist changed")

    result: dict[str, str] = {}
    for path, digest in pack_files.items():
        safe_path = relative(path, "pack input path").as_posix()
        if not is_repo_input(safe_path):
            raise ValueError(f"repository input is outside the approved crate roots: {safe_path}")
        if not isinstance(digest, str) or len(digest) != 64:
            raise ValueError(f"pack input hash is malformed: {safe_path}")
        result[safe_path] = digest
    for path, digest in local_inputs.items():
        safe_path = relative(path, "local input path").as_posix()
        if is_repo_input(safe_path):
            raise ValueError(f"local input must stay inside the packaging source tree: {safe_path}")
        if not isinstance(digest, str) or len(digest) != 64:
            raise ValueError(f"local input hash is malformed: {safe_path}")
        if safe_path in result:
            raise ValueError(f"duplicate input path: {safe_path}")
        result[safe_path] = digest

    for label, prefix in (("native_templates", "templates/native/"), ("content_templates", "templates/content/")):
        for path, digest in template_records(lock, label).items():
            local_path = prefix + path
            if local_path in result:
                raise ValueError(f"duplicate input path: {local_path}")
            result[local_path] = digest

    if len(result) != len(pack_files) + len(local_inputs) + len(template_records(lock, "native_templates")) + len(
        template_records(lock, "content_templates")
    ):
        raise ValueError("bundle lock contains duplicate approved inputs")
    if len(template_records(lock, "native_templates")) != 11 or len(template_records(lock, "content_templates")) != 8:
        raise ValueError("bundle lock must contain exactly 19 reviewed templates")
    return result


def walk_regular_files(root: Path) -> set[str]:
    reject_link_components(root, "source tree")
    if not root.is_dir():
        raise ValueError(f"approved source directory is missing: {root.name}")
    found: set[str] = set()
    pending = [root]
    while pending:
        directory = pending.pop()
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name)
        except OSError as exc:
            raise ValueError(f"approved source directory cannot be read: {directory.name}") from exc
        for entry in entries:
            path = Path(entry.path)
            if is_link(path):
                raise ValueError(f"linked source input: {path.name}")
            if entry.is_dir(follow_symlinks=False):
                pending.append(path)
            elif entry.is_file(follow_symlinks=False):
                found.add(path.relative_to(TOOL_ROOT).as_posix())
            else:
                raise ValueError(f"non-regular approved source entry: {path.name}")
    return found


def approved_inputs(repo_root: Path | str = REPO_ROOT) -> tuple[dict[str, bytes], str]:
    """Read the strict, pinned repository and local source allowlists."""
    lock = load_lock()
    requested_root = Path(repo_root).expanduser()
    if not requested_root.is_absolute():
        requested_root = Path.cwd() / requested_root
    repository_root = Path(os.path.abspath(requested_root))
    reject_link_components(repository_root, "repository root")
    if not repository_root.is_dir():
        raise ValueError("--repo-root must name an existing repository root")

    hashes = input_hashes(lock)
    expected_templates = {
        path for path in hashes if path.startswith("templates/native/") or path.startswith("templates/content/")
    }
    actual_templates = walk_regular_files(TOOL_ROOT / "templates")
    if actual_templates != expected_templates:
        raise ValueError("template allowlist changed")
    expected_support = {path for path in hashes if path.startswith("support/")}
    actual_support = walk_regular_files(TOOL_ROOT / "support")
    if actual_support != expected_support:
        raise ValueError("support-file allowlist changed")

    data: dict[str, bytes] = {}
    for path in sorted(hashes):
        base = repository_root if is_repo_input(path) else TOOL_ROOT
        content = source(base, path)
        if sha256(content) != hashes[path]:
            raise ValueError(f"source changed after approval: {path}")
        data[path] = content
    return data, sha256(LOCK_PATH.read_bytes())


def manifest(package_name: str, original: dict[str, object], dependencies: list[str]) -> bytes:
    allowed = {"package", "dependencies", "lints", "workspace"}
    if set(original) - allowed:
        raise ValueError("unreviewed source manifest table")
    package = original.get("package")
    lints = original.get("lints")
    if not isinstance(package, dict) or not isinstance(lints, dict):
        raise ValueError("source manifest package or lint settings are malformed")
    lines = [
        "[package]",
        f'name = "{package_name}"',
        'version = "0.1.0"',
        'edition = "2024"',
        'rust-version = "1.90"',
        'license = "MIT"',
        "publish = false",
        "",
        "[dependencies]",
        *dependencies,
        "",
        "[lints.rust]",
        'unsafe_code = "forbid"',
        "",
    ]
    return "\n".join(lines).encode("utf-8")


def runtime(adapter: bytes) -> bytes:
    text = adapter.decode("utf-8")
    marker = 'const CONTENT: &[u8] = include_bytes!("fixtures/compressor-reference-v1.json");'
    test_module = '\n#[cfg(test)]\n#[path = "tests.rs"]\nmod tests;\n'
    if text.count(marker) != 1 or text.count(test_module) != 1:
        raise ValueError("adapter assembly marker changed")
    if f'pub const BLOCK: &str = "{BLOCK}";' not in text:
        raise ValueError("adapter owner differs from bundle")
    text = text.replace(
        marker,
        f'const CONTENT: &[u8] = horizon_client::native_data!("{MOD}", "fixtures/compressor-reference-v1.json");',
    )
    text = text.replace(test_module, "\n")
    return ("pub(crate) mod adapter {\n" + text + "\n}\n").encode("utf-8")


def zip_bytes(entries: dict[str, bytes]) -> bytes:
    if len(entries) > 512 or sum(map(len, entries.values())) > 64 << 20:
        raise ValueError("bundle exceeds bound")
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, data in sorted(entries.items()):
            relative(name, "archive member path")
            info = zipfile.ZipInfo(name, FIXED_ZIP_TIME)
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
    result = stream.getvalue()
    with zipfile.ZipFile(io.BytesIO(result)) as archive:
        members = archive.namelist()
        if len(members) != len(set(members)) or set(members) != set(entries):
            raise ValueError("archive members changed")
        for info in archive.infolist():
            if archive.read(info) != entries[info.filename] or info.date_time != FIXED_ZIP_TIME:
                raise ValueError("archive readback differs")
            if info.create_system != 3 or info.external_attr != (stat.S_IFREG | 0o644) << 16:
                raise ValueError("archive metadata differs")
    return result


def validate_content(entries: dict[str, bytes], bundle_id: str) -> None:
    if any("textures" in name.split("/") for name in entries):
        raise ValueError("content package must not include texture assets")
    manifest_data = entries.get("horizon-mod.toml")
    if manifest_data is None:
        raise ValueError("content package has no horizon-mod.toml")
    if parse_toml(manifest_data, "content manifest") != {
        "id": bundle_id,
        "name": "GTNH LV Compressor Evaluation",
        "version": "0.1.0",
        "side": "both",
        "api": "0.2",
        "capabilities": ["registries"],
    }:
        raise ValueError("content manifest differs from the reviewed evaluation declaration")
    block_path = f"data/{bundle_id}/horizon/block/lv_compressor.json"
    block = parse_json(entries.get(block_path, b""), "compressor block declaration")
    if block != {"template": "minecraft:furnace", "item": True}:
        raise ValueError("compressor block must use the vanilla furnace template")
    states_path = f"assets/{bundle_id}/blockstates/lv_compressor.json"
    states_root = parse_json(entries.get(states_path, b""), "compressor blockstates")
    states = states_root.get("variants") if isinstance(states_root, dict) else None
    if not isinstance(states, dict) or len(states) != 8 or any(
        not isinstance(item, dict) or not str(item.get("model", "")).startswith("minecraft:block/furnace")
        for item in states.values()
    ):
        raise ValueError("compressor blockstates must reference vanilla furnace models")
    item_path = f"assets/{bundle_id}/items/lv_compressor.json"
    item = parse_json(entries.get(item_path, b""), "compressor item definition")
    if not isinstance(item, dict) or item.get("model") != {
        "type": "minecraft:model",
        "model": f"{bundle_id}:item/lv_compressor",
    }:
        raise ValueError("compressor item must use the bundle's vanilla-referencing model")
    recipe_path = f"data/{bundle_id}/recipe/evaluation_lv_compressor.json"
    recipe = parse_json(entries.get(recipe_path, b""), "evaluation compressor recipe")
    if not isinstance(recipe, dict) or recipe.get("result", {}).get("id") != f"{bundle_id}:lv_compressor":
        raise ValueError("evaluation recipe must create the declared compressor block")
    if len(entries) != 8:
        raise ValueError("content package entry count differs from the reviewed allowlist")


def assemble(data: dict[str, bytes]) -> tuple[dict[str, bytes], dict[str, bytes], list[dict[str, object]]]:
    """Build native/content source members and record the three test-only path rewrites."""
    lock = load_lock()
    hashes = input_hashes(lock)
    if set(data) != set(hashes):
        raise ValueError("source input set differs from the bundle lock")
    for path, digest in hashes.items():
        if sha256(data[path]) != digest:
            raise ValueError(f"source changed after approval: {path}")

    package = lock["package"]
    pack_inputs = lock["pack_inputs"]
    if not isinstance(package, dict) or not isinstance(pack_inputs, dict):
        raise ValueError("bundle package or pack-input metadata is malformed")
    if (
        package.get("id") != MOD
        or package.get("horizon_revision") != HORIZON
        or package.get("horizon_api") != 45
        or package.get("sdk_revision") != SDK
    ):
        raise ValueError("bundle identity or pinned source revisions changed")

    def read(path: str) -> bytes:
        return data[path]

    policy = read(pack_inputs["policy_source"])
    fixture = read("support/fixtures/compressor-reference-v1.json")
    fixture_data = parse_json(fixture, "compressor reference fixture")
    if not isinstance(fixture_data, dict) or len(fixture_data.get("recipes", [])) != 6:
        raise ValueError("compressor reference fixture is not the approved six-declaration catalog")

    workspace = parse_toml(read(pack_inputs["workspace_manifest"]), "native workspace manifest")
    workspace_table = workspace.get("workspace")
    if not isinstance(workspace_table, dict) or workspace_table.get("package") != pack_inputs["workspace_metadata"]:
        raise ValueError("native workspace metadata changed")

    policy_manifest = parse_toml(read(pack_inputs["policy_manifest"]), "policy crate manifest")
    industrial_manifest = parse_toml(read(pack_inputs["industrial_manifest"]), "industrial-sim manifest")
    dependencies = pack_inputs["dependencies"]
    crate_names = pack_inputs["crate_names"]
    if not isinstance(crate_names, dict) or set(crate_names) != {
        "industrial_source",
        "industrial_package",
        "policy_source",
        "policy_package",
    }:
        raise ValueError("crate name mapping is malformed")
    industrial_package = industrial_manifest.get("package")
    policy_package = policy_manifest.get("package")
    if (
        not isinstance(industrial_package, dict)
        or industrial_package.get("name") != crate_names["industrial_source"]
        or not isinstance(policy_package, dict)
        or policy_package.get("name") != crate_names["policy_source"]
    ):
        raise ValueError("source and packaged crate names differ from the reviewed mapping")
    expected_policy_dependencies = {
        "industrial-sim": {"path": "../native-foundation/crates/industrial-sim"},
        **dependencies,
    }
    if industrial_manifest.get("dependencies") != dependencies or policy_manifest.get("dependencies") != expected_policy_dependencies:
        raise ValueError("source dependency definitions changed")

    native_prefix = "templates/native/"
    content_prefix = "templates/content/"
    native_names: set[str] = set()
    native: dict[str, bytes] = {}
    content: dict[str, bytes] = {}
    for path, value in data.items():
        if path.startswith(native_prefix):
            name = path[len(native_prefix) :]
            native_names.add(name)
            native[name] = value
        elif path.startswith(content_prefix):
            content[path[len(content_prefix) :]] = value
    expected_native = {
        f"mixins/{MOD}/horizon-mixin.toml",
        f"mixins/{MOD}/LICENSE",
        f"mixins/{MOD}/NOTICE.md",
        *(f"mixins/{MOD}/hooks/{hook}.rs" for hook in HOOKS),
    }
    if native_names != expected_native:
        raise ValueError("native template set differs from the reviewed allowlist")
    if len(content) != 8:
        raise ValueError("content template set differs from the reviewed allowlist")

    native_manifest = parse_toml(native[f"mixins/{MOD}/horizon-mixin.toml"], "native mixin manifest")
    if (
        native_manifest.get("mod", {}).get("id") != MOD
        or native_manifest.get("compatibility", {}).get("horizon_revision") != HORIZON
    ):
        raise ValueError("native identity or Horizon compatibility changed")
    declared_crates = {(crate.get("id"), crate.get("path")) for crate in native_manifest.get("crate", [])}
    if declared_crates != {("industrial_sim", "crates/industrial-sim"), ("basic_machine", "crates/basic-machine")}:
        raise ValueError("native manifest must declare both the industrial and policy crates")
    declared_hooks = {
        (hook.get("hook"), hook.get("hook_version")) for hook in native_manifest.get("injection", [])
    }
    if declared_hooks != {(hook, 1) for hook in HOOKS}:
        raise ValueError("native hook set changed")
    if parse_toml(content["horizon-mod.toml"], "content manifest").get("id") != MOD:
        raise ValueError("content owner differs from bundle identity")
    if native[f"mixins/{MOD}/LICENSE"].replace(b"\r\n", b"\n") != read("LICENSE").replace(b"\r\n", b"\n"):
        raise ValueError("package MIT license and native template notice differ")

    native["horizon-native.json"] = (
        json.dumps(
            {
                "schema": 1,
                "id": MOD,
                "version": package["version"],
                "game": package["game"],
                "horizon": HORIZON,
                "mixins": SDK,
            },
            indent=2,
        )
        + "\n"
    ).encode("utf-8")
    native[f"mixins/{MOD}/data/fixtures/compressor-reference-v1.json"] = fixture
    native[f"mixins/{MOD}/runtime.rs"] = runtime(read("support/adapter.rs"))

    registry_lines = [
        'serde = { version = "1", features = ["derive"] }',
        'serde_json = "=1.0.151"',
    ]
    industrial_root = f"mixins/{MOD}/crates/industrial-sim"
    policy_root = f"mixins/{MOD}/crates/basic-machine"
    industrial_manifest_bytes = manifest(crate_names["industrial_package"], industrial_manifest, registry_lines)
    policy_dependencies = [
        f'industrial-sim = {{ path = "../industrial-sim", package = "{crate_names["industrial_package"]}" }}',
        *registry_lines,
    ]
    native[f"{industrial_root}/Cargo.toml"] = industrial_manifest_bytes
    native[f"{policy_root}/Cargo.toml"] = manifest(crate_names["policy_package"], policy_manifest, policy_dependencies)
    native[f"{policy_root}/src/lib.rs"] = policy

    transforms: list[dict[str, object]] = []
    for relative_source in pack_inputs["industrial_source_files"]:
        path = f"{pack_inputs['industrial_source_dir']}/{relative_source}"
        original = read(path)
        packaged = original
        old: bytes | None = None
        expected = 0
        if relative_source == "src/machine.rs":
            old, expected = b'include_bytes!("../../../fixtures/industrial-v1.json")', 1
        elif relative_source == "src/logistics/tests.rs":
            old, expected = b'include_bytes!("../../../../fixtures/industrial-v1.json")', 2
        if old is not None:
            if original.count(old) != expected:
                raise ValueError("test fixture include count changed")
            packaged = original.replace(
                old,
                b'include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/industrial-v1.json"))',
            )
            transforms.append(
                {
                    "file": relative_source,
                    "test_fixture_includes_rewritten": expected,
                    "source_sha256": sha256(original),
                    "packaged_sha256": sha256(packaged),
                }
            )
        native[f"{industrial_root}/{relative_source}"] = packaged
    native[f"{industrial_root}/fixtures/industrial-v1.json"] = read(pack_inputs["industrial_fixture"])

    if any(not name.endswith((".json", ".toml", ".md", ".rs", "LICENSE")) for name in [*native, *content]):
        raise ValueError("unreviewed package file extension")
    if [(item["file"], item["test_fixture_includes_rewritten"]) for item in transforms] != [
        ("src/logistics/tests.rs", 2),
        ("src/machine.rs", 1),
    ]:
        raise ValueError("packaged source must contain exactly three test-only fixture path rewrites")
    validate_content(content, MOD)
    return native, content, transforms


def build_archives(data: dict[str, bytes]) -> tuple[dict[str, bytes], dict[str, bytes], list[dict[str, object]]]:
    lock = load_lock()
    native_entries, content_entries, transforms = assemble(data)
    native_bytes = zip_bytes(native_entries)
    content_bytes = zip_bytes(content_entries)
    package = lock["package"]
    if sha256(native_bytes) != package["native_archive_sha256"]:
        raise ValueError("native archive hash differs from the accepted assembly-v1 reference")
    if sha256(content_bytes) != package["content_archive_sha256"]:
        raise ValueError("content archive hash differs from the accepted assembly-v1 reference")
    return (
        {package["native_archive"]: native_bytes, package["content_archive"]: content_bytes},
        {"native": native_entries, "content": content_entries},
        transforms,
    )


def make_receipt(
    lock_hash: str,
    data: dict[str, bytes],
    artifacts: dict[str, bytes],
    entries: dict[str, dict[str, bytes]],
    transforms: list[dict[str, object]],
) -> bytes:
    lock = load_lock()
    package = lock["package"]
    receipt = {
        "schema": 1,
        "status": "PASS_SOURCE_ASSEMBLY_NOT_RUNTIME",
        "bundle_lock_sha256": lock_hash,
        "packager_sha256": sha256(Path(__file__).read_bytes()),
        "horizon_revision": HORIZON,
        "horizon_api": package["horizon_api"],
        "sdk_revision": SDK,
        "sdk_upstream_contribution": package["sdk_upstream_contribution"],
        "sdk_upstream_status": "draft contribution; merge and stock-launcher installation are not claimed",
        "determinism": {
            "entry_order": "lexicographically sorted POSIX paths",
            "timestamp": "1980-01-01T00:00:00",
            "compression": "ZIP_DEFLATED level 9",
            "unix_entry_mode": "regular file 0644",
            "input_line_endings": "preserved byte-for-byte",
        },
        "inputs": {path: sha256(value) for path, value in sorted(data.items())},
        "test_only_source_transforms": transforms,
        "adapter_transform": {
            "source": "support/adapter.rs",
            "archive_path": f"mixins/{MOD}/runtime.rs",
            "native_data_fixture_rewrites": 1,
            "cfg_test_child_module_removed": 1,
        },
        "archives": [
            {
                "name": name,
                "sha256": sha256(value),
                "entries": [
                    {"path": path, "sha256": sha256(member)}
                    for path, member in sorted(entries[label].items())
                ],
            }
            for label, name in (("native", package["native_archive"]), ("content", package["content_archive"]))
            for value in (artifacts[name],)
        ],
        "scope": [
            "One LV compressor evaluation machine and its fixture-only charge/trace commands.",
            "Source archives package the approved industrial-sim and basic-machine crates plus allowlisted templates.",
            "The content package references vanilla furnace assets and includes no textures or game binaries.",
        ],
        "limits": [
            "This receipt checks source/template allowlists, locked SHA-256 hashes, archive members, deterministic metadata, and reference archive hashes.",
            "It does not verify the whole pack, network power, chained-job scheduling, machine removal, or stock-launcher installation.",
            "Adapter runtime and game behavior are not verified by this packaging command.",
        ],
    }
    return (json.dumps(receipt, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def output_root(value: str) -> Path:
    requested = Path(value).expanduser()
    if not requested.is_absolute():
        requested = Path.cwd() / requested
    requested = Path(os.path.abspath(requested))
    reject_link_components(requested, "output path")
    if requested.parent != TOOL_ROOT or not requested.name.startswith("build-"):
        raise ValueError("--output-dir must be a fresh build-* child of the packaging source directory")
    if requested.exists() or is_link(requested):
        raise ValueError("output directory already exists; choose a fresh path to preserve existing files")
    return requested


def write_output(output_dir: Path, files: dict[str, bytes]) -> None:
    if output_dir.exists() or is_link(output_dir):
        raise ValueError("output directory appeared before writing; nothing was written")
    with tempfile.TemporaryDirectory(prefix=f".{output_dir.name}.staging-", dir=TOOL_ROOT) as temp_name:
        staging = Path(temp_name)
        for name, data in files.items():
            safe_name = relative(name, "output filename")
            target = staging.joinpath(*safe_name.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            with target.open("xb") as handle:
                handle.write(data)
                handle.flush()
                os.fsync(handle.fileno())
        if output_dir.exists() or is_link(output_dir):
            raise ValueError("output directory appeared before finalization; nothing was published")
        os.rename(staging, output_dir)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo-root",
        default=str(REPO_ROOT),
        help="repository root containing gtnh-runtime/ and native-foundation/",
    )
    parser.add_argument("--output-dir", required=True, help="fresh build-* child of this packaging source directory")
    args = parser.parse_args(argv)
    try:
        output_dir = output_root(args.output_dir)
        data, lock_hash = approved_inputs(args.repo_root)
        artifacts, archive_entries, transforms = build_archives(data)
        verified_data, verified_lock_hash = approved_inputs(args.repo_root)
        if data != verified_data or lock_hash != verified_lock_hash:
            raise ValueError("approved sources changed during assembly; no output was written")

        files: dict[str, bytes] = {}
        for label, members in archive_entries.items():
            for name, content in members.items():
                files[f"{label}/{name}"] = content
        files.update(artifacts)
        files[RECEIPT_NAME] = make_receipt(lock_hash, data, artifacts, archive_entries, transforms)
        write_output(output_dir, files)
    except (OSError, ValueError, zipfile.BadZipFile, KeyError, TypeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    print(
        json.dumps(
            {
                "status": "PASS_SOURCE_ASSEMBLY_NOT_RUNTIME",
                "output_dir": output_dir.name,
                "archives": {
                    name: sha256(content)
                    for name, content in sorted(artifacts.items())
                },
                "receipt": RECEIPT_NAME,
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
