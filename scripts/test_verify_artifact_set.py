#!/usr/bin/env python3
"""Negative unit tests for signed artifact-set validation."""

from __future__ import annotations

import importlib.util
import pathlib
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("verify-artifact-set.py")
SPEC = importlib.util.spec_from_file_location("verify_artifact_set", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ArtifactSetNegativeTests(unittest.TestCase):
    """Reject missing, extra, duplicate, and tampered declarations."""

    def test_missing_component_is_rejected(self) -> None:
        bundle = {"artifact": [{"component": "request-id"}]}
        with self.assertRaises(MODULE.ArtifactSetError):
            MODULE.validate_artifact_entries(bundle, ["request-id", "cors"])

    def test_extra_component_is_rejected(self) -> None:
        bundle = {
            "artifact": [
                {"component": "request-id"},
                {"component": "cors"},
            ]
        }
        with self.assertRaises(MODULE.ArtifactSetError):
            MODULE.validate_artifact_entries(bundle, ["request-id"])

    def test_duplicate_component_is_rejected(self) -> None:
        bundle = {
            "artifact": [
                {"component": "request-id"},
                {"component": "request-id"},
            ]
        }
        with self.assertRaises(MODULE.ArtifactSetError):
            MODULE.validate_artifact_entries(bundle, ["request-id"])

    def test_tampered_bundle_key_and_signature_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for name in ("manifest", "key", "signature"):
                path = root / name
                path.write_bytes(name.encode())
                digest = MODULE.sha256(path)
                path.write_bytes(b"tampered")
                with self.assertRaises(MODULE.ArtifactSetError):
                    MODULE.verify_digest(path, digest, name)


if __name__ == "__main__":
    unittest.main()
