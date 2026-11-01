"""Source-only regression tests for the pinned LV compressor package builder."""

from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
import uuid
import zipfile
from pathlib import Path


TOOL_ROOT = Path(__file__).resolve().parent


def load_package_module(script_path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, script_path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


PACKAGE = load_package_module(TOOL_ROOT / "package_compressor.py", "lv_compressor_package")

NATIVE_NAME = "gtnh_lv_compressor_eval.hnmod"
CONTENT_NAME = "gtnh_lv_compressor_eval.hmod"
NATIVE_SHA256 = "d9cfe6d35bc82c7c2353c2b0018affc8d8cd8a0def0f79bcf696c8d48ab318c5"
CONTENT_SHA256 = "ab94a30985055ad725e43753997d415da0270cc7b99e4680a9bf3aa65812a9b6"
MOD = "gtnh_lv_compressor_eval"


class PackageTests(unittest.TestCase):
    def test_native_archive_uses_supported_crate_data_layout(self) -> None:
        data, _ = PACKAGE.approved_inputs()
        native, _, _ = PACKAGE.assemble(data)
        prefix = f"mixins/{MOD}/"
        manifest = PACKAGE.parse_toml(native[prefix + "horizon-mixin.toml"], "mixin manifest")
        self.assertEqual(
            {(item["id"], item["path"]) for item in manifest["crate"]},
            {("industrial_sim", "industrial-sim"), ("basic_machine", "basic-machine")},
        )
        # Horizon permits arbitrary data beneath a top-level crate's data/ directory.
        for name in native:
            if name == "horizon-native.json":
                continue
            rest = name.removeprefix(prefix)
            parts = rest.split("/")
            allowed_data = len(parts) >= 2 and (
                parts[0] == "data" or (len(parts) >= 3 and parts[1] == "data")
            )
            allowed_text = len(parts) <= 2 and (
                parts[-1].startswith(("README", "LICENSE", "LICENCE", "COPYING", "NOTICE"))
                or parts[-1] == ".gitignore"
            )
            self.assertTrue(name.startswith(prefix))
            self.assertTrue(rest.endswith((".rs", ".toml")) or allowed_data or allowed_text, name)
        self.assertEqual(
            native[prefix + "industrial-sim/data/industrial-v1.json"],
            data["native-foundation/fixtures/industrial-v1.json"],
        )
        for relative in ("src/machine.rs", "src/logistics/tests.rs"):
            self.assertIn(
                b'"/data/industrial-v1.json"',
                native[prefix + "industrial-sim/" + relative],
            )

    def test_archives_are_deterministic_and_keep_expected_manifests_and_members(self) -> None:
        data, _ = PACKAGE.approved_inputs()
        native, content, transforms = PACKAGE.assemble(data)
        native_bytes = PACKAGE.zip_bytes(native)
        content_bytes = PACKAGE.zip_bytes(content)
        artifacts, members, archive_transforms = PACKAGE.build_archives(data)

        self.assertEqual(PACKAGE.zip_bytes(native), native_bytes)
        self.assertEqual(PACKAGE.zip_bytes(content), content_bytes)
        self.assertEqual(hashlib.sha256(native_bytes).hexdigest(), NATIVE_SHA256)
        self.assertEqual(hashlib.sha256(content_bytes).hexdigest(), CONTENT_SHA256)
        self.assertEqual(set(artifacts), {NATIVE_NAME, CONTENT_NAME})
        self.assertEqual(artifacts[NATIVE_NAME], native_bytes)
        self.assertEqual(artifacts[CONTENT_NAME], content_bytes)
        self.assertEqual(members["native"], native)
        self.assertEqual(members["content"], content)
        self.assertEqual(archive_transforms, transforms)
        industrial_root = f"mixins/{MOD}/industrial-sim"
        expected_native = {
            "horizon-native.json",
            f"mixins/{MOD}/LICENSE",
            f"mixins/{MOD}/NOTICE.md",
            f"mixins/{MOD}/horizon-mixin.toml",
            f"mixins/{MOD}/runtime.rs",
            f"mixins/{MOD}/data/fixtures/compressor-reference-v1.json",
            f"mixins/{MOD}/basic-machine/Cargo.toml",
            f"mixins/{MOD}/basic-machine/src/lib.rs",
            f"{industrial_root}/Cargo.toml",
            f"{industrial_root}/data/industrial-v1.json",
            *(f"mixins/{MOD}/hooks/{hook}.rs" for hook in (
                "block_entity",
                "block_entity_ticks",
                "block_entity_container",
                "block_entity_tick",
                "screen",
                "screen_draw",
                "register_commands",
                "perform_command",
            )),
            *(f"{industrial_root}/{path}" for path in (
                "src/content.rs",
                "src/content/selection_tests.rs",
                "src/inventory.rs",
                "src/lib.rs",
                "src/logistics.rs",
                "src/logistics/tests.rs",
                "src/machine.rs",
            )),
        }
        expected_content = {
            "horizon-mod.toml",
            f"assets/{MOD}/blockstates/lv_compressor.json",
            f"assets/{MOD}/items/lv_compressor.json",
            f"assets/{MOD}/lang/en_us.json",
            f"assets/{MOD}/models/item/lv_compressor.json",
            f"data/{MOD}/horizon/block/lv_compressor.json",
            f"data/{MOD}/loot_table/blocks/lv_compressor.json",
            f"data/{MOD}/recipe/evaluation_lv_compressor.json",
        }
        self.assertEqual(set(native), expected_native)
        self.assertEqual(set(content), expected_content)
        self.assertIn("horizon-native.json", native)
        self.assertIn(f"mixins/{MOD}/runtime.rs", native)
        self.assertIn(f"mixins/{MOD}/industrial-sim/Cargo.toml", native)
        self.assertIn(f"mixins/{MOD}/basic-machine/Cargo.toml", native)
        self.assertIn("horizon-mod.toml", content)

        with zipfile.ZipFile(io.BytesIO(native_bytes)) as archive:
            self.assertEqual(archive.namelist(), sorted(native))
            native_manifest = json.loads(archive.read("horizon-native.json"))
            self.assertEqual(native_manifest["id"], MOD)
            self.assertEqual(native_manifest["horizon"], "681124a485b82a27ac4e769756a2f3a2356e6827")
            self.assertEqual(native_manifest["mixins"], "3ac63df26d332bc344ba645d5d23497b7d4ad985")
            mixin_manifest = PACKAGE.parse_toml(
                archive.read(f"mixins/{MOD}/horizon-mixin.toml"), "native mixin manifest"
            )
            self.assertEqual(
                {(item["id"], item["path"]) for item in mixin_manifest["crate"]},
                {("industrial_sim", "industrial-sim"), ("basic_machine", "basic-machine")},
            )
            industrial_manifest = PACKAGE.parse_toml(
                archive.read(f"mixins/{MOD}/industrial-sim/Cargo.toml"), "industrial-sim manifest"
            )
            policy_manifest = PACKAGE.parse_toml(
                archive.read(f"mixins/{MOD}/basic-machine/Cargo.toml"), "basic-machine manifest"
            )
            self.assertEqual(industrial_manifest["package"]["name"], "gtnh-lv-compressor-eval-industrial-sim")
            self.assertEqual(policy_manifest["package"]["name"], "gtnh-lv-compressor-eval-basic-machine")
            self.assertEqual(
                policy_manifest["dependencies"]["industrial-sim"],
                {
                    "path": "../industrial-sim",
                    "package": "gtnh-lv-compressor-eval-industrial-sim",
                },
            )
            runtime = archive.read(f"mixins/{MOD}/runtime.rs").decode("utf-8")
            self.assertEqual(runtime.count("horizon_client::native_data!"), 1)
            self.assertNotIn('include_bytes!("fixtures/compressor-reference-v1.json")', runtime)
            self.assertNotIn("#[cfg(test)]", runtime)
            self.assertNotIn("mod tests;", runtime)

        with zipfile.ZipFile(io.BytesIO(content_bytes)) as archive:
            self.assertEqual(PACKAGE.parse_toml(archive.read("horizon-mod.toml"), "content manifest")["id"], MOD)

        self.assertEqual(
            [(item["file"], item["test_fixture_includes_rewritten"]) for item in transforms],
            [("src/logistics/tests.rs", 2), ("src/machine.rs", 1)],
        )

    def test_tampered_pack_input_is_rejected_before_output_creation(self) -> None:
        original, _ = PACKAGE.approved_inputs()
        output = PACKAGE.TOOL_ROOT / f"build-test-corrupt-{uuid.uuid4().hex}"

        with tempfile.TemporaryDirectory(prefix="lv-compressor-tampered-") as temporary:
            repo_root = Path(temporary)
            for relative, content in original.items():
                if not PACKAGE.is_repo_input(relative):
                    continue
                target = repo_root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(content)
            changed = repo_root / "native-foundation/Cargo.toml"
            changed.write_bytes(changed.read_bytes() + b"\n# unauthorized change\n")

            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                result = PACKAGE.main(["--repo-root", str(repo_root), "--output-dir", str(output)])

        self.assertNotEqual(result, 0)
        self.assertIn("source changed after approval", stderr.getvalue())
        self.assertFalse(output.exists())

    def test_existing_output_is_preserved(self) -> None:
        output = PACKAGE.TOOL_ROOT / f"build-test-preserve-{uuid.uuid4().hex}"
        output.mkdir()
        sentinel = output / "sentinel.txt"
        sentinel.write_bytes(b"leave this output alone")
        try:
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                result = PACKAGE.main(["--output-dir", str(output)])
            self.assertNotEqual(result, 0)
            self.assertEqual(sentinel.read_bytes(), b"leave this output alone")
            self.assertEqual({path.name for path in output.iterdir()}, {"sentinel.txt"})
        finally:
            shutil.rmtree(output)

    def test_unsafe_relative_archive_paths_are_rejected(self) -> None:
        for path in ("../escape", "/absolute", "folder\\escape", "C:/drive"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                PACKAGE.relative(path)

    def test_linked_pack_input_is_rejected_when_symlink_creation_is_available(self) -> None:
        original, _ = PACKAGE.approved_inputs()
        with tempfile.TemporaryDirectory(prefix="lv-compressor-linked-") as temporary:
            repo_root = Path(temporary)
            for relative, content in original.items():
                if not PACKAGE.is_repo_input(relative):
                    continue
                target = repo_root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(content)
            linked = repo_root / "native-foundation/Cargo.toml"
            linked.unlink()
            referent = repo_root / "native-foundation/linked-cargo.toml"
            referent.write_bytes(original["native-foundation/Cargo.toml"])
            try:
                linked.symlink_to(referent)
            except (OSError, NotImplementedError) as exc:
                self.skipTest(f"symlink creation is unavailable: {exc}")
            with self.assertRaisesRegex(ValueError, "linked source input"):
                PACKAGE.approved_inputs(repo_root)

    def test_cli_builds_pinned_archives_from_an_arbitrarily_named_public_checkout(self) -> None:
        approved, _ = PACKAGE.approved_inputs()
        with tempfile.TemporaryDirectory(prefix="public-checkout-") as temporary:
            checkout_root = Path(temporary) / f"independent-repo-{uuid.uuid4().hex}"
            checkout_root.mkdir()
            for relative, content in approved.items():
                if not PACKAGE.is_repo_input(relative):
                    continue
                target = checkout_root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(content)

            isolated_tool = checkout_root / "tools" / "lv-compressor"
            shutil.copytree(
                PACKAGE.TOOL_ROOT,
                isolated_tool,
                ignore=shutil.ignore_patterns(".local", "__pycache__", "build-*", "*.pyc", "*.pyo"),
            )
            output = isolated_tool / f"build-portability-{uuid.uuid4().hex}"
            result = subprocess.run(
                [sys.executable, str(isolated_tool / "package_compressor.py"), "--output-dir", str(output)],
                cwd=checkout_root,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((checkout_root / "pack").exists())
            self.assertEqual(hashlib.sha256((output / NATIVE_NAME).read_bytes()).hexdigest(), NATIVE_SHA256)
            self.assertEqual(hashlib.sha256((output / CONTENT_NAME).read_bytes()).hexdigest(), CONTENT_SHA256)
            receipt = (output / "lv-compressor-package-receipt.json").read_text(encoding="utf-8")
            self.assertNotIn(str(PACKAGE.REPO_ROOT), receipt)
            self.assertNotIn(str(PACKAGE.TOOL_ROOT), receipt)


if __name__ == "__main__":
    unittest.main()
