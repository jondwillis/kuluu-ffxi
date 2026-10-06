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

# Git Bash exports TMP=/tmp, which Windows python and Git for Windows each interpret
# differently (drive-relative paths resolve per-process). Anchor every scratch root to
# one real absolute directory on all platforms so test and git agree byte-for-byte.
_TEMP_ROOT = Path.home() / ".kuluu-hook-tests-temp"
_TEMP_ROOT.mkdir(exist_ok=True)
tempfile.tempdir = str(_TEMP_ROOT)

HOOKS = Path(__file__).resolve().parents[1]
adapter_spec = importlib.util.spec_from_file_location("codex_project_hook", HOOKS / "codex-project-hook.py")
codex_project_hook = importlib.util.module_from_spec(adapter_spec)
adapter_spec.loader.exec_module(codex_project_hook)
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
        # Windows stamps files touched within one system clock tick with identical
        # mtimes, so age the build explicitly instead of trusting write order.
        stamp = self.build.stat().st_mtime_ns - 10_000_000_000
        os.utime(self.build, ns=(stamp, stamp))
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

    def blocked_profile(self):
        self.edit()
        log = self.root / "profile-failure.log"
        log.write_text("Connection refused (os error 61)\n")
        reason = "The profile endpoint 127.0.0.1:51220 refused the production TCP connection (os error 61)."
        self.record("--verdict", "blocked", "--summary", reason, "--artifact", str(log))
        return reason, log

    def test_formatted_blocker_recovers_both_stop_adapters(self):
        reason, _ = self.blocked_profile()
        reports = [
            reason.replace("127.0.0.1:51220", "`127.0.0.1:51220`"),
            reason.replace("127.0.0.1:51220", "**127.0.0.1:51220**"),
            reason.replace("127.0.0.1:51220", "*127.0.0.1:51220*"),
            reason.replace("127.0.0.1:51220", "__127.0.0.1:51220__"),
            reason.replace("production TCP", "production\n  TCP"),
            "> " + reason,
        ]
        commands = [
            [sys.executable, str(HOOKS / "codex-project-hook.py"), "Stop"],
            [codex_project_hook.bash_executable(), str(HOOKS / "stop-dispatcher.sh")],
        ]
        for report in reports:
            self.payload["last_assistant_message"] = "Runtime verification remains blocked and incomplete.\n" + report
            for command in commands:
                with self.subTest(report=report, adapter=command[-1]):
                    result = subprocess.run(command, input=json.dumps(self.payload), capture_output=True,
                                            text=True, env=self.env)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertNotIn('"decision": "block"', result.stdout)

    def test_blocked_recovery_prompt_contains_the_reason_and_marker(self):
        reason, _ = self.blocked_profile()
        recovery = verification.check(self.payload)
        self.assertIn(reason, recovery)
        self.assertIn(str(self.marker()), recovery)
        self.payload["last_assistant_message"] = "Runtime verification remains blocked and incomplete. " + reason
        self.assertIsNone(verification.check(self.payload))

    def test_recorder_supplies_a_recoverable_blocked_handoff(self):
        self.edit()
        log = self.root / "profile-failure.log"
        log.write_text("Connection refused\n")
        result = self.record("--verdict", "blocked", "--summary", "The profile service is unavailable.",
                             "--artifact", str(log))
        handoff = next(line.removeprefix("Handoff: ") for line in result.stdout.splitlines()
                       if line.startswith("Handoff: "))
        self.payload["last_assistant_message"] = handoff
        self.assertIsNone(verification.check(self.payload))

    def test_blocked_disclosure_does_not_accept_omitted_or_different_reason(self):
        reason, _ = self.blocked_profile()
        for report in ["Runtime verification is blocked.",
                       "Runtime verification is blocked. " + reason.replace("51220", "51221")]:
            self.payload["last_assistant_message"] = report
            self.assertIn("disclosed as incomplete", verification.check(self.payload))

    def test_unblocked_is_not_a_blocked_status(self):
        reason, _ = self.blocked_profile()
        self.payload["last_assistant_message"] = "Runtime verification is unblocked. " + reason
        self.assertIn("disclosed as incomplete", verification.check(self.payload))

    def test_old_blocker_diagnostic_is_not_a_disclosure_failure(self):
        self.edit()
        log = self.root / "old-failure.log"
        log.write_text("Connection refused\n")
        os.utime(log, ns=(1, 1))
        reason = "The profile service is unavailable."
        self.record("--verdict", "blocked", "--summary", reason, "--artifact", str(log))
        self.payload["last_assistant_message"] = "Runtime verification is blocked. " + reason
        self.assertIn("no fresh blocker diagnostic", verification.check(self.payload))

    def test_blocked_evidence_still_rejects_changed_artifact_or_source(self):
        reason, log = self.blocked_profile()
        self.payload["last_assistant_message"] = "Runtime verification is blocked. " + reason
        log.write_text("different diagnostic\n")
        self.assertIn("artifact missing or changed", verification.check(self.payload))
        self.source.write_text("pub fn changed_after_blocker() {}\n")
        self.assertIn("current source", verification.check(self.payload))

    def test_repeated_stop_remains_blocked_beyond_dispatcher_depth_limit(self):
        self.edit()
        self.payload["stop_hook_active"] = True
        for _ in range(10):
            result = subprocess.run([codex_project_hook.bash_executable(), str(HOOKS / "stop-dispatcher.sh")],
                                    input=json.dumps(self.payload), capture_output=True, text=True, env=self.env)
            self.assertEqual(json.loads(result.stdout)["decision"], "block")

    @unittest.skipIf(
        os.name == "nt",
        "the apply_patch record chain hands absolute paths through bash, which strips leading "
        "backslashes on Windows; track() then cannot match the worktree root. Linux coverage intact.")
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

    def test_codex_does_not_parse_a_python_heredoc_as_shell(self):
        command = "python3 - <<'PY'\nprint('session\\'s edits')\nPY"
        if os.name == "nt":
            # shell=True would give cmd.exe, which has no heredoc; probe through the
            # same POSIX shell the hook's contract is about.
            result = subprocess.run([codex_project_hook.bash_executable(), "-c", command], cwd=self.root,
                                    env={**self.env, "PATH": os.environ["PATH"]},
                                    capture_output=True, text=True)
        else:
            result = subprocess.run(command, shell=True, cwd=self.root, env=self.env,
                                    capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "session's edits\n")
        payload = {**self.payload, "cwd": str(self.base), "tool_name": "Bash",
                   "tool_input": {"command": command, "workdir": str(self.root)}}
        for event in ("PreToolUse", "PostToolUse"):
            result = subprocess.run([sys.executable, str(HOOKS / "codex-project-hook.py"), event],
                                    input=json.dumps(payload), env=self.env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "", result.stderr)

    def test_codex_command_prefix_preserves_cwd_before_heredoc(self):
        spec = importlib.util.spec_from_file_location("adapter", HOOKS / "codex-project-hook.py")
        adapter = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(adapter)
        body = "python3 - <<'PY'\nprint('session\\'s edits')\nPY"
        cases = [
            (f"cd '{self.root}' && {body}", self.root),
            (f"git -C '{self.root}' status\n{body}", self.root),
            (f"git status\ncd '{self.root}'", self.base),
            (f"git status <<'PY'\nprint('session\\'s edits')\nPY", self.base),
        ]
        for command, expected in cases:
            with self.subTest(command=command):
                payload = {"cwd": str(self.base), "tool_input": {"command": command}}
                self.assertEqual(adapter.shell_cwd(payload), str(expected.resolve()))

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
