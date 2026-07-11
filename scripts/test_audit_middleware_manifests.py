#!/usr/bin/env python3
"""Negative tests for deployment middleware policy enforcement."""

from __future__ import annotations

import copy
import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).with_name("audit-middleware-manifests.py")
SPEC = importlib.util.spec_from_file_location("audit_middleware_manifests", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class DeploymentPolicyNegativeTests(unittest.TestCase):
    """Ensure policy bypass and drift mutations are rejected."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.lock = MODULE.load("tests/middleware/components.lock.toml")
        cls.policy = MODULE.load("tests/middleware/deployment-policy.toml")

    def profile(self, policy: dict, name: str) -> dict:
        matches = [profile for profile in policy["profile"] if profile["name"] == name]
        self.assertEqual(len(matches), 1)
        return matches[0]

    def assert_policy_rejected(self, mutate) -> None:
        candidate = copy.deepcopy(self.policy)
        mutate(candidate)
        original_load = MODULE.load

        def load(relative: str) -> dict:
            if relative == "tests/middleware/deployment-policy.toml":
                return candidate
            return original_load(relative)

        MODULE.load = load
        try:
            with self.assertRaises(AssertionError):
                MODULE.audit_deployment_policy(self.lock)
        finally:
            MODULE.load = original_load

    def test_direct_production_terminal_bypass_is_rejected(self) -> None:
        def mutate(policy: dict) -> None:
            profile = self.profile(policy, "wasmtime-authn")
            profile["support"] = "production"
            profile["middleware_profile"] = "none"
            profile["middleware"] = []
            profile["artifact_sets"] = []

        self.assert_policy_rejected(mutate)

    def test_stack_order_drift_is_rejected(self) -> None:
        def mutate(policy: dict) -> None:
            profile = self.profile(policy, "wasmtime-authn")
            profile["middleware"][0], profile["middleware"][1] = (
                profile["middleware"][1],
                profile["middleware"][0],
            )

        self.assert_policy_rejected(mutate)

    def test_stack_omission_is_rejected(self) -> None:
        self.assert_policy_rejected(
            lambda policy: self.profile(policy, "wasmtime-authn")["middleware"].pop()
        )

    def test_authz_terminal_cannot_be_downgraded_to_authn_only(self) -> None:
        def mutate(policy: dict) -> None:
            profile = self.profile(policy, "wasmtime-authn-authz")
            profile["middleware_profile"] = "authn"
            profile["middleware"] = list(MODULE.EXPECTED_STACK)
            profile["artifact_sets"] = ["middleware"]

        self.assert_policy_rejected(mutate)

    def test_authz_terminal_cannot_be_swapped_for_an_unprotected_guest(self) -> None:
        self.assert_policy_rejected(
            lambda policy: self.profile(policy, "wasmtime-authn-authz").__setitem__(
                "terminal_artifact", "tests/test-app-p3.wasm"
            )
        )

    def test_authz_composed_output_and_alpha_support_cannot_drift(self) -> None:
        def mutate(policy: dict) -> None:
            profile = self.profile(policy, "wasmtime-authn-authz")
            profile["support"] = "production"
            profile["output_artifact"] = "tests/test-app-p3-middleware.wasm"

        self.assert_policy_rejected(mutate)

    def test_authz_profile_cannot_be_removed(self) -> None:
        self.assert_policy_rejected(
            lambda policy: policy.__setitem__(
                "profile",
                [
                    profile
                    for profile in policy["profile"]
                    if profile["name"] != "wasmtime-authn-authz"
                ],
            )
        )

    def test_public_exemption_overlap_is_rejected(self) -> None:
        self.assert_policy_rejected(
            lambda policy: self.profile(policy, "spin-counter-stable").pop(
                "public_exemption"
            )
        )

    def test_public_exemption_digest_drift_is_rejected(self) -> None:
        def mutate(policy: dict) -> None:
            profile = self.profile(policy, "spin-counter-stable")
            profile["public_exemption"][0]["source_digest"] = "sha256:" + "0" * 64

        self.assert_policy_rejected(mutate)

    def test_artifact_set_digest_drift_is_rejected(self) -> None:
        self.assert_policy_rejected(
            lambda policy: policy.__setitem__("artifact_set_sha256", "0" * 64)
        )

    def test_production_terminal_must_remain_private(self) -> None:
        self.assert_policy_rejected(
            lambda policy: self.profile(
                policy, "wasmtime-trusted-ingress-authz"
            ).__setitem__("private_terminal", False)
        )

    def test_production_authentication_mode_cannot_fall_back_implicitly(self) -> None:
        self.assert_policy_rejected(
            lambda policy: self.profile(
                policy, "wasmtime-trusted-ingress-authz"
            ).__setitem__("authentication_mode", "portable_component")
        )

    def test_production_ingress_omission_is_rejected(self) -> None:
        self.assert_policy_rejected(
            lambda policy: self.profile(
                policy, "wasmtime-trusted-ingress-authz"
            ).pop("ingress_manifest")
        )

    def test_production_terminal_public_listener_is_rejected(self) -> None:
        self.assert_policy_rejected(
            lambda policy: self.profile(
                policy, "wasmtime-trusted-ingress-authz"
            ).__setitem__("terminal_listener", "public")
        )

    def test_authorization_forwarding_is_rejected(self) -> None:
        self.assert_policy_rejected(
            lambda policy: self.profile(
                policy, "wasmtime-trusted-ingress-authz"
            ).__setitem__("forwards_authorization", True)
        )

    def test_trusted_response_metadata_is_rejected(self) -> None:
        self.assert_policy_rejected(
            lambda policy: self.profile(
                policy, "wasmtime-trusted-ingress-authz"
            ).__setitem__("allows_trusted_response_headers", True)
        )


if __name__ == "__main__":
    unittest.main()
