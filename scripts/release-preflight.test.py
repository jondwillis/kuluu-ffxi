import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('release-preflight.py')


class PreflightTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.state = self.root / 'state.json'
        self.snapshot = self.root / 'inputs.json'
        self.data = {'repo': 'jondwillis/kuluu-ffxi', 'main_sha': 'a' * 40,
                     'stable_sha': 'b' * 40, 'stable_tag': 'v0.7.0', 'release_updated_at': 'test'}
        self.snapshot.write_text(json.dumps(self.data))

    def run_probe(self, *extra, ok=True):
        result = subprocess.run([sys.executable, str(SCRIPT), '--state', str(self.state),
            '--snapshot', str(self.snapshot), '--date', '2026-10-08', *extra], capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, ok, result.stderr)
        return json.loads(result.stdout) if ok else result

    def acknowledge(self, probe):
        evidence = self.root / 'assessment.json'
        evidence.write_text(json.dumps({'key': probe['key'], 'decision': 'hold', 'summary': 'Missing runtime evidence'}))
        return self.run_probe('--ack', probe['key'], '--decision', 'hold', '--evidence', str(evidence))

    def test_interruption_does_not_suppress_work(self):
        self.assertEqual(self.run_probe()['status'], 'assessment-required')
        self.assertFalse(self.state.exists())
        self.assertEqual(self.run_probe()['status'], 'assessment-required')

    def test_acknowledged_hold_is_quiet_until_inputs_change(self):
        probe = self.run_probe()
        self.acknowledge(probe)
        self.assertEqual(self.run_probe()['status'], 'unchanged')
        self.data['main_sha'] = 'c' * 40
        self.snapshot.write_text(json.dumps(self.data))
        self.assertEqual(self.run_probe()['status'], 'assessment-required')
        self.assertFalse(self.run_probe()['publish_allowed'])

    def test_stale_ack_fails_without_ledger_write(self):
        probe = self.run_probe()
        self.data['main_sha'] = 'c' * 40
        self.snapshot.write_text(json.dumps(self.data))
        evidence = self.root / 'assessment.json'
        evidence.write_text(json.dumps({'key': probe['key'], 'decision': 'hold', 'summary': 'old'}))
        self.run_probe('--ack', probe['key'], '--decision', 'hold', '--evidence', str(evidence), ok=False)
        self.assertFalse(self.state.exists())

    def test_corrupt_state_is_not_replaced(self):
        self.state.write_text('broken')
        self.run_probe(ok=False)
        self.assertEqual(self.state.read_text(), 'broken')

    def test_weekly_scope_and_no_change(self):
        self.assertEqual(self.run_probe('--date', '2026-10-07')['scope'], 'weekly-features')
        self.data['main_sha'] = self.data['stable_sha']
        self.snapshot.write_text(json.dumps(self.data))
        self.assertEqual(self.run_probe()['status'], 'no-change')

    def test_new_validation_signals_reopen_hold(self):
        probe = self.run_probe()
        self.acknowledge(probe)
        signals = self.root / 'signals.json'
        signals.write_text(json.dumps({'runtime': 'new-evidence'}))
        self.assertEqual(self.run_probe('--signals', str(signals))['status'], 'assessment-required')

    def test_missing_assessment_reopens_candidate(self):
        self.acknowledge(self.run_probe())
        (self.root / 'assessment.json').unlink()
        self.assertEqual(self.run_probe()['status'], 'assessment-required')

    def test_failed_input_does_not_acknowledge(self):
        self.snapshot.write_text('{}')
        self.run_probe(ok=False)
        self.assertFalse(self.state.exists())


if __name__ == '__main__':
    unittest.main()
