#!/usr/bin/env python3
import base64
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HOOKS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("verification", HOOKS / "runtime-verification.py")
verification = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verification)
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=")


class VerificationGateTests(unittest.TestCase):
    def setUp(self):
        environment = patch.dict(os.environ, {
            key: value for key, value in os.environ.items() if not key.startswith("GIT_")
        }, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.root = self.base / "repo"
        self.root.mkdir()
        self.env = {**os.environ, "TMPDIR": str(self.base / "tmp")}
        Path(self.env["TMPDIR"]).mkdir()
        self.previous_tmp = os.environ.get("TMPDIR")
        os.environ["TMPDIR"] = self.env["TMPDIR"]
        self.addCleanup(self.restore_environment)
        self.git("init", "-q")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "Verification test")
        self.source = self.root / "kuluu-render/src/hud/quantity.rs"
        self.source.parent.mkdir(parents=True)
        self.source.write_text("pub fn quantity() {}\n")
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")
        self.session = "visual-session"
        self.build = self.root / "client"
        self.build.write_bytes(b"fixture executable")
        self.payload = {"session_id": self.session, "cwd": str(self.root), "last_assistant_message": "Verified the quantity text."}

    def restore_environment(self):
        if self.previous_tmp is None:
            os.environ.pop("TMPDIR", None)
        else:
            os.environ["TMPDIR"] = self.previous_tmp

    def git(self, *arguments):
        return subprocess.run(["git", "-C", str(self.root), *arguments], check=True, capture_output=True, text=True)

    def edit(self):
        self.source.write_text("pub fn shadowed_quantity() {}\n")
        verification.track(self.session, self.root, [str(self.source)])

    def capture(self):
        self.build.write_bytes(b"fixture executable")
        image = self.root / "quantity.png"
        image.write_bytes(PNG)
        return image

    def record(self, *arguments, success=True):
        result = subprocess.run(
            [sys.executable, str(HOOKS / "runtime-verification.py"), "record", *arguments],
            cwd=self.root, env=self.env, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0 if success else 1, result.stderr)
        return result

    def record_pass(self, image):
        payload = {**self.payload, "tool_name": "view_image", "tool_input": {"path": str(image)}}
        result = subprocess.run([sys.executable, str(HOOKS / "codex-project-hook.py"), "PostToolUse"],
                                input=json.dumps(payload), env=self.env, capture_output=True, text=True)
        self.assertEqual(result.stdout, "", result.stderr)
        self.record("--verdict", "pass", "--summary", "Quantity is legible without a chip",
                    "--surface", "visual", "--inspection", "Inspected digits over bright item art",
                    "--artifact", str(image), "--build", str(self.build))

    def marker(self):
        return self.root / ".verify/latest.json"

    def test_commit_does_not_clear_the_requirement(self):
        self.edit()
        self.git("commit", "-qam", "quantity fix")
        self.assertIn("no runtime evidence", verification.check(self.payload))

    def test_fresh_inspected_capture_passes_after_commit(self):
        self.edit()
        self.git("commit", "-qam", "quantity fix")
        self.record_pass(self.capture())
        self.assertIsNone(verification.check(self.payload))

    def test_logs_cannot_verify_visual_changes(self):
        self.edit()
        log = self.root / "tests.log"
        log.write_text("All HUD tests pass\n")
        self.record("--verdict", "pass", "--summary", "Tests passed", "--surface", "session", "--artifact", str(log))
        self.assertIn("wrong runtime surface", verification.check(self.payload))
        self.record("--verdict", "pass", "--summary", "Tests passed", "--surface", "visual",
                    "--inspection", "A test log", "--artifact", str(log), "--build", str(self.build), success=False)

    def test_verified_removal_passes_after_commit(self):
        self.source.unlink()
        verification.track(self.session, self.root, [str(self.source)])
        self.git("commit", "-qam", "remove duplicated widget")
        self.record_pass(self.capture())
        self.assertIsNone(verification.check(self.payload))

    def test_uninspected_capture_is_rejected(self):
        self.edit()
        self.record("--verdict", "pass", "--summary", "Captured", "--surface", "visual",
                    "--inspection", "Claimed inspection", "--artifact", str(self.capture()), "--build", str(self.build), success=False)

    def test_old_capture_cannot_be_refreshed_by_recording_it_again(self):
        image = self.capture()
        self.edit()
        self.build.write_bytes(b"updated executable")
        self.record_pass(image)
        self.assertIn("no fresh visual capture", verification.check(self.payload))

    def test_old_executable_cannot_verify_current_rust(self):
        self.edit()
        image = self.root / "quantity.png"
        image.write_bytes(PNG)
        self.record_pass(image)
        self.assertIn("client executable predates", verification.check(self.payload))

    def test_post_verification_source_edit_is_rejected_even_without_a_new_ledger_entry(self):
        self.edit()
        self.record_pass(self.capture())
        self.source.write_text("pub fn different_quantity() {}\n")
        self.assertIn("current source", verification.check(self.payload))

    def test_modified_artifact_is_rejected(self):
        self.edit()
        image = self.capture()
        self.record_pass(image)
        image.write_bytes(PNG + b"changed")
        self.assertIn("artifact missing or changed", verification.check(self.payload))

    def test_failed_and_legacy_markers_do_not_pass(self):
        self.edit()
        self.record_pass(self.capture())
        marker = json.loads(self.marker().read_text())
        marker["verdict"] = "fail"
        self.marker().write_text(json.dumps(marker))
        self.assertIn("not pass", verification.check(self.payload))
        marker["schema_version"] = 1
        self.marker().write_text(json.dumps(marker))
        self.assertIn("old schema", verification.check(self.payload))

    def test_ui_waiver_requires_disclosed_user_authorization(self):
        self.edit()
        self.record("--verdict", "waived", "--summary", "Builds take time", success=False)
        self.record("--verdict", "waived", "--summary", "User opted out",
                    "--authorization", "Skip visual verification this time.")
        self.assertIn("authorization", verification.check(self.payload))
        self.payload["last_assistant_message"] = 'You authorized "Skip visual verification this time."'
        self.assertIsNone(verification.check(self.payload))

    def test_blocked_handoff_requires_a_diagnostic_and_explicit_incomplete_report(self):
        self.edit()
        log = self.root / "build-failure.log"
        log.write_text("error: UI build failed in dependency\n")
        reason = "The changed build cannot launch because its dependency does not compile."
        self.record("--verdict", "blocked", "--summary", reason, "--artifact", str(log))
        self.assertIn("disclosed as incomplete", verification.check(self.payload))
        self.payload["last_assistant_message"] = "Visual verification is blocked. " + reason
        self.assertIsNone(verification.check(self.payload))

    def test_repeated_stop_remains_blocked_beyond_dispatcher_depth_limit(self):
        self.edit()
        self.payload["stop_hook_active"] = True
        for _ in range(10):
            result = subprocess.run(["bash", str(HOOKS / "stop-dispatcher.sh")],
                                    input=json.dumps(self.payload), capture_output=True, text=True, env=self.env)
            self.assertEqual(json.loads(result.stdout)["decision"], "block")

    def test_absolute_patch_in_another_worktree_is_owned_by_that_worktree(self):
        worktree = self.base / "other-worktree"
        self.git("worktree", "add", "-q", "--detach", str(worktree))
        path = worktree / self.source.relative_to(self.root)
        path.write_text("pub fn shadowed_quantity() {}\n")
        payload = {**self.payload, "tool_name": "apply_patch", "tool_input": {"command": f"*** Begin Patch\n*** Update File: {path}\n*** End Patch"}}
        result = subprocess.run([sys.executable, str(HOOKS / "codex-project-hook.py"), "PostToolUse"],
                                input=json.dumps(payload), env=self.env, capture_output=True, text=True)
        self.assertEqual(result.stdout, "", result.stderr)
        self.assertIn(str(worktree), verification.check(self.payload))

    def test_codex_shell_attribution_uses_the_command_workdir(self):
        payload = {**self.payload, "cwd": str(self.base), "tool_name": "Bash",
                   "tool_input": {"command": "sed -i 's/quantity/shadowed_quantity/' kuluu-render/src/hud/quantity.rs", "workdir": str(self.root)}}
        for event in ("PreToolUse", "PostToolUse"):
            if event == "PostToolUse":
                self.source.write_text("pub fn shadowed_quantity() {}\n")
            result = subprocess.run([sys.executable, str(HOOKS / "codex-project-hook.py"), event],
                                    input=json.dumps(payload), env=self.env, capture_output=True, text=True)
            self.assertEqual(result.stdout, "", result.stderr)
        self.assertIn("quantity.rs", verification.check(self.payload))

    def test_non_runtime_edits_do_not_acquire_a_visual_gate(self):
        note = self.root / "README.md"
        note.write_text("contributor orientation\n")
        verification.track(self.session, self.root, [str(note)])
        self.assertIsNone(verification.check(self.payload))


class HookEnvironmentTests(unittest.TestCase):
    def test_fixture_does_not_mutate_the_hook_callers_repository(self):
        with tempfile.TemporaryDirectory() as directory:
            caller = Path(directory)
            clean = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
            subprocess.run(["git", "init", "-q", str(caller)], env=clean, check=True)
            config = caller / ".git/config"
            before = config.read_bytes()
            environment = {**clean, "GIT_DIR": str(caller / ".git"),
                           "GIT_WORK_TREE": str(caller), "GIT_INDEX_FILE": str(caller / ".git/index")}
            result = subprocess.run([
                sys.executable, str(Path(__file__).resolve()),
                "VerificationGateTests.test_commit_does_not_clear_the_requirement",
            ], env=environment, capture_output=True, text=True)
            self.assertEqual(config.read_bytes(), before)
            self.assertFalse((caller / ".git/index").exists())
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
