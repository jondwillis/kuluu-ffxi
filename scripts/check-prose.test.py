#!/usr/bin/env python3
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ProseChecks(unittest.TestCase):
    def check_prose(self, text, *args):
        return subprocess.run([sys.executable, str(ROOT / "scripts/check-prose.py"), *args],
                              input=text, text=True, capture_output=True)

    def test_word_choice_and_clean_text(self):
        bad = self.check_prose("Utilize the tool in order to read the file.\n")
        self.assertEqual(bad.returncode, 1, bad.stderr)
        self.assertIn("KuluuSTE.WordChoice", bad.stdout)
        good = self.check_prose("Use the tool to read the file.\n")
        self.assertEqual(good.returncode, 0, good.stdout + good.stderr)

    def test_sentence_boundaries(self):
        for procedures, limit in ((False, 25), (True, 20)):
            args = ("--procedures",) if procedures else ()
            for words, expected in ((limit, 0), (limit + 1, 1)):
                with self.subTest(procedures=procedures, words=words):
                    result = self.check_prose(" ".join(["word"] * words) + ".\n", *args)
                    self.assertEqual(result.returncode, expected, result.stdout + result.stderr)

    def test_code_and_urls_are_preserved(self):
        result = self.check_prose("Use `utilize` here.\n\n```sh\nutilize in order to\n```\n\n[Read this](https://example.com/utilize).\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_reminder_events(self):
        for event in ("SessionStart", "UserPromptSubmit"):
            result = subprocess.run([sys.executable, str(ROOT / ".agents/hooks/ste100-reminder.py")],
                                    input=json.dumps({"hook_event_name": event}), text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            output = json.loads(result.stdout)["hookSpecificOutput"]
            self.assertEqual(output["hookEventName"], event)
            self.assertIn("ASD-STE100", output["additionalContext"])
            self.assertIn("scripts/check-prose.py", output["additionalContext"])
            self.assertNotIn("## What this is", output["additionalContext"])

    def test_adapter_is_idempotent_and_preserves_other_hooks(self):
        path = ROOT / ".agents/hooks/install-codex-project-hooks.py"
        spec = importlib.util.spec_from_file_location("installer", path)
        installer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(installer)
        original = {"command": "bd codex-hook SessionStart", "type": "command"}
        config = installer.configured({"hooks": {"SessionStart": [{"hooks": [original]}]}})
        once = json.dumps(config, sort_keys=True)
        self.assertEqual(once, json.dumps(installer.configured(config), sort_keys=True))
        self.assertEqual(config["hooks"]["SessionStart"][0]["hooks"][0], original)
        for adapter in (".codex/hooks.json", ".claude/settings.json"):
            hooks = json.loads((ROOT / adapter).read_text())["hooks"]
            for event in ("SessionStart", "UserPromptSubmit"):
                commands = [h["command"] for group in hooks[event] for h in group["hooks"]]
                self.assertEqual(sum("ste100-reminder.py" in command for command in commands), 1)


if __name__ == "__main__":
    unittest.main()
