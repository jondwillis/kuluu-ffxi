import importlib.util
import json
import base64
import hashlib
import io
import contextlib
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('macos_observe', Path(__file__).with_name('macos.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class MacosPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir='/tmp')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        quiet = contextlib.redirect_stdout(io.StringIO())
        quiet.__enter__()
        self.addCleanup(quiet.__exit__, None, None, None)
        self.image = self.root / 'screen.png'
        self.image.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\0' * 8 + struct.pack('>II', 800, 600))
        self.args = type('Args', (), dict(image=self.image, character='Test', vision=False, result=None))()

    def test_corrupt_archive_installs_nothing(self):
        archive = self.root / 'bad.zip'
        archive.write_bytes(b'not the official archive')
        with patch.dict(os.environ, XDG_CACHE_HOME=str(self.root / 'cache')):
            with self.assertRaisesRegex(ValueError, 'checksum'):
                m.install_ahk(archive)
        self.assertEqual(list(self.root.rglob('*.exe')), [])

    def startup_config(self, enabled=True):
        return dict(runner='/test/wine', loader='/test/loader.exe', loader_args=[], server='127.0.0.1',
                    startup_enter=m.startup_profile('127.0.0.1', enabled), prefix='/test/prefix')

    def test_startup_profile_requires_explicit_loopback_opt_in(self):
        self.assertFalse(m.startup_profile('127.0.0.1', False)['enabled'])
        enabled = m.startup_profile('127.0.0.1', True)
        self.assertEqual(enabled['action'], 'enter')
        self.assertEqual(enabled['caller_policy'], 'permitted')
        for host in ['example.com', '192.168.1.4', '127.0.0.1.example.com']:
            with self.subTest(host=host), self.assertRaises(ValueError):
                m.startup_profile(host, True)

    def test_setup_persists_the_explicit_startup_choice(self):
        runner = self.root / 'wine';runner.write_bytes(b'runner');runner.chmod(0o700)
        prefix = self.root / 'prefix';prefix.mkdir();(prefix / 'system.reg').touch()
        install = self.root / 'install';install.mkdir();(install / 'FFXiMain.dll').write_bytes(b'client')
        loader = self.root / 'loader.exe';loader.write_bytes(b'loader')
        ahk = self.root / 'ahk.exe';ahk.write_bytes(b'ahk')
        args = type('Args', (), dict(replace=False, runner=str(runner), prefix=str(prefix), install=str(install),
                                     loader=str(loader), server='127.0.0.1', loader_arg=[], startup_enter=True,
                                     ahk_archive=None, jev_key_file=str(self.root / 'key')))()
        profile = self.root / 'startup-profile.json'
        with patch.dict(m.os.environ, XDG_CACHE_HOME=str(self.root / 'cache')), \
                patch.object(m, 'install_ahk', return_value=ahk), \
                patch.object(m, 'run', return_value=type('Result', (), dict(stdout='Wine test'))()):
            m.setup(args, profile)
        saved = json.loads(profile.read_text())
        self.assertTrue(saved['startup_enter']['enabled'])
        self.assertEqual(saved['startup_enter']['caller_policy'], 'permitted')
        self.assertEqual(saved['startup_enter']['action'], 'enter')
        self.assertEqual(profile.stat().st_mode & 0o777, 0o600)

    def test_startup_enter_uses_persisted_choice_without_jev(self):
        config = self.startup_config()
        config['loader_args'] = ['--hairpin', '--authport', '57432', '--viewport', '57003', '--dataport', '57433']
        args = type('Args', (), dict(agreement_policy=None))()
        process = type('Process', (), dict(pid=123, poll=lambda self: None))()
        target = dict(id=456, pid=123, name='FINAL FANTASY XI', w=800, h=600)
        with patch.dict(m.os.environ, FFXI_USER='private-user', FFXI_PASS='private-password'), \
                patch.object(m, 'unlocked'), patch.object(m, 'environment', return_value={}), \
                patch.object(m, 'window', side_effect=[None, target, target]), \
                patch.object(m.subprocess, 'Popen', return_value=process) as spawn, \
                patch.object(m.time, 'monotonic', side_effect=[0, 0, m.STARTUP_ENTER_DELAY_SECONDS]), \
                patch.object(m.time, 'sleep'), patch.object(m, 'key') as key, patch.object(m, 'jev_decide') as jev:
            output = m.launch(config, args)
        spawn.assert_called_once()
        self.assertEqual(spawn.call_args.args[0][2:2 + len(config['loader_args'])], config['loader_args'])
        self.assertEqual(spawn.call_args.args[0][-6:-4], ['--server', '127.0.0.1'])
        key.assert_called_once_with(config, 'enter', m.DEFAULT_HOLD_SECONDS)
        jev.assert_not_called()
        self.assertEqual(output['status'], 'submitted')
        self.assertEqual(output['count'], 1)
        self.assertNotIn('private-password', json.dumps(output))
        self.assertNotIn('private-user', json.dumps(output))

    def test_startup_policy_override_launches_without_input(self):
        for policy, status in [('confirmation_required', 'policy_required'), ('caller', 'caller_policy_required')]:
            args = type('Args', (), dict(agreement_policy=policy))()
            with self.subTest(policy=policy), patch.dict(m.os.environ, FFXI_USER='test', FFXI_PASS='private'), \
                    patch.object(m, 'unlocked'), patch.object(m, 'environment', return_value={}), \
                    patch.object(m.os, 'chdir'), patch.object(m.os, 'execve') as execute, \
                    patch.object(m.subprocess, 'Popen') as spawn, patch.object(m, 'key') as key:
                output = m.launch(self.startup_config(), args)
            execute.assert_called_once()
            spawn.assert_not_called()
            key.assert_not_called()
            self.assertEqual(output['status'], status)
            self.assertTrue(output['startup_enter_skipped'])

    def test_existing_profile_without_startup_choice_keeps_normal_launch(self):
        config = self.startup_config();del config['startup_enter']
        with patch.dict(m.os.environ, FFXI_USER='test', FFXI_PASS='private'), \
                patch.object(m, 'unlocked'), patch.object(m, 'environment', return_value={}), \
                patch.object(m.os, 'chdir'), patch.object(m.os, 'execve') as execute, \
                patch.object(m, 'key') as key, patch.object(m, 'window') as window:
            m.launch(config, type('Args', (), {})())
        execute.assert_called_once()
        key.assert_not_called()
        window.assert_not_called()

    def test_existing_window_prevents_startup_input_and_duplicate_launch(self):
        with patch.dict(m.os.environ, FFXI_USER='test', FFXI_PASS='private'), \
                patch.object(m, 'unlocked'), patch.object(m, 'environment', return_value={}), \
                patch.object(m, 'window', return_value=dict(id=1)), \
                patch.object(m.subprocess, 'Popen') as spawn, patch.object(m, 'key') as key, self.assertRaises(ValueError):
            m.launch(self.startup_config(), type('Args', (), dict(agreement_policy=None))())
        spawn.assert_not_called()
        key.assert_not_called()

    def test_startup_rejects_actual_destination_overrides(self):
        for server, arguments in [('example.com', []), ('127.0.0.1', ['--server', 'example.com']),
                                  ('127.0.0.1', ['--server=example.com']), ('127.0.0.1', ['--unknown'])]:
            config = self.startup_config();config.update(server=server, loader_args=arguments)
            with self.subTest(server=server, arguments=arguments), \
                    patch.dict(m.os.environ, FFXI_USER='test', FFXI_PASS='private'), \
                    patch.object(m, 'unlocked'), patch.object(m, 'environment', return_value={}), \
                    patch.object(m.subprocess, 'Popen') as spawn, patch.object(m.os, 'execve') as execute, \
                    patch.object(m, 'key') as key, self.assertRaises(ValueError):
                m.launch(config, type('Args', (), dict(agreement_policy=None))())
            spawn.assert_not_called()
            execute.assert_not_called()
            key.assert_not_called()
        with self.assertRaises(ValueError):
            m.startup_profile('127.0.0.1', True, ['--server', 'example.com'])

    def test_actual_local_profile_port_arguments_are_supported(self):
        arguments = ['--hairpin', '--authport', '57432', '--viewport', '57003', '--dataport', '57433']
        saved = m.startup_profile('127.0.0.1', True, arguments)
        self.assertEqual(saved['loader_options'], {'--hairpin': True, '--authport': 57432,
                                                  '--viewport': 57003, '--dataport': 57433})
        options = m.validate_startup_loader_args(arguments + ['--lang', 'EU', '--profileport', '51220', '--hide'])
        self.assertEqual(options['--lang'], 'EU')
        self.assertEqual(options['--profileport'], 51220)
        self.assertTrue(options['--hide'])

    def test_startup_loader_argument_validation_rejects_ambiguity(self):
        for arguments in [['--authport'], ['--authport', '0'], ['--authport', str(m.MAX_TCP_PORT + 1)],
                          ['--dataport', '-1'], ['--viewport', '1.5'], ['--lang', 'unknown'],
                          ['--authport', '1234', '--authport', '4321'], ['--hairpin', '--hairpin'],
                          ['--json', 'remote.json'], ['--json-file=remote.json'],
                          ['--server', '127.0.0.1'], ['--trust']]:
            with self.subTest(arguments=arguments), self.assertRaises(ValueError):
                m.validate_startup_loader_args(arguments)

    def test_startup_wait_and_window_change_never_send_input(self):
        process = type('Process', (), dict(poll=lambda self: None))()
        for windows, times in [([None], [0, 0, m.STARTUP_WINDOW_TIMEOUT_SECONDS]),
                               ([dict(id=1), dict(id=2)], [0, 0, m.STARTUP_ENTER_DELAY_SECONDS]),
                               ([dict(id=1)], [0, 0, m.STARTUP_WINDOW_TIMEOUT_SECONDS])]:
            with self.subTest(windows=windows), patch.object(m, 'window', side_effect=windows), \
                    patch.object(m.time, 'monotonic', side_effect=times), patch.object(m.time, 'sleep'), \
                    patch.object(m, 'key') as key, self.assertRaises(ValueError):
                m.submit_startup_enter(self.startup_config(), process)
            key.assert_not_called()

    def test_profile_name_cannot_escape_configuration_directory(self):
        with self.assertRaises(ValueError):
            m.profile_path('../other')

    def test_environment_removes_legacy_profile_and_background_settings(self):
        config = dict(prefix='/prefix', runner='/wine', ahk='/ahk', window_title='FFXI')
        with patch.dict(os.environ, FFXI_OBSERVE_PROFILE='wrong', FFXI_OBSERVE_BG='1', HXI_BG='1'):
            result = m.environment(config)
        self.assertNotIn('FFXI_OBSERVE_PROFILE', result)
        self.assertNotIn('FFXI_OBSERVE_BG', result)
        self.assertNotIn('HXI_BG', result)
        self.assertEqual(result['WINEPREFIX'], '/prefix')

    def test_key_rejects_invalid_input_before_host_access(self):
        for name, duration in [('enter', 'nan'), ('enter', '-1'), ('enter', '6'), ('no-such-key', '.2')]:
            with self.subTest(name=name, duration=duration), patch.object(m, 'unlocked') as unlocked:
                with self.assertRaises(ValueError):
                    m.key({}, name, duration)
                unlocked.assert_not_called()

    def test_wheel_rejects_invalid_input_before_host_access(self):
        for direction, count in [('left', 1), ('up', 0), ('down', m.MAX_WHEEL_COUNT + 1), ('up', 1.5), ('up', True)]:
            with self.subTest(direction=direction, count=count), patch.object(m, 'unlocked') as unlocked:
                with self.assertRaises(ValueError):
                    m.wheel_input({}, direction, count)
                unlocked.assert_not_called()

    def test_wheel_passes_bounded_count_to_shared_input(self):
        with patch.object(m, 'wine_input') as submit:
            m.wheel_input({}, 'up', m.MAX_WHEEL_COUNT)
        submit.assert_called_once_with({}, ['wheel', 'up', m.MAX_WHEEL_COUNT])

    def test_restore_uses_shared_lock_without_sending_a_key(self):
        with patch.object(m, 'wine_input') as submit:
            m.restore_window({})
        submit.assert_called_once_with({}, ['restore', 'visible', 'only'], allow_missing_window=True)

    def test_drag_rejects_invalid_displacement_and_duration_before_host_access(self):
        requests = [(0, 0, '1'), (m.MAX_DRAG_PIXELS + 1, 0, '1'), (1.5, 0, '1'),
                    (True, 0, '1'), (1, 0, 'nan'), (1, 0, 'inf'), (1, 0, '0'),
                    (1, 0, '-1'), (1, 0, str(m.MAX_KEY_HOLD_SECONDS + 1)), (1, 0, '0.00001')]
        for dx, dy, seconds in requests:
            with self.subTest(dx=dx, dy=dy, seconds=seconds), patch.object(m, 'unlocked') as unlocked:
                with self.assertRaises(ValueError):
                    m.drag_input({}, dx, dy, seconds)
                unlocked.assert_not_called()

    def test_drag_passes_bounded_displacement_and_duration(self):
        with patch.object(m, 'wine_input') as submit:
            m.drag_input({}, 120, -30, '1')
        submit.assert_called_once_with({}, ['drag', 120, -30, m.MILLISECONDS_PER_SECOND])

    def test_text_rejects_unsafe_files_and_control_characters(self):
        source = self.root / 'text.txt'
        source.write_text('hello')
        source.chmod(0o644)
        with self.assertRaises(ValueError):
            m.literal_text({}, source)
        source.chmod(0o600)
        for data in [b'', b'hello\n', b'hello\r', b'hello\t', b'hello\x00', b'\xff', b'x' * (m.MAX_TEXT_BYTES + 1)]:
            source.write_bytes(data)
            with self.subTest(data=data[:20]), patch.object(m, 'wine_input') as submit:
                with self.assertRaises(ValueError):
                    m.literal_text({}, source)
                submit.assert_not_called()

    def test_text_passes_literal_utf8_without_logging_the_payload(self):
        source = self.root / 'text.txt'
        payload = 'hello {Enter} ! ^ + # ` " \\ é'
        source.write_text(payload, encoding='utf-8')
        source.chmod(0o600)
        output = io.StringIO()
        with patch.object(m, 'wine_input') as submit, contextlib.redirect_stdout(output):
            m.literal_text({}, source)
        submit.assert_called_once_with({}, [], payload.encode('utf-8'))
        self.assertNotIn(payload, output.getvalue())
        self.assertTrue(json.loads(output.getvalue())['submitted'])

    def test_text_standard_input_has_the_same_bounds(self):
        for data in [b'hello', b'x' * (m.MAX_TEXT_BYTES + 1)]:
            stream = type('Input', (), dict(buffer=io.BytesIO(data)))()
            with patch.object(m.sys, 'stdin', stream), patch.object(m, 'wine_input') as submit:
                if len(data) > m.MAX_TEXT_BYTES:
                    with self.assertRaises(ValueError):
                        m.literal_text({}, '-')
                    submit.assert_not_called()
                else:
                    m.literal_text({}, '-')
                    submit.assert_called_once_with({}, [], data)

    def test_shared_input_uses_private_payload_file_and_removes_it(self):
        config = dict(prefix='/prefix', runner='/wine', ahk='/ahk', window_title='FFXI')
        payload = b'hello {Enter} ! ^'
        files = []
        def execute(arguments, **kwargs):
            if arguments[1] == 'winepath.exe':
                return type('Result', (), dict(stdout=str(arguments[-1])))()
            self.assertEqual(arguments[-3], 'text')
            self.assertEqual(arguments[-1], 'literal')
            path = Path(arguments[-2])
            self.assertEqual(path.read_bytes(), payload)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertNotIn(payload.decode(), arguments)
            files.append(path)
        with patch.object(m, 'unlocked'), patch.object(m, 'window'), \
                patch.object(m, 'environment', return_value={}), \
                patch.object(m, 'profile_path', return_value=self.root / 'input.lock'), \
                patch.object(m, 'run', side_effect=execute):
            m.wine_input(config, [], payload)
        self.assertEqual(len(files), 1)
        self.assertFalse(files[0].exists())

    def test_shared_input_refuses_another_prefix_driver(self):
        config = dict(prefix='/prefix', runner='/wine', ahk='/ahk', window_title='FFXI')
        with patch.object(m, 'unlocked'), patch.object(m, 'window'), \
                patch.object(m, 'profile_path', return_value=self.root / 'input.lock'), \
                patch.object(m.fcntl, 'flock', side_effect=BlockingIOError), \
                patch.object(m, 'run') as execute:
            with self.assertRaisesRegex(ValueError, 'Another packaged input'):
                m.wine_input(config, ['wheel', 'up', 1])
            execute.assert_not_called()

    def test_key_file_must_be_private(self):
        key = self.root / 'key'
        key.write_text('test-key')
        key.chmod(0o644)
        with self.assertRaises(ValueError):
            m.credential(key)
        key.chmod(0o600)
        self.assertEqual(m.credential(key), 'test-key')

    def test_missing_key_http_timeout_and_bad_answer_request_vision(self):
        errors = [(m.DecisionFailure('credential_error', 'Missing private credential.'), 'credential_error'),
                  (m.DecisionFailure('api_error', 'HTTP 503'), 'api_error'),
                  (TimeoutError(), 'api_error'), (KeyError('answers'), 'schema_error')]
        for error, expected in errors:
            with self.subTest(error=type(error)), patch.object(m, 'jev_decide', side_effect=error):
                m.decide({}, self.args)
                result = json.loads(self.image.with_suffix('.decision.json').read_text())
                self.assertEqual(result['status'], expected)
                self.assertEqual(result['image_sha256'], m.digest(self.image))
                self.assertIn('diagnostic', result)

    def resize_stub(self, arguments, **kwargs):
        Path(arguments[-1]).write_bytes(Path(arguments[3]).read_bytes())

    def test_malformed_jev_json_requests_vision(self):
        payloads = [None, [], {'answers': None}, {'answers': []},
                    {'answers': {'next_key': None}}, {'answers': {'screen': [], 'next_key': {}}}]
        config = dict(jev_model=m.JEV_MODEL, jev_key_file='/not-read')
        for payload in payloads:
            with self.subTest(payload=payload), patch.object(m, 'run', side_effect=self.resize_stub), \
                    patch.object(m, 'credential', return_value='test'), \
                    patch.object(m.urllib.request, 'urlopen', return_value=io.StringIO(json.dumps(payload))):
                m.decide(config, self.args)
                result = json.loads(self.image.with_suffix('.decision.json').read_text())
                self.assertEqual(result['status'], 'schema_error')

    def test_capture_replacement_cannot_relabel_an_old_decision(self):
        original = self.image.read_bytes()
        submitted = []
        def response(request, **kwargs):
            body = json.loads(request.data)
            submitted.append(base64.b64decode(body['assets']['game-screen']['data_base64']))
            self.image.write_bytes(original + b'new frame')
            return io.StringIO(json.dumps({'answers': {
                'screen': {'choice': 'character_select', 'confidence': 1},
                'next_key': {'choice': 'enter', 'confidence': 1}}}))
        config = dict(jev_model=m.JEV_MODEL, jev_key_file='/not-read')
        with patch.object(m, 'run', side_effect=self.resize_stub), \
                patch.object(m, 'credential', return_value='test'), \
                patch.object(m.urllib.request, 'urlopen', side_effect=response):
            m.decide(config, self.args)
        result = json.loads(self.image.with_suffix('.decision.json').read_text())
        self.assertEqual(submitted, [original])
        self.assertEqual(result['status'], 'capture_changed')
        raw = json.loads(self.image.with_suffix('.jev.json').read_text())
        self.assertEqual(raw['image_sha256'], hashlib.sha256(original).hexdigest())
        self.assertNotEqual(raw['image_sha256'], result['image_sha256'])

    def test_valid_jev_response_keeps_its_capture_hash(self):
        payload = {'answers': {'screen': {'choice': 'gameplay', 'confidence': 1},
                               'next_key': {'choice': 'stop', 'confidence': 1}}}
        config = dict(jev_model=m.JEV_MODEL, jev_key_file='/not-read')
        with patch.object(m, 'run', side_effect=self.resize_stub), \
                patch.object(m, 'credential', return_value='test'), \
                patch.object(m.urllib.request, 'urlopen', return_value=io.StringIO(json.dumps(payload))):
            m.decide(config, self.args)
        result = json.loads(self.image.with_suffix('.decision.json').read_text())
        self.assertEqual(result['provider'], 'jev')
        self.assertEqual(result['decision']['image_sha256'], m.digest(self.image))

    def test_low_confidence_jev_answer_requests_vision(self):
        response = dict(seconds=.1, image_sha256=m.digest(self.image), response=dict(answers={
            'screen': dict(choice='gameplay', confidence=.7),
            'next_key': dict(choice='stop', confidence=1)}))
        with patch.object(m, 'jev_decide', return_value=response):
            m.decide({}, self.args)
        result = json.loads(self.image.with_suffix('.decision.json').read_text())
        self.assertEqual(result['status'], 'low_confidence')

    def test_image_request_header_assets_and_reference(self):
        payload = {'answers': {'screen': {'choice': 'gameplay', 'confidence': 1},
                               'next_key': {'choice': 'stop', 'confidence': 1}}}
        def response(request, **kwargs):
            body = json.loads(request.data)
            headers = {key.lower(): value for key, value in request.header_items()}
            self.assertEqual(headers['typesafe-danger-zone-images'], 'true')
            asset = body['assets'][body['state']['screen_image']]
            self.assertEqual(asset['type'], 'image')
            self.assertEqual(asset['format'], 'png')
            self.assertEqual(base64.b64decode(asset['data_base64']), self.image.read_bytes())
            return io.StringIO(json.dumps(payload))
        with patch.object(m, 'run', side_effect=self.resize_stub), \
                patch.object(m, 'credential', return_value='private-test-key'), \
                patch.object(m.urllib.request, 'urlopen', side_effect=response):
            output = m.decide(dict(jev_model=m.JEV_MODEL, jev_key_file='/not-read'), self.args)
        self.assertEqual(output['status'], 'decision')
        self.assertNotIn('private-test-key', json.dumps(output))

    def test_agreement_perception_is_independent_of_caller_policy(self):
        response = dict(seconds=.1, image_sha256=m.digest(self.image), response=dict(answers={
            'screen': dict(choice='agreement', confidence=1),
            'next_key': dict(choice='enter', confidence=1)}))
        for policy, status in [('permitted', 'decision'), ('confirmation_required', 'policy_required'),
                               ('caller', 'caller_policy_required')]:
            self.args.agreement_policy = policy
            with self.subTest(policy=policy), patch.object(m, 'jev_decide', return_value=response):
                output = m.decide({}, self.args)
            self.assertEqual(output['status'], status)
            self.assertEqual(output['decision']['screen'], 'agreement')
            self.assertEqual(output['decision']['next_key'], 'enter')
            self.assertEqual(output['provider'], 'jev')

    def test_invalid_confidence_is_schema_error_not_low_confidence(self):
        for confidence in [None, '0.8', float('nan'), True]:
            response = dict(seconds=.1, image_sha256=m.digest(self.image), response=dict(answers={
                'screen': dict(choice='dialogue', confidence=confidence),
                'next_key': dict(choice='enter', confidence=1)}))
            with self.subTest(confidence=confidence), patch.object(m, 'jev_decide', return_value=response):
                output = m.decide({}, self.args)
            self.assertEqual(output['status'], 'schema_error')
            self.assertIn('confidence', output['diagnostic'].lower())

    def test_forced_vision_never_calls_jev(self):
        self.args.vision = True
        with patch.object(m, 'jev_decide') as jev:
            m.decide({}, self.args)
            jev.assert_not_called()

    def test_decision_rejects_wrong_image_and_unsafe_enter(self):
        result = dict(image_sha256=m.digest(self.image), screen='gameplay', next_key='stop', confidence=1)
        self.assertEqual(m.validate_decision(result, self.image), result)
        for changes in [dict(image_sha256='wrong'), dict(next_key='enter'), dict(confidence=float('nan')),
                        dict(screen='character_select', next_key='enter', confidence=.4)]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                m.validate_decision({**result, **changes}, self.image)

    def test_dialogue_policy_cannot_select_choices_or_login_prompts(self):
        result = dict(image_sha256=m.digest(self.image), screen='dialogue', next_key='enter', confidence=1)
        self.assertEqual(m.validate_decision(result, self.image, 'dialogue'), result)
        with self.assertRaises(ValueError):
            m.validate_decision(result, self.image)
        for screen in ('choice', 'agreement', 'gameplay', 'error', 'other', 'character_confirm'):
            with self.subTest(screen=screen), self.assertRaises(ValueError):
                m.validate_decision({**result, 'screen': screen}, self.image, 'dialogue')
        with self.assertRaises(ValueError):
            m.validate_decision({**result, 'confidence': .5}, self.image, 'dialogue')
        typing = {**result, 'screen': 'dialogue_typing', 'next_key': 'wait'}
        self.assertEqual(m.validate_decision(typing, self.image, 'dialogue'), typing)
        with self.assertRaises(ValueError):
            m.validate_decision(typing, self.image)

    def test_dialogue_jev_request_preserves_explicit_policy(self):
        self.args.task = 'dialogue'
        submitted = []
        def response(request, **kwargs):
            body = json.loads(request.data)
            submitted.append(body)
            return io.StringIO(json.dumps({'answers': {
                'screen': {'choice': 'dialogue', 'confidence': 1},
                'next_key': {'choice': 'enter', 'confidence': 1}}}))
        config = dict(jev_model=m.JEV_MODEL, jev_key_file='/not-read')
        with patch.object(m, 'run', side_effect=self.resize_stub), \
                patch.object(m, 'credential', return_value='test'), \
                patch.object(m.urllib.request, 'urlopen', side_effect=response):
            m.decide(config, self.args)
        saved = json.loads(self.image.with_suffix('.decision.json').read_text())
        self.assertEqual(saved['task'], 'dialogue')
        self.assertEqual(saved['decision']['screen'], 'dialogue')
        self.assertEqual(submitted[0]['state']['task'], 'dialogue')
        self.assertIn('choice', submitted[0]['questions']['screen']['criteria'])
        self.assertIn('next_key', submitted[0]['questions'])

    def test_dialogue_screen_policy_stops_on_choices_and_low_confidence(self):
        self.args.task = 'dialogue'
        config = dict(jev_model=m.JEV_MODEL, jev_key_file='/not-read')
        for screen, confidence, status, action in [('choice', 1, 'decision', 'stop'),
                                                  ('dialogue', .5, 'low_confidence', None),
                                                  ('dialogue', 1, 'decision', 'enter')]:
            payload = {'answers': {'screen': {'choice': screen, 'confidence': confidence}}}
            with self.subTest(screen=screen, confidence=confidence), \
                    patch.object(m, 'run', side_effect=self.resize_stub), \
                    patch.object(m, 'credential', return_value='test'), \
                    patch.object(m.urllib.request, 'urlopen', return_value=io.StringIO(json.dumps(payload))):
                m.decide(config, self.args)
            saved = json.loads(self.image.with_suffix('.decision.json').read_text())
            self.assertEqual(saved['status'], status)
            if action:
                self.assertEqual(saved['decision']['next_key'], action)

    def dialogue_driver_stubs(self, screen='dialogue', status='decision'):
        def capture(config, destination):
            destination.write_bytes(self.image.read_bytes())
            return destination
        def decide(config, args):
            output = dict(status=status, decision=dict(image_sha256=m.digest(args.image),
                          screen=screen, next_key='enter' if screen == 'dialogue' else 'stop', confidence=1))
            args.image.with_suffix('.decision.json').write_text(json.dumps(output))
        return capture, decide

    def test_dialogue_driver_stops_after_two_unchanged_outcomes(self):
        args = type('Args', (), dict(folder=self.root / 'drive', character='Test', max_actions=10, max_seconds=10))()
        capture, decide = self.dialogue_driver_stubs()
        with patch.object(m, 'capture', side_effect=capture), patch.object(m, 'decide', side_effect=decide), \
                patch.object(m, 'dialogue_fingerprint', return_value='same'), patch.object(m, 'key') as key, \
                patch.object(m.time, 'sleep') as settle:
            m.drive_dialogue({}, args)
        self.assertEqual(key.call_count, m.MAX_UNCHANGED_DIALOGUE_OUTCOMES)
        self.assertEqual(settle.call_count, key.call_count)
        summary = json.loads((args.folder / 'summary.json').read_text())
        self.assertEqual(summary['reason'], 'unchanged_dialogue')
        self.assertEqual(len(summary['captures']), key.call_count + 1)

    def test_dialogue_driver_stops_on_choice_or_vision_request(self):
        for screen, status in [('choice', 'decision'), ('dialogue', 'low_confidence')]:
            args = type('Args', (), dict(folder=self.root / status, character='Test', max_actions=10, max_seconds=10))()
            capture, decide = self.dialogue_driver_stubs(screen, status)
            with patch.object(m, 'capture', side_effect=capture), patch.object(m, 'decide', side_effect=decide), \
                    patch.object(m, 'key') as key:
                m.drive_dialogue({}, args)
            key.assert_not_called()

    def test_controller_applies_injected_agreement_policy_before_input(self):
        for policy, count, reason in [('permitted', 1, 'action_or_time_limit'),
                                      ('confirmation_required', 0, 'policy_required')]:
            args = type('Args', (), dict(folder=self.root / policy, character='Test', max_actions=1,
                                         max_seconds=10, agreement_policy=policy))()
            capture, unused = self.dialogue_driver_stubs()
            def decide(config, decision_args):
                decision = dict(image_sha256=m.digest(decision_args.image), screen='agreement',
                                next_key='enter', confidence=1)
                output = dict(status='decision', decision=decision)
                decision_args.image.with_suffix('.decision.json').write_text(json.dumps(output))
            with self.subTest(policy=policy), patch.object(m, 'capture', side_effect=capture), \
                    patch.object(m, 'decide', side_effect=decide), patch.object(m, 'key') as key, \
                    patch.object(m, 'dialogue_fingerprint', return_value='agreement'), patch.object(m.time, 'sleep'):
                result = m.drive_dialogue({}, args)
            self.assertEqual(key.call_count, count)
            self.assertEqual(result['reason'], reason)

    def test_controller_rejects_low_confidence_permitted_agreement(self):
        args = type('Args', (), dict(folder=self.root / 'low-agreement', character='Test', max_actions=1,
                                     max_seconds=10, agreement_policy='permitted'))()
        capture, unused = self.dialogue_driver_stubs()
        def decide(config, decision_args):
            decision = dict(image_sha256=m.digest(decision_args.image), screen='agreement',
                            next_key='enter', confidence=.5)
            decision_args.image.with_suffix('.decision.json').write_text(json.dumps(dict(status='decision', decision=decision)))
        with patch.object(m, 'capture', side_effect=capture), patch.object(m, 'decide', side_effect=decide), \
                patch.object(m, 'key') as key:
            result = m.drive_dialogue({}, args)
        key.assert_not_called()
        self.assertEqual(result['reason'], 'low_confidence')

    def test_controller_retries_low_confidence_without_input(self):
        for recover in (True, False):
            args = type('Args', (), dict(folder=self.root / ('recover' if recover else 'exhaust'), character='Test',
                                         max_actions=10, max_seconds=10))()
            capture, unused = self.dialogue_driver_stubs()
            statuses = ['low_confidence'] * m.MAX_LOW_CONFIDENCE_RETRIES
            statuses += (['decision'] + ['low_confidence'] * m.MAX_LOW_CONFIDENCE_RETRIES + ['gameplay']) if recover else ['low_confidence']
            statuses = iter(statuses)
            def decide(config, decision_args):
                status = next(statuses)
                screen = 'gameplay' if status == 'gameplay' else 'dialogue'
                decision = dict(image_sha256=m.digest(decision_args.image), screen=screen,
                                next_key='stop' if screen == 'gameplay' else 'enter', confidence=1)
                decision_args.image.with_suffix('.decision.json').write_text(json.dumps(
                    dict(status='decision' if status == 'gameplay' else status, decision=decision)))
            with self.subTest(recover=recover), patch.object(m, 'capture', side_effect=capture), \
                    patch.object(m, 'decide', side_effect=decide), patch.object(m, 'key') as key, \
                    patch.object(m, 'dialogue_fingerprint', return_value='text'), patch.object(m.time, 'sleep'):
                result = m.drive_dialogue({}, args)
            self.assertEqual(key.call_count, int(recover))
            self.assertEqual(result['reason'], 'gameplay' if recover else 'low_confidence')
            self.assertEqual(sum(entry.get('recovery') == 'fresh_capture_without_input'
                                 for entry in result['captures']), m.MAX_LOW_CONFIDENCE_RETRIES * (2 if recover else 1))

    def test_dialogue_driver_rejects_excessive_limits_before_host_access(self):
        args = type('Args', (), dict(folder=self.root / 'drive', character='Test', max_actions=m.MAX_DIALOGUE_ACTIONS + 1, max_seconds=10))()
        with patch.object(m, 'capture') as capture, self.assertRaises(ValueError):
            m.drive_dialogue({}, args)
        capture.assert_not_called()

    def test_dialogue_driver_records_capture_failure_without_input(self):
        args = type('Args', (), dict(folder=self.root / 'failure', character='Test', max_actions=10, max_seconds=10))()
        with patch.object(m, 'capture', side_effect=ValueError('No matching window.')), patch.object(m, 'key') as key:
            result = m.drive_dialogue({}, args)
        key.assert_not_called()
        self.assertEqual(result['status'], 'blocked')
        self.assertEqual(result['actions'], 0)
        self.assertEqual(json.loads((args.folder / 'summary.json').read_text()), result)

    def test_dialogue_driver_binds_offscreen_input_to_its_current_capture(self):
        args = type('Args', (), dict(folder=self.root / 'offscreen-drive', character='Test', max_actions=1, max_seconds=10))()
        capture, decide = self.dialogue_driver_stubs()
        config = dict(offscreen_control=True)
        with patch.object(m, 'capture', side_effect=capture), patch.object(m, 'decide', side_effect=decide), \
                patch.object(m, 'dialogue_fingerprint', side_effect=['first', 'next']), \
                patch.object(m.time, 'sleep'), patch.object(m, 'key') as key:
            m.drive_dialogue(config, args)
        key.assert_called_once()
        self.assertEqual(key.call_args.args[0]['input_frame'], args.folder.resolve() / 'frame-000.png')
        self.assertNotIn('input_frame', config)

    def test_native_vision_result_is_validated_and_saved(self):
        result = dict(image_sha256=m.digest(self.image), screen='gameplay', next_key='stop', confidence=1)
        self.args.result = self.root / 'vision.json'
        self.args.result.write_text(json.dumps(result))
        with patch.object(m, 'jev_decide') as jev:
            m.decide({}, self.args)
            jev.assert_not_called()
        saved = json.loads(self.image.with_suffix('.decision.json').read_text())
        self.assertEqual(saved['provider'], 'vision')
        self.assertEqual(saved['decision']['next_key'], 'stop')

    def test_changed_dependency_blocks_runtime(self):
        runner, loader, ahk = [self.root / name for name in ('wine', 'loader.exe', 'ahk.exe')]
        for path in (runner, loader, ahk):
            path.write_bytes(b'original')
            path.chmod(0o700)
        prefix = self.root / 'prefix'
        prefix.mkdir()
        (prefix / 'system.reg').touch()
        install = self.root / 'game'
        install.mkdir()
        dll = install / 'FFXiMain.dll'
        dll.write_bytes(b'client')
        config = dict(schema=m.PROFILE_VERSION, runner=str(runner), loader=str(loader), ahk=str(ahk),
                      runner_sha256=m.digest(runner), loader_sha256=m.digest(loader), ahk_sha256=m.digest(ahk),
                      prefix=str(prefix), install=str(install), client_sha256=m.digest(dll))
        profile = self.root / 'profile.json'
        profile.write_text(json.dumps(config))
        m.load(profile)
        ahk.write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'ahk changed'):
            m.load(profile)

    def test_atomic_profile_is_private(self):
        profile = self.root / 'config' / 'profile.json'
        m.private_write(profile, b'{}')
        self.assertEqual(profile.stat().st_mode & 0o777, 0o600)
        m.private_write(profile, b'{"schema": 1}')
        self.assertEqual(json.loads(profile.read_text()), dict(schema=1))

    def test_ambiguous_windows_are_rejected(self):
        response = type('Result', (), dict(stdout=json.dumps([dict(name='FINAL FANTASY XI')] * 2)))()
        with patch.object(m, 'environment', return_value={}), patch.object(m, 'run', return_value=response):
            with self.assertRaisesRegex(ValueError, 'found 2'):
                m.window(dict(window_title='FINAL FANTASY XI'))

    def test_capture_can_read_one_offscreen_window_without_input(self):
        config = dict(window_title='FINAL FANTASY XI')
        target = dict(id=123, w=800, on_screen=False)
        output = self.root / 'offscreen.png'
        def execute(arguments, **kwargs):
            if arguments[0] == 'screencapture':
                Path(arguments[-1]).write_bytes(self.image.read_bytes())
        with patch.object(m, 'permissions'), patch.object(m, 'window', side_effect=[None, target]) as windows, \
                patch.object(m, 'run', side_effect=execute), patch.object(m, 'key') as key:
            m.capture(config, output)
        self.assertEqual(windows.call_args_list[0].kwargs, dict(allow_missing=True))
        self.assertEqual(windows.call_args_list[1].kwargs, dict(include_offscreen=True))
        key.assert_not_called()
        self.assertTrue(output.is_file())

    def test_offscreen_input_requires_fresh_matching_capture_and_host_identity(self):
        target = dict(id=123, pid=456)
        metadata = dict(image_sha256=m.digest(self.image), captured_unix=m.time.time(), window=target)
        record = self.image.with_suffix('.capture.json')
        record.write_text(json.dumps(metadata))
        config = dict(input_frame=self.image)
        with patch.object(m, 'window', return_value=target):
            self.assertEqual(m.validate_input_capture(config), target)
        with patch.object(m, 'window', return_value={**target, 'pid': target['pid'] + 1}), self.assertRaises(ValueError):
            m.validate_input_capture(config)
        record.write_text(json.dumps({**metadata, 'captured_unix': m.time.time() - m.MAX_INPUT_CAPTURE_AGE_SECONDS - 1}))
        with patch.object(m, 'window') as windows, self.assertRaises(ValueError):
            m.validate_input_capture(config)
        windows.assert_not_called()
        record.write_text(json.dumps(metadata))
        self.image.write_bytes(self.image.read_bytes() + b'changed')
        with self.assertRaises(ValueError):
            m.validate_input_capture(config)

    def test_offscreen_input_rejects_missing_capture_before_window_access(self):
        with patch.object(m, 'window') as windows, self.assertRaises(ValueError):
            m.validate_input_capture({})
        windows.assert_not_called()


if __name__ == '__main__':
    unittest.main()
