import contextlib
import importlib.util
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "beads-github-publish.py"
SPEC = importlib.util.spec_from_file_location("beads_github_publish", SCRIPT)
publisher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publisher)


class DryRunTest(unittest.TestCase):
    def test_existing_projections_are_planned_without_mutations(self):
        beads = [
            {"id": "changed", "title": "Updated title", "status": "closed"},
            {"id": "reopened", "title": "Reopened", "status": "open"},
            {"id": "new", "title": "New", "status": "open"},
            {"id": "finished", "title": "Finished", "status": "closed"},
            {"id": "unchanged", "title": "Unchanged", "status": "open"},
        ]
        repo = "test/repo"
        issues = []
        for number, bead in enumerate((beads[0], beads[1], beads[4]), start=1):
            issues.append({
                "number": number,
                "title": bead["title"],
                "body": publisher.project_body(bead, repo),
                "state": "CLOSED" if bead["id"] == "reopened" else "OPEN",
                "labels": [
                    {"name": label} for label in publisher.bead_labels_to_gh(bead)
                ],
            })
        issues[0]["title"] = "Old title"
        issues[0]["body"] = "Old description\n" + publisher.MARKER.format(
            id="changed"
        )
        issues[0]["labels"] = [
            {"name": "status:missing"}, {"name": "hand-applied"}
        ]

        def read_only_gh(command, **kwargs):
            self.assertEqual(command[:3], ["gh", "issue", "list"])
            self.assertIn("all", command)
            self.assertTrue(kwargs["capture_output"])
            return subprocess.CompletedProcess(command, 0, json.dumps(issues))

        for argv, environment in [
            ([str(SCRIPT), "--repo", repo, "--all", "--dry-run"], {}),
            ([str(SCRIPT), "--repo", repo, "--all"], {"DRY_RUN": "1"}),
        ]:
            with (
                self.subTest(argv=argv, environment=environment),
                tempfile.TemporaryDirectory() as directory,
            ):
                export = Path(directory) / "issues.jsonl"
                export.write_text("\n".join(json.dumps(bead) for bead in beads))
                output = io.StringIO()
                with (
                    patch.object(publisher, "JSONL", export),
                    patch.object(publisher.sys, "argv", argv),
                    patch.dict(publisher.os.environ, environment, clear=True),
                    patch.object(
                        publisher.subprocess, "run", side_effect=read_only_gh
                    ) as executed,
                    contextlib.redirect_stdout(output),
                ):
                    self.assertEqual(publisher.main(), 0)

                executed.assert_called_once()
                plan = output.getvalue()
                self.assertIn("update: #1 changed Updated title", plan)
                self.assertIn("close:  #1 changed", plan)
                self.assertIn("reopen: #2 reopened", plan)
                self.assertIn("create: [status:missing] new New", plan)
                self.assertNotIn("create: [] changed", plan)
                self.assertNotIn("issue edit 3", plan)
                self.assertNotIn("--remove-label hand-applied", plan)
                self.assertIn(
                    "created=1 updated=1 closed=1 reopened=1 pruned=0 "
                    "skipped(closed,unpublished)=1",
                    plan,
                )
                self.assertIn("GitHub was not modified", plan)


class ScopedPublicationTest(unittest.TestCase):
    repo = "test/repo"

    def run_publisher(self, beads, args, *, issues=(), imported=None, dry=True, created_url=None, fail_close=False):
        executed = []
        imported = imported or {}

        def fake_run(command, **kwargs):
            executed.append(command)
            if command[:3] == ["gh", "issue", "list"]:
                return subprocess.CompletedProcess(command, 0, json.dumps(issues))
            if command[:3] == ["gh", "issue", "view"]:
                value = imported[command[3]]
                if isinstance(value, Exception):
                    raise value
                return subprocess.CompletedProcess(command, 0, json.dumps(value))
            self.assertFalse(dry, f"dry run executed mutation: {command}")
            if command[:3] == ["gh", "issue", "create"]:
                url = created_url if created_url is not None else f"https://github.com/{self.repo}/issues/42"
                return subprocess.CompletedProcess(command, 0, url + "\n")
            if command[:3] == ["gh", "issue", "close"] and fail_close:
                raise subprocess.CalledProcessError(1, command)
            self.assertIn(command[:3], (["gh", "label", "create"], ["gh", "issue", "close"], ["gh", "issue", "edit"], ["gh", "issue", "reopen"]))
            return subprocess.CompletedProcess(command, 0, "")

        with tempfile.TemporaryDirectory() as directory:
            export = Path(directory) / "review-snapshot.jsonl"
            export.write_text("\n".join(json.dumps(bead) for bead in beads))
            unrelated_export = Path(directory) / "checkout.jsonl"
            unrelated_export.write_text(json.dumps({"id": "wrong-snapshot", "title": "Wrong", "status": "open"}))
            argv = [str(SCRIPT), "--repo", self.repo, "--source-export", str(export), *args]
            if dry:
                argv.append("--dry-run")
            output, errors = io.StringIO(), io.StringIO()
            with (
                patch.object(publisher, "JSONL", unrelated_export),
                patch.object(publisher.sys, "argv", argv),
                patch.dict(publisher.os.environ, {}, clear=True),
                patch.object(publisher.subprocess, "run", side_effect=fake_run),
                contextlib.redirect_stdout(output),
                contextlib.redirect_stderr(errors),
            ):
                try:
                    result = publisher.main()
                except SystemExit as error:
                    result = error.code
        return result, output.getvalue(), errors.getvalue(), executed

    def test_exact_ids_bypass_roadmap_and_ignore_other_snapshot_and_labels(self):
        beads = [
            {"id": "alpha", "title": "Alpha", "status": "open"},
            {"id": "beta", "title": "Beta", "status": "open"},
            {"id": "unrelated", "title": "Unrelated", "status": "open", "labels": ["roadmap", "unrelated-area"]},
        ]
        result, output, _, commands = self.run_publisher(beads, ["--id", "beta", "--id", "alpha", "--id", "alpha"])
        self.assertEqual(result, 0)
        self.assertIn("scope=selected IDs alpha,beta  count=2", output)
        self.assertIn("created=2", output)
        self.assertNotIn("Unrelated", output)
        self.assertNotIn("unrelated-area", output)
        self.assertNotIn("wrong-snapshot", output)
        self.assertEqual(len(commands), 1)

    def test_missing_or_ambiguous_selected_id_fails_before_any_github_call(self):
        for beads in [[], [{"id": "selected"}, {"id": "selected"}]]:
            with self.subTest(beads=beads):
                result, _, errors, commands = self.run_publisher(beads, ["--id", "selected"])
                self.assertEqual(result, 1)
                self.assertIn("exactly once", errors)
                self.assertEqual(commands, [])

    def test_unsafe_scope_combinations_fail_before_any_github_call(self):
        for args in (["--id", "a", "--all"], ["--id", "a", "--prune-unmarked"], ["--include-closed"]):
            with self.subTest(args=args):
                result, _, _, commands = self.run_publisher([], args)
                self.assertEqual(result, 2)
                self.assertEqual(commands, [])

    def test_selected_closed_backfill_creates_then_closes_only_that_issue(self):
        beads = [
            {"id": "finished", "title": "Finished", "status": "closed"},
            {"id": "unrelated", "title": "Unrelated", "status": "open"},
        ]
        confirmation = {"number": 42, "url": f"https://github.com/{self.repo}/issues/42", "body": publisher.MARKER.format(id="finished")}
        result, output, _, commands = self.run_publisher(beads, ["--id", "finished", "--include-closed"], dry=False, imported={"42": confirmation})
        self.assertEqual(result, 0)
        self.assertEqual(commands[1][:3], ["gh", "issue", "create"])
        self.assertEqual(commands[1][commands[1].index("--title") + 1], "Finished")
        self.assertEqual(commands[2][:4], ["gh", "issue", "view", "42"])
        self.assertEqual(commands[3], ["gh", "issue", "close", "42", "--repo", self.repo])
        self.assertEqual(len(commands), 4)
        self.assertIn("created=1 updated=0 closed=1", output)
        self.assertIn(f"published: finished https://github.com/{self.repo}/issues/42", output)

    def test_closed_backfill_dry_run_plans_both_operations_without_writes(self):
        result, output, _, commands = self.run_publisher([{"id": "finished", "title": "Finished", "status": "closed"}], ["--id", "finished", "--include-closed"])
        self.assertEqual(result, 0)
        self.assertIn("issue create", output)
        self.assertIn("issue close <new issue URL>", output)
        self.assertIn("created=1 updated=0 closed=1", output)
        self.assertEqual(len(commands), 1)

    def test_default_global_and_selected_closed_skip_remain_unchanged(self):
        for args in (["--all"], ["--id", "finished"]):
            with self.subTest(args=args):
                result, output, _, commands = self.run_publisher([{"id": "finished", "title": "Finished", "status": "closed"}], args)
                self.assertEqual(result, 0)
                self.assertIn("created=0", output)
                self.assertIn("skipped(closed,unpublished)=1", output)
                self.assertEqual(len(commands), 1)

    def test_imported_issue_is_verified_without_duplicate_or_overwrite(self):
        bead = {"id": "imported", "title": "Do not overwrite", "status": "closed", "external_ref": "gh-17", "labels": ["unrelated-area"]}
        issue = {"number": 17, "title": "Contributor title", "body": publisher.MARKER.format(id="imported"), "state": "OPEN", "labels": []}
        for dry in (True, False):
            with self.subTest(dry=dry):
                result, output, _, commands = self.run_publisher([bead], ["--id", "imported", "--include-closed"], issues=[issue], imported={"17": {"number": 17, "url": f"https://github.com/{self.repo}/issues/17"}}, dry=dry)
                self.assertEqual(result, 0)
                self.assertIn("contributor issue; not modified", output)
                self.assertIn("created=0 updated=0 closed=0", output)
                self.assertTrue(all(command[:3] in (["gh", "issue", "view"], ["gh", "issue", "list"]) for command in commands))

    def test_bad_import_fails_before_mutating_even_other_selected_bead(self):
        beads = [
            {"id": "ordinary", "title": "Ordinary", "status": "open"},
            {"id": "imported", "title": "Imported", "external_ref": "gh-17"},
        ]
        bad_issues = [
            {"number": 18, "url": f"https://github.com/{self.repo}/issues/17"},
            {"number": 17, "url": "https://github.com/other/repo/issues/17"},
            subprocess.CalledProcessError(1, ["gh", "issue", "view", "17"]),
        ]
        for issue in bad_issues:
            with self.subTest(issue=issue):
                result, _, errors, commands = self.run_publisher(beads, ["--id", "ordinary", "--id", "imported"], imported={"17": issue}, dry=False)
                self.assertEqual(result, 1)
                self.assertIn("error:", errors)
                self.assertEqual(len(commands), 1)
                self.assertEqual(commands[0][:3], ["gh", "issue", "view"])

    def test_bulk_skips_imported_refs_without_querying_or_reconciling_them(self):
        result, output, _, commands = self.run_publisher([{"id": "imported", "title": "Imported", "external_ref": "gh-17"}], ["--all"], dry=False)
        self.assertEqual(result, 0)
        self.assertIn("skip 1 imported GitHub references", output)
        self.assertEqual(len(commands), 1)
        self.assertEqual(commands[0][:3], ["gh", "issue", "list"])

    def test_pruning_preserves_imported_issue_even_outside_roadmap_scope(self):
        beads = [{"id": "imported", "title": "Imported", "external_ref": "gh-17"}]
        issue = {"number": 17, "title": "Contributor title", "body": "Unmarked contributor description", "state": "OPEN", "labels": []}
        result, output, _, commands = self.run_publisher(beads, ["--prune-unmarked"], issues=[issue], dry=False)
        self.assertEqual(result, 0)
        self.assertIn("pruned=0", output)
        self.assertEqual(len(commands), 2)
        self.assertTrue(all(command[:3] == ["gh", "issue", "list"] for command in commands))

    def test_selected_open_publication_executes_no_unrelated_issue_mutations(self):
        beads = [
            {"id": "selected", "title": "Selected", "status": "open"},
            {"id": "unrelated", "title": "Unrelated", "status": "closed", "labels": ["unrelated-area"]},
        ]
        issue = {"number": 19, "title": "Old unrelated title", "body": publisher.MARKER.format(id="unrelated"), "state": "OPEN", "labels": []}
        result, output, _, commands = self.run_publisher(beads, ["--id", "selected"], issues=[issue], dry=False)
        self.assertEqual(result, 0)
        self.assertEqual([command[:3] for command in commands], [["gh", "issue", "list"], ["gh", "label", "create"], ["gh", "issue", "create"]])
        self.assertNotIn("unrelated-area", output)
        self.assertIn(f"published: selected https://github.com/{self.repo}/issues/42", output)

    def test_invalid_creation_url_never_closes_an_issue(self):
        bead = {"id": "finished", "title": "Finished", "status": "closed"}
        for url in ("", "not-a-url", "https://github.com/other/repo/issues/42", "https://example.com/test/repo/issues/42", "https://github.com/test/repo/issues/0", "https://github.com/test/repo/pull/42"):
            with self.subTest(url=url):
                result, _, errors, commands = self.run_publisher([bead], ["--id", "finished", "--include-closed"], dry=False, created_url=url)
                self.assertEqual(result, 1)
                self.assertIn("issue URL does not match", errors)
                self.assertFalse(any(command[:3] == ["gh", "issue", "close"] for command in commands))

    def test_mismatched_created_identity_or_marker_never_closes_an_issue(self):
        bead = {"id": "finished", "title": "Finished", "status": "closed"}
        valid = {"number": 42, "url": f"https://github.com/{self.repo}/issues/42", "body": publisher.MARKER.format(id="finished")}
        for changed in ({"number": 43}, {"url": f"https://github.com/{self.repo}/issues/43"}, {"body": publisher.MARKER.format(id="unrelated")}):
            with self.subTest(changed=changed):
                result, _, errors, commands = self.run_publisher([bead], ["--id", "finished", "--include-closed"], dry=False, imported={"42": {**valid, **changed}})
                self.assertEqual(result, 1)
                self.assertIn("created issue does not match", errors)
                self.assertFalse(any(command[:3] == ["gh", "issue", "close"] for command in commands))

    def test_interrupted_close_recovers_existing_issue_without_duplicate(self):
        bead = {"id": "finished", "title": "Finished", "status": "closed"}
        url = f"https://github.com/{self.repo}/issues/42"
        issue = {"number": 42, "url": url, "title": "Finished", "body": publisher.project_body(bead, self.repo), "state": "OPEN", "labels": []}
        result, _, _, _ = self.run_publisher([bead], ["--id", "finished", "--include-closed"], dry=False, imported={"42": issue}, fail_close=True)
        self.assertEqual(result, 1)
        result, output, _, commands = self.run_publisher([bead], ["--id", "finished", "--include-closed"], issues=[issue], dry=False)
        self.assertEqual(result, 0)
        self.assertIn("created=0 updated=0 closed=1", output)
        self.assertEqual(commands[-1], ["gh", "issue", "close", "42", "--repo", self.repo])
        self.assertFalse(any(command[:3] == ["gh", "issue", "create"] for command in commands))

    def test_imported_number_protects_issue_with_another_bead_marker(self):
        beads = [
            {"id": "original", "title": "Would overwrite", "status": "closed"},
            {"id": "imported", "title": "Imported", "external_ref": "gh-17"},
        ]
        issue = {"number": 17, "title": "Contributor title", "body": publisher.MARKER.format(id="original"), "state": "OPEN", "labels": []}
        result, output, _, commands = self.run_publisher(beads, ["--id", "original"], issues=[issue], dry=False)
        self.assertEqual(result, 0)
        self.assertIn("imported issue; not modified", output)
        self.assertEqual(len(commands), 1)
        self.assertEqual(commands[0][:3], ["gh", "issue", "list"])


if __name__ == "__main__":
    unittest.main()
