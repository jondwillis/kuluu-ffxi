import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('check_readme', Path(__file__).with_name('check-readme.py'))
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class ReadmeGuard(unittest.TestCase):
    def test_length_cannot_hide_in_details(self):
        self.assertTrue(check.editorial_errors('<details>\n' + 'word ' * (check.MAX_WORDS + 1) + '\n</details>'))

    def test_badge_destinations_do_not_consume_budget(self):
        self.assertFalse(check.editorial_errors('[Download](https://example.org/' + 'x' * 20000 + ')'))

    def test_code_reference_and_task_ids(self):
        self.assertTrue(check.editorial_errors('```bash\n' + 'command\n' * (check.MAX_CODE_LINES + 1) + '```'))
        self.assertTrue(check.editorial_errors('Fixed kuluu-ab12.3'))
        self.assertFalse(check.editorial_errors('Run kuluu install get in kuluu-ffxi.'))

    def test_links_and_duplicate_heading_slugs(self):
        class Tree:
            def exists(self, path):
                return path == 'CONTRIBUTING.md'

            def read(self, path):
                return '# Build\n\n## Build\n'
        text = '[Build](CONTRIBUTING.md#build-1) <a href="CONTRIBUTING.md#missing">Bad</a> [Gone](missing.md)'
        errors = check.link_errors('README.md', text, Tree())
        self.assertEqual(len(errors), 2)
        self.assertIn('missing heading', errors[0])
        self.assertIn('missing local target', errors[1])

    def test_staged_content_cannot_be_masked_by_worktree(self):
        env = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
        script = Path(__file__).with_name('check-readme.py').resolve()
        with tempfile.TemporaryDirectory() as folder:
            subprocess.run(['git', 'init', '-q', folder], check=True, env=env)
            for guide in check.PUBLIC_GUIDES:
                path = Path(folder) / guide
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('A short guide.')
            path = Path(folder) / 'README.md'
            path.write_text('word ' * (check.MAX_WORDS + 1))
            subprocess.run(['git', '-C', folder, 'add', '.'], check=True, env=env)
            path.write_text('A short working copy.')
            staged = subprocess.run(
                [sys.executable, str(script), '--staged'], cwd=folder, env=env,
                capture_output=True, text=True,
            )
            self.assertEqual(staged.returncode, 1)
            self.assertIn('words exceeds', staged.stderr)
            working = subprocess.run(
                [sys.executable, str(script)], cwd=folder, env=env,
                capture_output=True, text=True,
            )
            self.assertEqual(working.returncode, 0, working.stderr)


if __name__ == '__main__':
    unittest.main()
