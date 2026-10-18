"""Check launcher boundaries without building packages or loading a model."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
LAUNCHER = ROOT / 'scripts' / 'run.sh'


class LauncherTest(unittest.TestCase):
    def test_passes_paths_with_spaces_and_stops_on_build_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            config = base / 'model config.toml'
            config.write_text('# fixture\n')
            log = base / 'calls.jsonl'
            guix = base / 'guix'
            guix.write_text('#!' + os.sys.executable + '\n' + '''import json, os, sys
with open(os.environ['CALLS'], 'a') as out:
    out.write(json.dumps({'args': sys.argv[1:], 'cwd': os.getcwd()}) + '\\n')
if 'build' in sys.argv and os.environ.get('FAIL_BUILD'):
    sys.exit(17)
''')
            guix.chmod(0o700)
            env = dict(os.environ, PATH=str(base) + os.pathsep + os.environ['PATH'], CALLS=str(log))
            result = subprocess.run([str(LAUNCHER), config.name, 'check'], cwd=base, env=env, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            self.assertEqual(len(calls), 2)
            self.assertIn('build', calls[0]['args'])
            self.assertIn('--locked', calls[0]['args'])
            self.assertEqual(calls[0]['cwd'], str(ROOT))
            self.assertEqual(calls[1]['args'][-3:], ['--config', str(config), 'check'])
            self.assertNotIn('-C', calls[1]['args'])
            self.assertNotIn('-CWNF', calls[1]['args'])
            log.unlink()
            result = subprocess.run([str(LAUNCHER), str(config)], env=dict(env, FAIL_BUILD='1'), capture_output=True)
            self.assertEqual(result.returncode, 17)
            self.assertEqual(len(log.read_text().splitlines()), 1)

    def test_missing_configuration_fails_with_setup_instructions(self):
        result = subprocess.run([str(LAUNCHER), '/nonexistent/lllmmm.toml'], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('cp -r', result.stderr)


if __name__ == '__main__':
    unittest.main()
