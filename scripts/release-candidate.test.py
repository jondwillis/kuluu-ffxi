import os
from pathlib import Path
import shlex
import subprocess
import shutil
import textwrap
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('release-candidate.py').resolve()


class CandidateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.cwd = self.tmp.name
        self.git('init', '-q')
        self.git('config', 'user.email', 'test@example.invalid')
        self.git('config', 'user.name', 'Release test')
        self.git('-c', 'core.hooksPath=/dev/null', 'commit', '--allow-empty', '-qm', 'base')
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.git('tag', 'v0.7.0')

    def git(self, *args):
        env = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
        return subprocess.check_output(['git', *args], cwd=self.cwd, text=True, env=env)

    def run_candidate(self, tag='v0.7.0', sha=None, publish=True):
        return subprocess.run([sys.executable, str(SCRIPT), '--tag', tag,
            '--sha', sha or self.base, '--publish', str(publish).lower()],
            cwd=self.cwd, capture_output=True, text=True,
            env={k: v for k, v in os.environ.items() if not k.startswith('GIT_')})

    def test_workflow_event_context(self):
        workflow = SCRIPT.parent.parent / '.github/workflows/release.yml'
        body = workflow.read_text().split('        run: |\n', 1)[1].split('\n  # Gate', 1)[0]
        body = textwrap.dedent(body)
        scripts = Path(self.cwd) / 'scripts'
        scripts.mkdir()
        shutil.copyfile(SCRIPT, scripts / SCRIPT.name)
        cases = [
            ('push', 'branch', 'main', 'v0.7.0', True, 'v0.7.0', True),
            ('push', 'branch', 'release/v0.8.0', '', False, 'v0.8.0', False),
            ('push', 'branch', 'release/v0.8.0', '', True, 'v0.8.0', False),
            ('push', 'tag', 'v0.7.0', '', True, 'v0.7.0', True),
            ('workflow_dispatch', 'branch', 'main', 'v0.7.0', True, 'v0.7.0', True),
            ('workflow_dispatch', 'branch', 'release/v0.8.0', 'v0.8.0', False, 'v0.8.0', False),
        ]
        for event, ref_type, ref_name, tag, publish, expected_tag, expected_publish in cases:
            with self.subTest(event=event, ref=ref_name, publish=publish):
                output = Path(self.cwd) / 'workflow-output'
                output.write_text('')
                env = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
                # The body picks its interpreter off PATH unless PYTHON names one. A developer box
                # may have no runnable python on PATH for the shell to find (git-bash on Windows
                # resolves `python3` to an alias that fails to exec), so hand this script's own
                # interpreter down as the first line of the snippet - forward slashes, which git-
                # bash accepts. Assigning inside the command rather than via env keeps the pin
                # working in shells that filter variables out of a child environment.
                env.update(GITHUB_EVENT_NAME=event, GITHUB_REF_TYPE=ref_type,
                           GITHUB_REF_NAME=ref_name, GITHUB_SHA=self.base,
                           INPUT_TAG=tag, PUBLISH=str(publish).lower(), GITHUB_OUTPUT=str(output))
                py = sys.executable.replace('\\', '/')
                script = f"PYTHON={shlex.quote(py)}\n{body}"
                result = subprocess.run(['bash', '-eu', '-c', script], cwd=self.cwd,
                                        env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                values = dict(line.split('=', 1) for line in output.read_text().splitlines())
                self.assertEqual(values, dict(tag=expected_tag, sha=self.base,
                                              publish=str(expected_publish).lower()))

    def test_matching_tag(self):
        self.assertEqual(self.run_candidate().returncode, 0)

    def test_untagged_rehearsal_only(self):
        self.assertEqual(self.run_candidate('v0.8.0-rehearsal', publish=False).returncode, 0)
        self.assertNotEqual(self.run_candidate('v0.8.0-rehearsal').returncode, 0)

    def test_tag_at_other_commit_rejected(self):
        self.git('-c', 'core.hooksPath=/dev/null', 'commit', '--allow-empty', '-qm', 'next')
        current = self.git('rev-parse', 'HEAD').strip()
        self.assertNotEqual(self.run_candidate(sha=current).returncode, 0)
        self.assertNotEqual(self.run_candidate(sha=current, publish=False).returncode, 0)

    def test_workflow_checkout_mismatch(self):
        self.assertNotEqual(self.run_candidate(sha='a' * 40).returncode, 0)

    def test_bad_tag(self):
        for tag in ['main', 'v1.2.3junk', 'v01.2.3', 'v1.2.3\npublish=true']:
            self.assertNotEqual(self.run_candidate(tag, publish=False).returncode, 0)


if __name__ == '__main__':
    unittest.main()
