"""Exercise queue state transitions without requiring Blender on CI runners."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[2] / "blender_extension/assetstudio_bridge/__init__.py"


class QueueTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        handlers = types.ModuleType("bpy.app.handlers")
        handlers.persistent = lambda function: function
        bpy = types.ModuleType("bpy")
        bpy.types = types.SimpleNamespace(Operator=object, Panel=object)
        bpy.props = types.SimpleNamespace(StringProperty=lambda **kwargs: None)
        bpy.app = types.SimpleNamespace(handlers=handlers, version_string="queue test")
        bpy.context = types.SimpleNamespace(mode="OBJECT", scene=types.SimpleNamespace(name="Test"),
                                            selected_objects=[], view_layer=types.SimpleNamespace(objects=types.SimpleNamespace(active=None)))
        with patch.dict(sys.modules, {"bpy": bpy, "bpy.app.handlers": handlers}):
            spec = importlib.util.spec_from_file_location("queue_test_bridge", SOURCE)
            self.bridge = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(self.bridge)
        self.bridge.bridge_root = lambda: self.root
        self.bridge._enabled = True
        bpy.ops = types.SimpleNamespace(assetstudio=types.SimpleNamespace(
            receive=lambda directory: self.bridge.process_job(Path(directory))))
        self.imports = []

        def import_package(path):
            self.imports.append(path)
            return {"objects": 1, "materials": 1, "warnings": []}

        self.bridge.import_package = import_package

    def job(self, name, **request):
        job = self.root / "jobs" / name
        job.mkdir(parents=True)
        (job / "model.fbx").write_bytes(b"queue fixture")
        self.bridge.atomic_json(job / "request.ready.json", {
            "version": 1, "receiver": self.bridge.INSTANCE, "models": ["model.fbx"], **request})
        return job

    def test_invalid_protocol_and_models_are_claimed_and_replied_once(self):
        for index, request in enumerate([{"version": 2}, {"models": []}, {"models": "model.fbx"}, {"models": [1]}]):
            with self.subTest(request=request):
                job = self.job(f"invalid-{index}", **request)
                self.bridge.tick()
                self.assertFalse((job / "request.ready.json").exists())
                receipt = (job / "result.json").read_bytes()
                self.assertTrue(json.loads(receipt)["failed"])
                self.bridge.tick()
                self.assertEqual((job / "result.json").read_bytes(), receipt)
        self.assertFalse(self.imports)

    def test_malformed_request_does_not_block_a_valid_job_or_retry(self):
        broken = self.job("a-broken")
        (broken / "request.ready.json").write_text("{broken", encoding="utf-8")
        valid = self.job("b-valid")
        self.bridge.tick()
        self.assertFalse((broken / "request.ready.json").exists())
        self.assertTrue(json.loads((broken / "result.json").read_text(encoding="utf-8"))["failed"])
        receipt = (valid / "result.json").read_bytes()
        self.assertFalse(json.loads(receipt)["failed"])
        self.assertEqual(len(self.imports), 1)
        self.bridge.tick()
        self.assertEqual(len(self.imports), 1)
        self.assertEqual((valid / "result.json").read_bytes(), receipt)

    def test_foreign_requests_are_left_for_their_receiver(self):
        job = self.job("foreign", receiver="other-instance")
        self.bridge.tick()
        self.assertTrue((job / "request.ready.json").exists())
        self.assertFalse((job / "result.json").exists())
        self.assertFalse(self.imports)

    def test_operator_error_after_import_preserves_the_successful_receipt(self):
        job = self.job("completed")

        def receive(directory):
            self.bridge.process_job(Path(directory))
            raise RuntimeError("operator failed after writing the receipt")

        self.bridge.bpy.ops.assetstudio.receive = receive
        self.bridge.tick()
        receipt = json.loads((job / "result.json").read_text(encoding="utf-8"))
        self.assertEqual(len(receipt["imported"]), 1)
        self.assertFalse(receipt["failed"])
        self.assertEqual(len(self.imports), 1)

    def test_existing_file_outside_the_job_is_rejected(self):
        job = self.job("escape", models=["../outside.fbx"])
        (job.parent / "outside.fbx").write_bytes(b"outside the package")
        self.bridge.tick()
        receipt = json.loads((job / "result.json").read_text(encoding="utf-8"))
        self.assertIn("escapes", receipt["failed"][0])
        self.assertFalse(self.imports)


if __name__ == "__main__":
    unittest.main()
