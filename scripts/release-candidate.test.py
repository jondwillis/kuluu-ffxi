import os
from pathlib import Path
import subprocess
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
